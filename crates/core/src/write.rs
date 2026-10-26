//! 企業・担当者の書き込みと、活動による状態の適用。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::sql::{Connection, OptionalExtension, Transaction, params};

use crate::{
    houjin, normalize,
    schema::{Db, WRITE_BASE, counts, fts},
    types::{
        ActivityId, ActivityInput, ActivityKind, CompanyId, CompanyInput, ContactId, ContactInput,
        EngineError, Industry, NextAction, Status,
    },
};

/// 検証済みの企業入力。company の電話・URLは空白除去済み。
#[derive(Debug, Clone)]
pub(crate) struct NormalizedCompany {
    pub company: CompanyInput,
    pub name_key: String,
}

/// 記録順で適用する活動。取り消し行と取り消し済み行は呼び出し側で除く。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoldInput {
    pub kind: ActivityKind,
    pub at: i64,
    pub status_after: Option<Status>,
    pub next_action: NextAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Folded {
    pub status: Status,
    pub last_activity_at: i64,
    pub next_action_at: Option<i64>,
}

fn storage(error: crate::sql::Error) -> EngineError {
    EngineError::Storage(Box::new(error))
}

fn invalid(field: &str, reason: &str) -> EngineError {
    EngineError::InvalidInput {
        field: field.into(),
        reason: reason.into(),
    }
}

pub(crate) fn reject_nul(field: &str, value: &str) -> Result<(), EngineError> {
    if value.contains('\0') {
        return Err(invalid(field, "must not contain NUL"));
    }
    Ok(())
}

pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn optional_text(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn phone(text: Option<&str>) -> Option<String> {
    optional_text(text.map(normalize::phone).as_deref())
}

fn valid_date(date: &str) -> bool {
    let b = date.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let year: u32 = date[..4].parse().unwrap_or(0);
    let month: u32 = date[5..7].parse().unwrap_or(0);
    let day: u32 = date[8..].parse().unwrap_or(0);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    year > 0 && day > 0 && day <= days
}

pub(crate) fn validate_company_input(c: &CompanyInput) -> Result<NormalizedCompany, EngineError> {
    reject_nul("name", &c.name)?;
    for (field, value) in [
        ("phone", c.phone.as_deref()),
        ("website", c.website.as_deref()),
        ("source_id", c.source_id.as_deref()),
    ] {
        if let Some(value) = value {
            reject_nul(field, value)?;
        }
    }
    if c.name.trim().is_empty() {
        return Err(EngineError::EmptyName);
    }
    if let Some(number) = &c.corporate_number {
        houjin::validate(number)?;
    }
    if !(1..=47).contains(&c.prefecture) {
        return Err(invalid("prefecture", "expected 1..=47"));
    }
    if let Some(code) = &c.municipality
        && (code.len() != 5
            || !code.bytes().all(|b| b.is_ascii_digit())
            || code[..2].parse::<u8>().ok() != Some(c.prefecture))
    {
        return Err(invalid(
            "municipality",
            "expected five ASCII digits matching prefecture",
        ));
    }
    if c.source_as_of.as_deref().is_some_and(|d| !valid_date(d)) {
        return Err(invalid("source_as_of", "expected a valid YYYY-MM-DD date"));
    }
    let mut company = c.clone();
    company.phone = phone(c.phone.as_deref());
    company.website = optional_text(c.website.as_deref());
    Ok(NormalizedCompany {
        company,
        name_key: normalize::name_key(&c.name),
    })
}

fn check_duplicate(
    conn: &Connection,
    number: Option<&str>,
    except: Option<CompanyId>,
) -> Result<(), EngineError> {
    if let Some(number) = number {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM companies WHERE corporate_number=?1 AND (?2 IS NULL OR id<>?2)",
                (number, except.map(|id| id.0)),
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        if let Some(id) = existing {
            return Err(EngineError::DuplicateCorporateNumber {
                existing: CompanyId(id),
            });
        }
    }
    Ok(())
}

// 表・K1〜K10・全文索引への文字列の写しを見込む。
pub(crate) fn company_estimate(c: &CompanyInput) -> u64 {
    let bytes = [
        Some(c.name.as_str()),
        c.corporate_number.as_deref(),
        c.municipality.as_deref(),
        c.phone.as_deref(),
        c.website.as_deref(),
        c.source_id.as_deref(),
        c.source_as_of.as_deref(),
    ]
    .into_iter()
    .flatten()
    .fold(0u64, |n, s| n.saturating_add(s.len() as u64));
    WRITE_BASE
        .saturating_add(bytes.saturating_mul(8))
        .saturating_add((normalize::name_key(&c.name).len() as u64).saturating_mul(16))
}

pub(crate) fn update_company_estimate(c: &CompanyInput, old_key: &str) -> u64 {
    company_estimate(c).saturating_add((old_key.len() as u64).saturating_mul(16))
}

pub(crate) fn contact_estimate(c: &ContactInput) -> u64 {
    let bytes = [
        Some(c.name.as_str()),
        c.phone.as_deref(),
        c.email.as_deref(),
    ]
    .into_iter()
    .flatten()
    .fold(0u64, |n, s| n.saturating_add(s.len() as u64));
    WRITE_BASE.saturating_add(bytes.saturating_mul(4))
}

pub(crate) fn activity_estimate(a: &ActivityInput) -> u64 {
    WRITE_BASE.saturating_add((a.note.as_ref().map_or(0, String::len) as u64).saturating_mul(4))
}

// 行の再構築で既存のオーバーフローページも書き直される。
pub(crate) fn company_row_bytes(conn: &Connection, id: CompanyId) -> Result<u64, EngineError> {
    let bytes: Option<i64> = conn
        .query_row(
            "SELECT length(CAST(name AS BLOB)) + length(CAST(name_key AS BLOB))
         + coalesce(length(CAST(corporate_number AS BLOB)),0)
         + coalesce(length(CAST(municipality_code AS BLOB)),0)
         + coalesce(length(CAST(phone AS BLOB)),0)
         + coalesce(length(CAST(website AS BLOB)),0)
         + coalesce(length(CAST(source_id AS BLOB)),0)
         + coalesce(length(CAST(source_as_of AS BLOB)),0)
         FROM companies WHERE id=?1",
            [id.0],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    u64::try_from(bytes.unwrap_or(0)).map_err(|_| EngineError::Corrupt)
}

pub(crate) fn validate_activity_times(at: i64, next: NextAction) -> Result<(), EngineError> {
    if at < 0 {
        return Err(invalid("at", "must be nonnegative"));
    }
    if matches!(next, NextAction::Set(t) if t < 0) {
        return Err(invalid("next_action", "must be nonnegative"));
    }
    Ok(())
}

pub(crate) fn register_company(db: &mut Db, c: &CompanyInput) -> Result<CompanyId, EngineError> {
    db.check_origin(c.origin)?;
    let normalized = validate_company_input(c)?;
    let estimate = company_estimate(c);
    let c = &normalized.company;
    db.write(estimate, |tx| {
        check_duplicate(tx, c.corporate_number.as_deref(), None)?;
        tx.execute("INSERT INTO companies(corporate_number,name,name_key,prefecture_code,municipality_code,industry_code,status,phone,website,owner_id,base_status,is_sole_proprietor,source_id,source_as_of,created_at,updated_at,last_activity_at,next_action_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?7,?11,?12,?13,?14,?14,0,NULL)",
            params![c.corporate_number,c.name,normalized.name_key,c.prefecture,c.municipality,c.industry.map(Industry::code),c.status.code(),c.phone,c.website,c.owner.map(|u| u.0),c.is_sole_proprietor,c.source_id,c.source_as_of,now_secs()]).map_err(storage)?;
        let id = CompanyId(tx.last_insert_rowid());
        fts::insert(tx, id.0, &normalized.name_key)?;
        counts::apply_delta(tx, None, Some((c.status, c.phone.is_some())))?;
        Ok(id)
    })
}

fn status(code: u8) -> Result<Status, EngineError> {
    Status::from_code(code).ok_or(EngineError::Corrupt)
}

struct CompanyState {
    folded: Folded,
    base: Status,
    has_phone: bool,
    erased: bool,
}

fn company_state(conn: &Connection, id: CompanyId) -> Result<CompanyState, EngineError> {
    let row = conn.query_row("SELECT status,last_activity_at,next_action_at,base_status,has_phone,erased_at IS NOT NULL FROM companies WHERE id=?1", [id.0], |r| {
        Ok((r.get::<_,u8>(0)?,r.get(1)?,r.get(2)?,r.get::<_,u8>(3)?,r.get(4)?,r.get(5)?))
    }).optional().map_err(storage)?.ok_or(EngineError::NotFound)?;
    Ok(CompanyState {
        folded: Folded {
            status: status(row.0)?,
            last_activity_at: row.1,
            next_action_at: row.2,
        },
        base: status(row.3)?,
        has_phone: row.4,
        erased: row.5,
    })
}

pub(crate) fn update_company(
    db: &mut Db,
    id: CompanyId,
    c: &CompanyInput,
) -> Result<(), EngineError> {
    let origin_check = db.check_origin(c.origin);
    let old_key: Option<String> = db
        .conn()
        .query_row("SELECT name_key FROM companies WHERE id=?1", [id.0], |r| {
            r.get(0)
        })
        .optional()
        .map_err(storage)?;
    let estimate = update_company_estimate(c, old_key.as_deref().unwrap_or(""));
    db.write(estimate, |tx| {
        let old = company_state(tx, id)?;
        if old.erased { return Err(invalid("id", "erased")); }
        origin_check?;
        let normalized = validate_company_input(c)?;
        let c = &normalized.company;
        if old.folded.status != c.status { return Err(EngineError::StatusViaActivity); }
        check_duplicate(tx, c.corporate_number.as_deref(), Some(id))?;
        let old_key: String = tx.query_row("SELECT name_key FROM companies WHERE id=?1", [id.0], |r| r.get(0)).map_err(storage)?;
        if old_key != normalized.name_key { fts::delete(tx, id.0, &old_key)?; }
        tx.execute("UPDATE companies SET corporate_number=?2,name=?3,name_key=?4,prefecture_code=?5,municipality_code=?6,industry_code=?7,phone=?8,website=?9,owner_id=?10,is_sole_proprietor=?11,source_id=?12,source_as_of=?13,updated_at=?14 WHERE id=?1",
            params![id.0,c.corporate_number,c.name,normalized.name_key,c.prefecture,c.municipality,c.industry.map(Industry::code),c.phone,c.website,c.owner.map(|u| u.0),c.is_sole_proprietor,c.source_id,c.source_as_of,now_secs()]).map_err(storage)?;
        if old_key != normalized.name_key { fts::insert(tx, id.0, &normalized.name_key)?; }
        counts::apply_delta(tx, Some((c.status,old.has_phone)), Some((c.status,c.phone.is_some())))
    })
}

pub(crate) fn add_contact(db: &mut Db, c: &ContactInput) -> Result<ContactId, EngineError> {
    reject_nul("name", &c.name)?;
    for (field, value) in [("phone", c.phone.as_deref()), ("email", c.email.as_deref())] {
        if let Some(value) = value {
            reject_nul(field, value)?;
        }
    }
    if c.name.trim().is_empty() {
        return Err(EngineError::EmptyName);
    }
    let estimate = contact_estimate(c);
    db.write(estimate, |tx| {
        if company_state(tx, c.company)?.erased {
            return Err(EngineError::NotFound);
        }
        tx.execute(
            "INSERT INTO contacts(company_id,name,phone,email,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![
                c.company.0,
                c.name,
                phone(c.phone.as_deref()),
                optional_text(c.email.as_deref()),
                now_secs()
            ],
        )
        .map_err(storage)?;
        Ok(ContactId(tx.last_insert_rowid()))
    })
}

pub(crate) fn update_contact(
    db: &mut Db,
    id: ContactId,
    c: &ContactInput,
) -> Result<(), EngineError> {
    reject_nul("name", &c.name)?;
    for (field, value) in [("phone", c.phone.as_deref()), ("email", c.email.as_deref())] {
        if let Some(value) = value {
            reject_nul(field, value)?;
        }
    }
    if c.name.trim().is_empty() {
        return Err(EngineError::EmptyName);
    }
    let previous_bytes: i64 = db.conn().query_row(
        "SELECT coalesce(length(CAST(name AS BLOB)),0)+coalesce(length(CAST(phone AS BLOB)),0)+coalesce(length(CAST(email AS BLOB)),0) FROM contacts WHERE id=?1",
        [id.0], |r| r.get(0),
    ).optional().map_err(storage)?.unwrap_or(0);
    let previous_bytes = u64::try_from(previous_bytes).map_err(|_| EngineError::Corrupt)?;
    db.write(
        contact_estimate(c).saturating_add(previous_bytes.saturating_mul(4)),
        |tx| {
            if company_state(tx, c.company)?.erased {
                return Err(EngineError::NotFound);
            }
            let (company, erased): (i64, Option<i64>) = tx
                .query_row(
                    "SELECT company_id,erased_at FROM contacts WHERE id=?1",
                    [id.0],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(storage)?
                .ok_or(EngineError::NotFound)?;
            if company != c.company.0 {
                return Err(EngineError::ForeignContact);
            }
            if erased.is_some() {
                return Err(EngineError::NotFound);
            }
            tx.execute(
                "UPDATE contacts SET name=?2,phone=?3,email=?4 WHERE id=?1",
                params![
                    id.0,
                    c.name.trim(),
                    phone(c.phone.as_deref()),
                    optional_text(c.email.as_deref())
                ],
            )
            .map_err(storage)?;
            Ok(())
        },
    )
}

fn apply(value: &mut Folded, a: FoldInput) {
    if let Some(status) = a.status_after {
        value.status = status;
    }
    match a.next_action {
        NextAction::Keep => {}
        NextAction::Clear => value.next_action_at = None,
        NextAction::Set(at) => value.next_action_at = Some(at),
    }
    if matches!(
        a.kind,
        ActivityKind::Call | ActivityKind::Visit | ActivityKind::Email | ActivityKind::Memo
    ) {
        value.last_activity_at = value.last_activity_at.max(a.at);
    }
}

pub(crate) fn fold(base: Status, activities: impl Iterator<Item = FoldInput>) -> Folded {
    let mut value = Folded {
        status: base,
        last_activity_at: 0,
        next_action_at: None,
    };
    for a in activities {
        apply(&mut value, a);
    }
    value
}

fn read_fold(row: &crate::sql::Row<'_>, start: usize) -> Result<FoldInput, EngineError> {
    let kind =
        ActivityKind::from_code(row.get(start).map_err(storage)?).ok_or(EngineError::Corrupt)?;
    let after: Option<u8> = row.get(start + 2).map_err(storage)?;
    let next = match row.get::<_, u8>(start + 3).map_err(storage)? {
        0 => NextAction::Keep,
        1 => NextAction::Clear,
        2 => NextAction::Set(row.get(start + 4).map_err(storage)?),
        _ => return Err(EngineError::Corrupt),
    };
    Ok(FoldInput {
        kind,
        at: row.get(start + 1).map_err(storage)?,
        status_after: after.map(status).transpose()?,
        next_action: next,
    })
}

fn refold(conn: &Connection, id: CompanyId, base: Status) -> Result<Folded, EngineError> {
    let mut stmt = conn.prepare("SELECT a.kind,a.at,a.status_after,a.next_action_kind,a.next_action_at FROM activities a WHERE a.company_id=?1 AND a.kind<>9 AND NOT EXISTS(SELECT 1 FROM activities r WHERE r.reverses_id=a.id) ORDER BY a.id").map_err(storage)?;
    let mut rows = stmt.query([id.0]).map_err(storage)?;
    let mut value = fold(base, std::iter::empty());
    while let Some(row) = rows.next().map_err(storage)? {
        apply(&mut value, read_fold(row, 0)?);
    }
    Ok(value)
}

pub(crate) fn record_activity(db: &mut Db, a: &ActivityInput) -> Result<ActivityId, EngineError> {
    let estimate = activity_estimate(a)
        .saturating_add(company_row_bytes(db.conn(), a.company)?.saturating_mul(4));
    db.write(estimate, |tx| record_activity_tx(tx, a, false))
}

/// Err は呼び出し側へ伝播し、同じ確定の全体を戻すこと。
pub(crate) fn record_activity_tx(
    tx: &Transaction<'_>,
    a: &ActivityInput,
    allow_release: bool,
) -> Result<ActivityId, EngineError> {
    validate_activity_times(a.at, a.next_action)?;
    if let Some(note) = &a.note {
        reject_nul("note", note)?;
    }
    let old = company_state(tx, a.company)?;
    if old.erased {
        return Err(invalid("id", "erased"));
    }
    if let Some(contact) = a.contact {
        let company: i64 = tx
            .query_row(
                "SELECT company_id FROM contacts WHERE id=?1",
                [contact.0],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?
            .ok_or(EngineError::NotFound)?;
        if company != a.company.0 {
            return Err(EngineError::ForeignContact);
        }
    }
    if a.kind == ActivityKind::Cancel {
        if a.reverses.is_none() || a.status_after.is_some() || a.next_action != NextAction::Keep {
            return Err(invalid(
                "reverses",
                "cancellation requires a target, no status and Keep",
            ));
        }
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM activities a WHERE a.id=?1 AND a.company_id=?2 AND a.kind<>9 AND NOT EXISTS(SELECT 1 FROM activities r WHERE r.reverses_id=a.id))",
            (a.reverses.map(|id| id.0),a.company.0), |r| r.get(0)).map_err(storage)?;
        if !valid {
            return Err(EngineError::InvalidCancel);
        }
    } else {
        if a.reverses.is_some() {
            return Err(invalid(
                "reverses",
                "only cancellation may reverse an activity",
            ));
        }
        if a.kind == ActivityKind::StatusChange && a.status_after.is_none() {
            return Err(invalid("status_after", "required for status change"));
        }
    }
    if !allow_release
        && old.folded.status == Status::Excluded
        && a.status_after.is_some_and(|s| s != Status::Excluded)
    {
        return Err(EngineError::ExcludedCompany);
    }
    let (next_kind, next_at) = match a.next_action {
        NextAction::Keep => (0, None),
        NextAction::Clear => (1, None),
        NextAction::Set(at) => (2, Some(at)),
    };
    tx.execute("INSERT INTO activities(company_id,contact_id,kind,at,note,status_after,next_action_kind,next_action_at,reverses_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![a.company.0,a.contact.map(|id| id.0),a.kind.code(),a.at,a.note,a.status_after.map(Status::code),next_kind,next_at,a.reverses.map(|id| id.0)]).map_err(storage)?;
    let id = ActivityId(tx.last_insert_rowid());
    let value = if a.kind == ActivityKind::Cancel {
        refold(tx, a.company, old.base)?
    } else {
        let mut value = old.folded;
        apply(
            &mut value,
            FoldInput {
                kind: a.kind,
                at: a.at,
                status_after: a.status_after,
                next_action: a.next_action,
            },
        );
        value
    };
    if !allow_release && old.folded.status == Status::Excluded && value.status != Status::Excluded {
        return Err(EngineError::ExcludedCompany);
    }
    tx.execute("UPDATE companies SET status=?2,last_activity_at=?3,next_action_at=?4,updated_at=?5 WHERE id=?1",
        params![a.company.0,value.status.code(),value.last_activity_at,value.next_action_at,now_secs()]).map_err(storage)?;
    counts::apply_delta(
        tx,
        Some((old.folded.status, old.has_phone)),
        Some((value.status, old.has_phone)),
    )?;
    Ok(id)
}

pub(crate) fn release_exclusion(
    db: &mut Db,
    id: CompanyId,
    to: Status,
) -> Result<ActivityId, EngineError> {
    let estimate = WRITE_BASE.saturating_add(company_row_bytes(db.conn(), id)?.saturating_mul(4));
    db.write(estimate, |tx| {
        if company_state(tx, id)?.folded.status != Status::Excluded {
            return Err(invalid("status", "not excluded"));
        }
        if to == Status::Excluded {
            return Err(invalid("status", "release target is excluded"));
        }
        record_activity_tx(
            tx,
            &ActivityInput {
                company: id,
                contact: None,
                kind: ActivityKind::StatusChange,
                at: now_secs(),
                note: None,
                status_after: Some(to),
                next_action: NextAction::Keep,
                reverses: None,
            },
            true,
        )
    })
}

pub(crate) fn verify_folds(conn: &Connection) -> Result<bool, EngineError> {
    let empty_ok: bool = conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM companies c WHERE NOT EXISTS(SELECT 1 FROM activities a WHERE a.company_id=c.id) AND (c.status<>c.base_status OR c.last_activity_at<>0 OR c.next_action_at IS NOT NULL))",[],|r|r.get(0)).map_err(storage)?;
    if !empty_ok {
        return Ok(false);
    }
    // 取り消しだけが残る企業も、元の活動を含むストリームで照合する。
    let mut stmt = conn.prepare("SELECT c.id,c.base_status,c.status,c.last_activity_at,c.next_action_at,a.kind,a.at,a.status_after,a.next_action_kind,a.next_action_at,(a.kind<>9 AND r.id IS NULL) FROM activities a JOIN companies c ON c.id=a.company_id LEFT JOIN activities r ON r.reverses_id=a.id ORDER BY a.company_id,a.id").map_err(storage)?;
    let mut rows = stmt.query([]).map_err(storage)?;
    let mut current: Option<(i64, Folded, Folded)> = None;
    while let Some(row) = rows.next().map_err(storage)? {
        let id: i64 = row.get(0).map_err(storage)?;
        if current.as_ref().is_none_or(|c| c.0 != id) {
            if current.is_some_and(|(_, actual, expected)| actual != expected) {
                return Ok(false);
            }
            current = Some((
                id,
                fold(status(row.get(1).map_err(storage)?)?, std::iter::empty()),
                Folded {
                    status: status(row.get(2).map_err(storage)?)?,
                    last_activity_at: row.get(3).map_err(storage)?,
                    next_action_at: row.get(4).map_err(storage)?,
                },
            ));
        }
        if row.get::<_, bool>(10).map_err(storage)?
            && let Some((_, actual, _)) = &mut current
        {
            apply(actual, read_fold(row, 5)?);
        }
    }
    Ok(current.is_none_or(|(_, actual, expected)| actual == expected))
}

#[cfg(test)]
mod tests;
