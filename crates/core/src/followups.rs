//! Absence callbacks linked to the originating activity and the existing task list.
use crate::{
    Engine, crm,
    query::storage,
    schema::WRITE_BASE,
    sql::{OptionalExtension, params},
    types::{ActivityId, ActivityInput, ActivityKind, EngineError, NextAction, Status},
    write::{activity_estimate, now_secs, record_activity_tx},
};
use serde::Serialize;

const DAY: i64 = 86_400;
const JST: i64 = 32_400;
const MAX_TIME: i64 = 253_402_300_799;

fn invalid() -> EngineError {
    EngineError::InvalidInput {
        field: "follow_up".into(),
        reason: "invalid callback request".into(),
    }
}

fn next_business_morning(at: i64) -> Result<i64, EngineError> {
    if !(0..=MAX_TIME).contains(&at) {
        return Err(invalid());
    }
    let mut day = (at + JST).div_euclid(DAY) + 1;
    // 1970-01-01 was Thursday. 09:00 JST falls at 00:00 UTC on the same day.
    while (day + 3).rem_euclid(7) >= 5 {
        day += 1;
    }
    let due = day * DAY;
    if due > MAX_TIME {
        return Err(invalid());
    }
    Ok(due)
}

/// A completed callback must not leave a stale date in the existing next-action list.
/// A later manual appointment, even for the same date, still belongs to its author.
pub(crate) fn after_completion(
    tx: &crate::sql::Transaction<'_>,
    work: i64,
    completed: bool,
) -> Result<(), EngineError> {
    if !completed || crm::version(tx)? < 3 {
        return Ok(());
    }
    let company: Option<i64> = tx.query_row(
        "SELECT w.company_id FROM crm_call_followups f JOIN crm_work_items w ON w.id=f.work_id
         WHERE f.work_id=?1 AND COALESCE(f.scheduled_by,f.activity_id)=(SELECT a.id FROM activities a WHERE a.company_id=w.company_id
         AND a.next_action_kind<>0 AND NOT EXISTS(SELECT 1 FROM activities r WHERE r.reverses_id=a.id) ORDER BY a.id DESC LIMIT 1)",
        [work], |r| r.get(0)).optional().map_err(storage)?;
    if let Some(company) = company {
        let next:Option<(i64,i64)>=tx.query_row("SELECT w.id,w.due_at FROM crm_work_items w JOIN crm_call_followups f ON f.work_id=w.id
            WHERE w.company_id=?1 AND w.completed=0 AND w.erased_at IS NULL ORDER BY w.due_at,w.id LIMIT 1",
            [company],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?;
        let action = ActivityInput {
            company: crate::types::CompanyId(company),
            contact: None,
            kind: ActivityKind::Memo,
            at: now_secs(),
            note: Some("再架電を完了".into()),
            status_after: None,
            next_action: next.map_or(NextAction::Clear, |(_, due)| NextAction::Set(due)),
            reverses: None,
        };
        let change = record_activity_tx(tx, &action, false)?;
        if let Some((id, _)) = next {
            tx.execute(
                "UPDATE crm_call_followups SET scheduled_by=?1 WHERE work_id=?2",
                (change.0, id),
            )
            .map_err(storage)?;
        }
    }
    Ok(())
}

/// One outstanding callback, with the same company masking as the CRM list.
#[derive(Serialize)]
pub struct FollowUp {
    /// Existing CRM task ID.
    pub work_id: i64,
    /// Owning company.
    pub company_id: i64,
    /// Visible company name where permitted.
    pub company_name: Option<String>,
    /// Assigned sales user, or unassigned in a local database.
    pub owner_id: Option<i64>,
    /// UTC seconds of the agreed callback.
    pub due_at: i64,
}

/// Notification count and its earliest entries.
#[derive(Serialize, Default)]
pub struct DueFollowUps {
    /// Full number of overdue or due-today callbacks.
    pub total: i64,
    /// Earliest 50 callbacks, ordered by deadline and ID.
    pub items: Vec<FollowUp>,
}

impl Engine {
    /// Record an absent call, its next action and its task in one transaction.
    /// An identical retry returns the previously recorded IDs.
    /// # Errors
    /// Invalid or erased targets, conflicting submission, invalid dates, or storage errors.
    pub fn crm_record_absence(
        &mut self,
        input: &ActivityInput,
        actor: Option<i64>,
        request_key: &str,
        fingerprint: &str,
    ) -> Result<(ActivityId, i64, i64), EngineError> {
        if input.kind != ActivityKind::Call
            || input.status_after == Some(Status::Excluded)
            || input.next_action == NextAction::Clear
            || !(16..=80).contains(&request_key.len())
            || !request_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || fingerprint.len() != 64
            || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
            || actor.is_some_and(|id| id <= 0)
        {
            return Err(invalid());
        }
        self.db.write(WRITE_BASE + activity_estimate(input), |tx| {
            crm::ensure_sales_ops(tx)?;
            let existing = tx.query_row(
                "SELECT f.activity_id,f.work_id,w.due_at,f.fingerprint,a.company_id FROM crm_call_followups f
                 JOIN activities a ON a.id=f.activity_id JOIN crm_work_items w ON w.id=f.work_id WHERE f.request_key=?1",
                [request_key], |r| Ok((r.get::<_,i64>(0)?,r.get(1)?,r.get(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?)))
                .optional().map_err(storage)?;
            if let Some((id,work,due,stored,company)) = existing {
                if stored != fingerprint || company != input.company.0 { return Err(EngineError::RevisionConflict); }
                return Ok((ActivityId(id),work,due));
            }
            let owner: Option<i64> = tx.query_row(
                "SELECT owner_id FROM companies WHERE id=? AND erased_at IS NULL AND status<>5", [input.company.0], |r| r.get(0))
                .optional().map_err(storage)?.ok_or(EngineError::NotFound)?;
            if let Some(contact) = input.contact {
                let active: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM contacts WHERE id=?1 AND company_id=?2 AND erased_at IS NULL)",
                    (contact.0,input.company.0),|r| r.get(0)).map_err(storage)?;
                if !active {return Err(EngineError::ForeignContact);}
            }
            let due = match input.next_action {
                NextAction::Set(t) if (0..=MAX_TIME).contains(&t) => t,
                NextAction::Keep => next_business_morning(input.at)?,
                _ => return Err(invalid()),
            };
            let call = ActivityInput { next_action: NextAction::Set(due), ..input.clone() };
            let id = record_activity_tx(tx,&call,false)?;
            let now=now_secs();
            tx.execute("INSERT INTO crm_work_items(company_id,contact_id,kind,subject,due_at,completed,created_at,updated_at)
                        VALUES(?1,?2,1,'不在のため再架電',?3,0,?4,?4)",params![input.company.0,input.contact.map(|c|c.0),due,now]).map_err(storage)?;
            let work = tx.last_insert_rowid();
            tx.execute("INSERT INTO crm_call_followups(activity_id,work_id,owner_id,request_key,fingerprint) VALUES(?1,?2,?3,?4,?5)",
                params![id.0,work,actor.or(owner),request_key,fingerprint]).map_err(storage)?;
            Ok((id,work,due))
        })
    }

    /// Read open callbacks due by the end of the given Japanese business day.
    /// # Errors
    /// Invalid timestamp or storage error.
    pub fn crm_due_followups(
        &self,
        actor: Option<i64>,
        now: i64,
    ) -> Result<DueFollowUps, EngineError> {
        if !(0..=MAX_TIME).contains(&now) {
            return Err(invalid());
        }
        let end = ((now + JST).div_euclid(DAY) + 1) * DAY - JST;
        let tx = self.db.conn().unchecked_transaction().map_err(storage)?;
        if crm::version(&tx)? < 3 {
            return Ok(DueFollowUps::default());
        }
        let filter="FROM crm_call_followups f JOIN crm_work_items w ON w.id=f.work_id JOIN companies c ON c.id=w.company_id
            WHERE w.erased_at IS NULL AND w.completed=0 AND c.erased_at IS NULL AND c.status<>5 AND w.due_at<?1 AND (?2 IS NULL OR f.owner_id=?2)";
        let total = tx
            .query_row(
                &format!("SELECT COUNT(*) {filter}"),
                params![end, actor],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let items=tx.prepare(&format!("SELECT w.id,c.id,CASE WHEN c.is_sole_proprietor=0 THEN c.name END,f.owner_id,w.due_at
                {filter} ORDER BY w.due_at,w.id LIMIT 50")).map_err(storage)?
            .query_map(params![end,actor],|r|Ok(FollowUp{work_id:r.get(0)?,company_id:r.get(1)?,company_name:r.get(2)?,owner_id:r.get(3)?,due_at:r.get(4)?}))
            .map_err(storage)?.collect::<Result<Vec<_>,_>>().map_err(storage)?;
        tx.rollback().map_err(storage)?;
        Ok(DueFollowUps { total, items })
    }

    /// Sales identities already referenced by this single-user local database.
    /// # Errors
    /// The database cannot be read.
    pub fn crm_local_owner_ids(&self) -> Result<Vec<i64>, EngineError> {
        let conn = self.db.conn();
        let mut owners =
            "SELECT owner_id FROM companies WHERE erased_at IS NULL AND status<>5 AND owner_id>0"
                .to_owned();
        if crm::version(conn)? >= 3 {
            owners.push_str(" UNION SELECT owner_id FROM crm_opportunity_ownership WHERE owner_id>0
                UNION SELECT owner_id FROM crm_sales_targets UNION SELECT owner_id FROM crm_leads WHERE state=0 AND owner_id>0");
        }
        conn.prepare(&format!(
            "SELECT DISTINCT owner_id FROM ({owners}) ORDER BY owner_id"
        ))
        .map_err(storage)?
        .query_map([], |r| r.get(0))
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ 20.1
    #[test]
    fn business_days_cross_weekends_and_japanese_midnight() {
        // 1970-01-01 Thu local. Each expected value is 09:00 Japan time.
        for (at, due) in [
            (0, DAY),
            (DAY - 1, 4 * DAY),
            (DAY, 4 * DAY),
            (2 * DAY, 4 * DAY),
            (3 * DAY, 4 * DAY),
            (4 * DAY, 5 * DAY),
            (15 * 3600 - 1, DAY),
            (15 * 3600, 4 * DAY),
        ] {
            assert_eq!(next_business_morning(at).unwrap(), due);
        }
        assert!(next_business_morning(MAX_TIME).is_err());
        assert!(next_business_morning(-1).is_err());
    }
}
