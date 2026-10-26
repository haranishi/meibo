//! 独立したToDo・予定・商談。基盤の保存形式とは別に拡張版を管理する。

use crate::sql::{Connection, OptionalExtension, Row, params, params_from_iter, types::Value};

use crate::{
    Engine,
    schema::WRITE_BASE,
    types::{CompanyId, ContactId, EngineError},
    write::{now_secs, reject_nul},
};

/// ブラウザでも整数として正確に扱える最大金額。
pub const MAX_AMOUNT_JPY: i64 = 9_007_199_254_740_991;
const MAX_TIME: i64 = 253_402_300_799;
const EXTENSION_VERSION: u32 = 1;

/// 仕事の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    /// 期限と完了状態を持つタスク。
    Task,
    /// 開始と終了を持つ予定。
    Event,
}

impl WorkKind {
    const fn code(self) -> i64 {
        match self {
            Self::Task => 1,
            Self::Event => 2,
        }
    }
}

/// ToDoまたは予定の登録・全項目更新。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemInput {
    /// 所属企業。更新で移動できない。
    pub company: CompanyId,
    /// 関連する同じ企業の担当者。
    pub contact: Option<ContactId>,
    /// 種類。更新で変更できない。
    pub kind: WorkKind,
    /// 1〜200文字の件名。
    pub subject: String,
    /// ToDoの期限（UNIX秒）。
    pub due_at: Option<i64>,
    /// 予定の開始（UNIX秒）。
    pub start_at: Option<i64>,
    /// 予定の終了（UNIX秒）。
    pub end_at: Option<i64>,
    /// ToDoの完了。予定ではfalseだけ。
    pub completed: bool,
}

/// 保存されたタスクまたは予定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItem {
    /// 独立した識別番号。
    pub id: i64,
    /// 企業の名称。個人事業では非表示。
    pub company_name: Option<String>,
    /// 保存内容。
    pub input: WorkItemInput,
}

/// 日付順の仕事一覧の条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkQuery {
    /// 企業を限定する。
    pub company: Option<CompanyId>,
    /// 種類を限定する。
    pub kind: Option<WorkKind>,
    /// 完了状態を限定する。
    pub completed: Option<bool>,
    /// 前ページ末尾のID。同じ条件に属するIDだけを許す。
    pub after: Option<i64>,
    /// 1〜200件。
    pub limit: u32,
}

/// 商談の登録・全項目更新。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpportunityInput {
    /// 所属企業。更新で移動できない。
    pub company: CompanyId,
    /// 関連する同じ企業の担当者。
    pub contact: Option<ContactId>,
    /// 1〜200文字の商談名。
    pub name: String,
    /// 0以上の整数円。最大値は`MAX_AMOUNT_JPY`。
    pub amount_jpy: i64,
    /// 見込み/評価/提案/交渉/受注/失注の0〜5。
    pub stage: u8,
    /// 完了予定日（YYYY-MM-DD）。
    pub close_date: String,
}

/// 保存された商談。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opportunity {
    /// 独立した識別番号。
    pub id: i64,
    /// 企業の名称。個人事業では非表示。
    pub company_name: Option<String>,
    /// 保存内容。
    pub input: OpportunityInput,
}

/// 完了予定日順の商談一覧の条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpportunityQuery {
    /// 企業を限定する。
    pub company: Option<CompanyId>,
    /// 前ページ末尾のID。同じ条件に属するIDだけを許す。
    pub after: Option<i64>,
    /// 1〜200件。
    pub limit: u32,
}

/// 件数と続きの位置を持つ一覧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrmPage<T> {
    /// 今回のレコード。
    pub items: Vec<T>,
    /// カーソル適用前の一致件数。
    pub total: u64,
    /// 続きがある場合の末尾ID。
    pub next: Option<i64>,
}

const DDL: &str = r"
CREATE TABLE IF NOT EXISTS crm_work_items(
 id INTEGER PRIMARY KEY,
 company_id INTEGER NOT NULL REFERENCES companies(id),
 contact_id INTEGER REFERENCES contacts(id),
 kind INTEGER NOT NULL CHECK(kind IN(1,2)),
 subject TEXT NOT NULL,
 due_at INTEGER, start_at INTEGER, end_at INTEGER,
 completed INTEGER NOT NULL CHECK(completed IN(0,1)),
 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, erased_at INTEGER,
 CHECK(erased_at IS NOT NULL OR length(trim(subject))>0),
 CHECK((kind=1 AND due_at IS NOT NULL AND start_at IS NULL AND end_at IS NULL)
    OR (kind=2 AND due_at IS NULL AND start_at IS NOT NULL AND end_at>start_at AND completed=0)));
CREATE INDEX IF NOT EXISTS crm_work_company ON crm_work_items(company_id,id);
CREATE INDEX IF NOT EXISTS crm_work_contact ON crm_work_items(contact_id,id);
CREATE INDEX IF NOT EXISTS crm_work_schedule ON crm_work_items(COALESCE(due_at,start_at),id);
CREATE TABLE IF NOT EXISTS crm_opportunities(
 id INTEGER PRIMARY KEY,
 company_id INTEGER NOT NULL REFERENCES companies(id),
 contact_id INTEGER REFERENCES contacts(id),
 name TEXT NOT NULL,
 amount_jpy INTEGER NOT NULL CHECK(amount_jpy BETWEEN 0 AND 9007199254740991),
 stage INTEGER NOT NULL CHECK(stage BETWEEN 0 AND 5),
 close_date TEXT NOT NULL,
 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, erased_at INTEGER,
 CHECK(erased_at IS NOT NULL OR length(trim(name))>0));
CREATE INDEX IF NOT EXISTS crm_opportunity_company ON crm_opportunities(company_id,id);
CREATE INDEX IF NOT EXISTS crm_opportunity_contact ON crm_opportunities(contact_id,id);
CREATE INDEX IF NOT EXISTS crm_opportunity_close ON crm_opportunities(close_date,id);
CREATE TRIGGER IF NOT EXISTS crm_contact_erase AFTER UPDATE OF erased_at ON contacts
WHEN OLD.erased_at IS NULL AND NEW.erased_at IS NOT NULL BEGIN
 UPDATE crm_work_items SET subject='',contact_id=NULL,erased_at=NEW.erased_at WHERE contact_id=NEW.id;
 UPDATE crm_opportunities SET name='',contact_id=NULL,amount_jpy=0,erased_at=NEW.erased_at WHERE contact_id=NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS crm_company_erase AFTER UPDATE OF erased_at ON companies
WHEN OLD.erased_at IS NULL AND NEW.erased_at IS NOT NULL BEGIN
 UPDATE crm_work_items SET subject='',contact_id=NULL,erased_at=NEW.erased_at WHERE company_id=NEW.id;
 UPDATE crm_opportunities SET name='',contact_id=NULL,amount_jpy=0,erased_at=NEW.erased_at WHERE company_id=NEW.id;
END;
";

/// 呼出側の同じトランザクション内で、互換拡張を一括作成する。
pub(crate) fn initialize(conn: &Connection) -> Result<(), EngineError> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='crm_schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    if let Some(version) = version {
        let found = version.parse::<u32>().map_err(|_| EngineError::Corrupt)?;
        if found > EXTENSION_VERSION {
            return Err(EngineError::UnsupportedVersion {
                found,
                supported: EXTENSION_VERSION,
            });
        }
        if found < EXTENSION_VERSION {
            return Err(EngineError::NeedsMigration);
        }
    }
    conn.execute_batch(DDL).map_err(storage)?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(key,value) VALUES('crm_schema_version',?1)",
        [EXTENSION_VERSION.to_string()],
    )
    .map_err(storage)?;
    Ok(())
}

impl Engine {
    /// タスクまたは予定を追加する。
    /// # Errors
    /// 種別不一致、不正入力、企業/担当者不在、他社担当者、除外、保存の失敗。
    pub fn crm_create_work_item(&mut self, input: &WorkItemInput) -> Result<i64, EngineError> {
        let subject = validate_work(input)?;
        self.db.write(WRITE_BASE + 4096, |tx| {
            target(tx, input.company, input.contact)?;
            let now = now_secs();
            tx.execute(
                "INSERT INTO crm_work_items(company_id,contact_id,kind,subject,due_at,start_at,end_at,completed,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",
                params![input.company.0,input.contact.map(|c| c.0),input.kind.code(),subject,input.due_at,input.start_at,input.end_at,input.completed,now],
            ).map_err(storage)?;
            Ok(tx.last_insert_rowid())
        })
    }

    /// 同じ企業・種類の仕事を更新する。
    /// # Errors
    /// 不正入力、移動要求、対象不在、他社担当者、除外、保存の失敗。
    pub fn crm_update_work_item(
        &mut self,
        id: i64,
        input: &WorkItemInput,
    ) -> Result<(), EngineError> {
        let subject = validate_work(input)?;
        self.db.write(WRITE_BASE + 4096, |tx| {
            let (company, kind, _) = work_target(tx, id)?;
            if company != input.company || kind != input.kind {
                return Err(invalid("company_id/kind", "cannot move a work item"));
            }
            target(tx, input.company, input.contact)?;
            tx.execute(
                "UPDATE crm_work_items SET contact_id=?2,subject=?3,due_at=?4,start_at=?5,end_at=?6,completed=?7,updated_at=?8 WHERE id=?1",
                params![id,input.contact.map(|c| c.0),subject,input.due_at,input.start_at,input.end_at,input.completed,now_secs()],
            ).map_err(storage)?;
            Ok(())
        })
    }

    /// ToDoを完了または再開する。予定には適用しない。
    /// # Errors
    /// 対象不在、予定への適用、除外、保存の失敗。
    pub fn crm_set_completion(&mut self, id: i64, completed: bool) -> Result<(), EngineError> {
        self.db.write(WRITE_BASE + 4096, |tx| {
            let (company, kind, contact) = work_target(tx, id)?;
            if kind != WorkKind::Task {
                return Err(invalid("kind", "only tasks can be completed"));
            }
            target(tx, company, contact)?;
            tx.execute(
                "UPDATE crm_work_items SET completed=?2,updated_at=?3 WHERE id=?1",
                params![id, completed, now_secs()],
            )
            .map_err(storage)?;
            Ok(())
        })
    }

    /// 期限・開始日時とIDの昇順で仕事を取得する。
    /// # Errors
    /// 不正条件、別条件のカーソル、種別不一致、保存の失敗。
    pub fn crm_work_items(&self, query: &WorkQuery) -> Result<CrmPage<WorkItem>, EngineError> {
        validate_query(query.company, query.after, query.limit)?;
        let (base, mut args) = work_filter(query);
        let snapshot = self.db.conn().unchecked_transaction().map_err(storage)?;
        let conn: &Connection = &snapshot;
        let total = total(conn, &base, &args)?;
        let mut suffix = String::new();
        if let Some(id) = query.after {
            let mut anchor_args = args.clone();
            anchor_args.push(Value::Integer(id));
            let at: i64 = conn
                .query_row(
                    &format!("SELECT COALESCE(w.due_at,w.start_at) {base} AND w.id=?"),
                    params_from_iter(anchor_args),
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?
                .ok_or_else(|| invalid("after", "cursor does not match filters"))?;
            suffix.push_str(" AND (COALESCE(w.due_at,w.start_at),w.id)>(?,?)");
            args.extend([Value::Integer(at), Value::Integer(id)]);
        }
        args.push(Value::Integer(i64::from(query.limit) + 1));
        let sql = format!(
            "SELECT w.id,w.company_id,CASE WHEN c.is_sole_proprietor=1 THEN NULL ELSE c.name END,w.contact_id,w.kind,w.subject,w.due_at,w.start_at,w.end_at,w.completed {base}{suffix} ORDER BY COALESCE(w.due_at,w.start_at),w.id LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql).map_err(storage)?;
        let items = stmt
            .query_map(params_from_iter(args), work_row)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        Ok(page(items, total, query.limit, |item| item.id))
    }

    /// 商談を追加する。企業の営業状態は変更しない。
    /// # Errors
    /// 不正入力、企業/担当者不在、他社担当者、除外、保存の失敗。
    pub fn crm_create_opportunity(&mut self, input: &OpportunityInput) -> Result<i64, EngineError> {
        let name = validate_opportunity(input)?;
        self.db.write(WRITE_BASE + 4096, |tx| {
            target(tx, input.company, input.contact)?;
            tx.execute(
                "INSERT INTO crm_opportunities(company_id,contact_id,name,amount_jpy,stage,close_date,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
                params![input.company.0,input.contact.map(|c| c.0),name,input.amount_jpy,input.stage,input.close_date,now_secs()],
            ).map_err(storage)?;
            Ok(tx.last_insert_rowid())
        })
    }

    /// 同じ企業の商談を更新する。
    /// # Errors
    /// 不正入力、企業の移動要求、対象不在、他社担当者、除外、保存の失敗。
    pub fn crm_update_opportunity(
        &mut self,
        id: i64,
        input: &OpportunityInput,
    ) -> Result<(), EngineError> {
        let name = validate_opportunity(input)?;
        self.db.write(WRITE_BASE + 4096, |tx| {
            let (company, _) = opportunity_target(tx, id)?;
            if company != input.company {
                return Err(invalid("company_id", "cannot move an opportunity"));
            }
            target(tx, input.company, input.contact)?;
            tx.execute(
                "UPDATE crm_opportunities SET contact_id=?2,name=?3,amount_jpy=?4,stage=?5,close_date=?6,updated_at=?7 WHERE id=?1",
                params![id,input.contact.map(|c| c.0),name,input.amount_jpy,input.stage,input.close_date,now_secs()],
            ).map_err(storage)?;
            Ok(())
        })
    }

    /// 商談の段階だけを変更する。
    /// # Errors
    /// 不正段階、対象不在、除外、保存の失敗。
    pub fn crm_set_stage(&mut self, id: i64, stage: u8) -> Result<(), EngineError> {
        if stage > 5 {
            return Err(invalid("stage", "must be between 0 and 5"));
        }
        self.db.write(WRITE_BASE + 4096, |tx| {
            let (company, contact) = opportunity_target(tx, id)?;
            target(tx, company, contact)?;
            tx.execute(
                "UPDATE crm_opportunities SET stage=?2,updated_at=?3 WHERE id=?1",
                params![id, stage, now_secs()],
            )
            .map_err(storage)?;
            Ok(())
        })
    }

    /// 完了予定日とIDの昇順で商談を取得する。
    /// # Errors
    /// 不正条件、別条件のカーソル、種別不一致、保存の失敗。
    pub fn crm_opportunities(
        &self,
        query: &OpportunityQuery,
    ) -> Result<CrmPage<Opportunity>, EngineError> {
        validate_query(query.company, query.after, query.limit)?;
        let (base, mut args) = opportunity_filter(query.company);
        let snapshot = self.db.conn().unchecked_transaction().map_err(storage)?;
        let conn: &Connection = &snapshot;
        let total = total(conn, &base, &args)?;
        let mut suffix = String::new();
        if let Some(id) = query.after {
            let mut anchor_args = args.clone();
            anchor_args.push(Value::Integer(id));
            let date: String = conn
                .query_row(
                    &format!("SELECT o.close_date {base} AND o.id=?"),
                    params_from_iter(anchor_args),
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?
                .ok_or_else(|| invalid("after", "cursor does not match filters"))?;
            suffix.push_str(" AND (o.close_date,o.id)>(?,?)");
            args.extend([Value::Text(date), Value::Integer(id)]);
        }
        args.push(Value::Integer(i64::from(query.limit) + 1));
        let sql = format!(
            "SELECT o.id,o.company_id,CASE WHEN c.is_sole_proprietor=1 THEN NULL ELSE c.name END,o.contact_id,o.name,o.amount_jpy,o.stage,o.close_date {base}{suffix} ORDER BY o.close_date,o.id LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql).map_err(storage)?;
        let items = stmt
            .query_map(params_from_iter(args), opportunity_row)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        Ok(page(items, total, query.limit, |item| item.id))
    }
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

fn title(field: &str, value: &str) -> Result<String, EngineError> {
    reject_nul(field, value)?;
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 200 {
        return Err(invalid(field, "must contain 1 to 200 characters"));
    }
    Ok(value.into())
}

fn validate_work(input: &WorkItemInput) -> Result<String, EngineError> {
    let subject = title("subject", &input.subject)?;
    for at in [input.due_at, input.start_at, input.end_at]
        .into_iter()
        .flatten()
    {
        if !(0..=MAX_TIME).contains(&at) {
            return Err(invalid("time", "timestamp out of range"));
        }
    }
    match input.kind {
        WorkKind::Task
            if input.due_at.is_some() && input.start_at.is_none() && input.end_at.is_none() => {}
        WorkKind::Event
            if input.due_at.is_none()
                && !input.completed
                && matches!((input.start_at,input.end_at),(Some(start),Some(end)) if start<end) => {
        }
        _ => return Err(invalid("kind/time", "invalid task or event fields")),
    }
    Ok(subject)
}

fn validate_opportunity(input: &OpportunityInput) -> Result<String, EngineError> {
    let name = title("name", &input.name)?;
    if !(0..=MAX_AMOUNT_JPY).contains(&input.amount_jpy) {
        return Err(invalid("amount_jpy", "amount out of range"));
    }
    if input.stage > 5 {
        return Err(invalid("stage", "must be between 0 and 5"));
    }
    if !valid_date(&input.close_date) {
        return Err(invalid("close_date", "invalid YYYY-MM-DD date"));
    }
    Ok(name)
}

fn valid_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return false;
    }
    let year = date[..4].parse::<u32>().unwrap_or(0);
    let month = date[5..7].parse::<u32>().unwrap_or(0);
    let day = date[8..].parse::<u32>().unwrap_or(0);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days
}

fn target(
    conn: &Connection,
    company: CompanyId,
    contact: Option<ContactId>,
) -> Result<(), EngineError> {
    let (status, erased): (i64, Option<i64>) = conn
        .query_row(
            "SELECT status,erased_at FROM companies WHERE id=?1",
            [company.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage)?
        .ok_or(EngineError::NotFound)?;
    if erased.is_some() {
        return Err(EngineError::NotFound);
    }
    if status == 5 {
        return Err(EngineError::ExcludedCompany);
    }
    if let Some(contact) = contact {
        let owner: i64 = conn
            .query_row(
                "SELECT company_id FROM contacts WHERE id=?1 AND erased_at IS NULL",
                [contact.0],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?
            .ok_or(EngineError::NotFound)?;
        if owner != company.0 {
            return Err(EngineError::ForeignContact);
        }
    }
    Ok(())
}

fn work_target(
    conn: &Connection,
    id: i64,
) -> Result<(CompanyId, WorkKind, Option<ContactId>), EngineError> {
    let (company,kind,contact): (i64,i64,Option<i64>) = conn.query_row("SELECT company_id,kind,contact_id FROM crm_work_items WHERE id=?1 AND erased_at IS NULL", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))
        .optional().map_err(storage)?.ok_or(EngineError::NotFound)?;
    let kind = match kind {
        1 => WorkKind::Task,
        2 => WorkKind::Event,
        _ => return Err(EngineError::Corrupt),
    };
    Ok((CompanyId(company), kind, contact.map(ContactId)))
}

fn opportunity_target(
    conn: &Connection,
    id: i64,
) -> Result<(CompanyId, Option<ContactId>), EngineError> {
    conn.query_row(
        "SELECT company_id,contact_id FROM crm_opportunities WHERE id=?1 AND erased_at IS NULL",
        [id],
        |r| {
            Ok((
                CompanyId(r.get(0)?),
                r.get::<_, Option<i64>>(1)?.map(ContactId),
            ))
        },
    )
    .optional()
    .map_err(storage)?
    .ok_or(EngineError::NotFound)
}

fn validate_query(
    company: Option<CompanyId>,
    after: Option<i64>,
    limit: u32,
) -> Result<(), EngineError> {
    if !(1..=200).contains(&limit)
        || company.is_some_and(|id| id.0 <= 0)
        || after.is_some_and(|id| id <= 0)
    {
        return Err(invalid("query", "invalid limit or identifier"));
    }
    Ok(())
}

fn work_filter(query: &WorkQuery) -> (String, Vec<Value>) {
    let mut base = "FROM crm_work_items w JOIN companies c ON c.id=w.company_id WHERE w.erased_at IS NULL AND c.erased_at IS NULL AND c.status<>5".to_owned();
    let mut args = Vec::new();
    if let Some(company) = query.company {
        base.push_str(" AND w.company_id=?");
        args.push(Value::Integer(company.0));
    }
    if let Some(kind) = query.kind {
        base.push_str(" AND w.kind=?");
        args.push(Value::Integer(kind.code()));
    }
    if let Some(completed) = query.completed {
        base.push_str(" AND w.completed=?");
        args.push(Value::Integer(i64::from(completed)));
    }
    (base, args)
}

fn opportunity_filter(company: Option<CompanyId>) -> (String, Vec<Value>) {
    let mut base = "FROM crm_opportunities o JOIN companies c ON c.id=o.company_id WHERE o.erased_at IS NULL AND c.erased_at IS NULL AND c.status<>5".to_owned();
    let mut args = Vec::new();
    if let Some(company) = company {
        base.push_str(" AND o.company_id=?");
        args.push(Value::Integer(company.0));
    }
    (base, args)
}

fn total(conn: &Connection, base: &str, args: &[Value]) -> Result<u64, EngineError> {
    let total: i64 = conn
        .query_row(
            &format!("SELECT count(*) {base}"),
            params_from_iter(args),
            |r| r.get(0),
        )
        .map_err(storage)?;
    u64::try_from(total).map_err(|_| EngineError::Corrupt)
}

fn page<T>(mut items: Vec<T>, total: u64, limit: u32, id: impl Fn(&T) -> i64) -> CrmPage<T> {
    let next = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(id)
    } else {
        None
    };
    CrmPage { items, total, next }
}

fn work_row(row: &Row<'_>) -> crate::sql::Result<WorkItem> {
    let kind = match row.get::<_, i64>(4)? {
        1 => WorkKind::Task,
        2 => WorkKind::Event,
        value => return Err(crate::sql::Error::IntegralValueOutOfRange(4, value)),
    };
    Ok(WorkItem {
        id: row.get(0)?,
        company_name: row.get(2)?,
        input: WorkItemInput {
            company: CompanyId(row.get(1)?),
            contact: row.get::<_, Option<i64>>(3)?.map(ContactId),
            kind,
            subject: row.get(5)?,
            due_at: row.get(6)?,
            start_at: row.get(7)?,
            end_at: row.get(8)?,
            completed: row.get(9)?,
        },
    })
}

fn opportunity_row(row: &Row<'_>) -> crate::sql::Result<Opportunity> {
    Ok(Opportunity {
        id: row.get(0)?,
        company_name: row.get(2)?,
        input: OpportunityInput {
            company: CompanyId(row.get(1)?),
            contact: row.get::<_, Option<i64>>(3)?.map(ContactId),
            name: row.get(4)?,
            amount_jpy: row.get(5)?,
            stage: row.get(6)?,
            close_date: row.get(7)?,
        },
    })
}

#[cfg(test)]
#[path = "crm_tests.rs"]
mod tests;
