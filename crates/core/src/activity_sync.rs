//! Project a provider page onto existing business contacts without storing mailbox identifiers.
use crate::{
    Engine, crm,
    schema::WRITE_BASE,
    sql::{Connection, OptionalExtension, Transaction, params, params_from_iter},
    types::{ActivityInput, ActivityKind, CompanyId, ContactId, EngineError, NextAction},
    write::{now_secs, record_activity_tx},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Metadata emitted by the trusted provider adapter. Removed or private events carry only their ID.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    id: String,
    kind: Kind,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    addresses: Vec<String>,
    #[serde(default)]
    at: i64,
    end: Option<i64>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Mail,
    Event,
    RemovedEvent,
}

/// Changes visible in the CRM after committing one page.
#[derive(Default, Serialize)]
pub struct Changes {
    /// New matched mail or events.
    pub created: usize,
    /// Changed or cancelled matched events.
    pub updated: usize,
    /// Unmatched, ambiguous, or unchanged items.
    pub skipped: usize,
    /// Matched events to reconcile if a provider invalidates its differential cursor.
    #[serde(skip)]
    pub tracked_events: Vec<String>,
}

fn invalid() -> EngineError {
    EngineError::InvalidInput {
        field: "sync".into(),
        reason: "invalid provider page".into(),
    }
}

fn storage(e: crate::sql::Error) -> EngineError {
    EngineError::Storage(Box::new(e))
}

fn source_key(provider: &str, mailbox: &str, item: &Item) -> String {
    let mut hash = Sha256::new();
    for part in [
        provider,
        mailbox,
        if item.kind == Kind::Mail {
            "mail"
        } else {
            "event"
        },
        &item.id,
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn contact(conn: &Connection, addresses: &[String]) -> Result<Option<(i64, i64)>, EngineError> {
    if addresses.is_empty() {
        return Ok(None);
    }
    let slots = vec!["?"; addresses.len()].join(",");
    let sql = format!(
        "SELECT c.company_id,c.id FROM contacts c JOIN companies a ON a.id=c.company_id
        WHERE c.erased_at IS NULL AND a.erased_at IS NULL AND a.status<>5 AND a.is_sole_proprietor=0
        AND lower(trim(c.email)) IN ({slots}) LIMIT 2"
    );
    let mut statement = conn.prepare(&sql).map_err(storage)?;
    let mut rows = statement
        .query(params_from_iter(
            addresses
                .iter()
                .map(|address| address.trim().to_ascii_lowercase()),
        ))
        .map_err(storage)?;
    let first = rows
        .next()
        .map_err(storage)?
        .map(|r| Ok((r.get(0)?, r.get(1)?)))
        .transpose()
        .map_err(storage)?;
    Ok(if rows.next().map_err(storage)?.is_some() {
        None
    } else {
        first
    })
}

impl Engine {
    /// Bind optional local connector credentials to this particular database, including after replacing a file at the same path.
    /// # Errors
    /// Returns a storage error if the local database identity cannot be read or initialized.
    pub fn sync_local_identity(&mut self, initialize: bool) -> Result<Option<String>, EngineError> {
        if self.path().is_none() {
            return Err(invalid());
        }
        let read = |conn: &Connection| {
            conn.query_row(
                "SELECT value FROM meta WHERE key='sync_local_identity'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)
        };
        let current = read(self.db.conn())?;
        if current.is_some() || !initialize {
            return Ok(current);
        }
        self.db.write(WRITE_BASE, |tx| {
            tx.execute("INSERT OR IGNORE INTO meta(key,value) VALUES('sync_local_identity',lower(hex(randomblob(32))))",[]).map_err(storage)?;
            read(tx)
        })
    }

    /// Commit a page and its deduplication links together. The caller advances its private cursor after success.
    /// # Errors
    /// Rejects invalid provider metadata and propagates database errors.
    pub fn sync_activity_page(
        &mut self,
        provider: &str,
        mailbox: &str,
        items: &[Item],
    ) -> Result<Changes, EngineError> {
        if !matches!(provider, "google" | "microsoft")
            || mailbox.is_empty()
            || mailbox.len() > 1024
            || items.len() > 100
        {
            return Err(invalid());
        }
        for item in items {
            if item.id.is_empty()
                || item.id.len() > 2048
                || item.subject.chars().count() > 200
                || item.subject.contains('\0')
                || item.addresses.len() > 256
                || item
                    .addresses
                    .iter()
                    .any(|s| s.len() > 320 || s.trim().is_empty())
                || !(0..=253_402_300_799).contains(&item.at)
                || item.end.is_some_and(|end| end > 253_402_300_799)
                || (item.kind == Kind::Event && item.end.is_none_or(|end| end <= item.at))
            {
                return Err(invalid());
            }
        }
        self.db
            .write(WRITE_BASE.saturating_mul(items.len() as u64 + 1), |tx| {
                crm::ensure_sales_ops(tx)?;
                let mut changes = Changes::default();
                for item in items {
                    let key = source_key(provider, mailbox, item);
                    project(tx, &key, item, &mut changes)?;
                }
                Ok(changes)
            })
    }
}

fn project(
    tx: &Transaction<'_>,
    key: &str,
    item: &Item,
    tally: &mut Changes,
) -> Result<(), EngineError> {
    let link: Option<Option<i64>> = tx
        .query_row(
            "SELECT work_item_id FROM crm_sync_links WHERE source_key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if item.kind == Kind::Mail && link.is_some() {
        tally.skipped += 1;
        return Ok(());
    }
    let matched = if item.kind == Kind::RemovedEvent {
        None
    } else {
        contact(tx, &item.addresses)?
    };
    let prior = link.flatten();
    match (matched, prior) {
        (Some((company, person)), Some(id)) => {
            let rows = tx.execute(
                "UPDATE crm_work_items SET company_id=?2,contact_id=?3,subject=?4,start_at=?5,end_at=?6,updated_at=?7,erased_at=NULL
                 WHERE id=?1 AND contact_id IS NOT NULL AND (company_id<>?2 OR contact_id<>?3 OR subject<>?4 OR start_at<>?5 OR end_at<>?6 OR erased_at IS NOT NULL)",
                params![id,company,person,item.title(),item.at,item.end,now_secs()]
            ).map_err(storage)?;
            // Erasure clears contact_id permanently. A matching email may later be reused by someone else.
            tally.updated += rows;
            tally.skipped += usize::from(rows == 0);
        }
        (Some((company, person)), None) => {
            create(tx, key, item, company, person, item.title())?;
            tally.created += 1;
        }
        (None, Some(id)) => {
            let rows = tx.execute(
                "UPDATE crm_work_items SET subject='',erased_at=?2,updated_at=?2 WHERE id=?1 AND erased_at IS NULL",
                params![id, now_secs()],
            ).map_err(storage)?;
            tally.updated += rows;
            tally.skipped += usize::from(rows == 0);
        }
        (None, None) => tally.skipped += 1,
    }
    if item.kind == Kind::Event && (matched.is_some() || prior.is_some()) {
        tally.tracked_events.push(item.id.clone());
    }
    Ok(())
}

impl Item {
    fn title(&self) -> &str {
        let title = self.subject.trim();
        if title.is_empty() {
            "（件名なし）"
        } else {
            title
        }
    }
}

fn create(
    tx: &Transaction<'_>,
    key: &str,
    item: &Item,
    company: i64,
    contact: i64,
    subject: &str,
) -> Result<(), EngineError> {
    let (mail, event) = if item.kind == Kind::Mail {
        let id = record_activity_tx(
            tx,
            &ActivityInput {
                company: CompanyId(company),
                contact: Some(ContactId(contact)),
                kind: ActivityKind::Email,
                at: item.at,
                note: Some(subject.into()),
                status_after: None,
                next_action: NextAction::Keep,
                reverses: None,
            },
            false,
        )?;
        (Some(id.0), None)
    } else {
        tx.execute("INSERT INTO crm_work_items(company_id,contact_id,kind,subject,start_at,end_at,completed,created_at,updated_at)
            VALUES(?1,?2,2,?3,?4,?5,0,?6,?6)",params![company,contact,subject,item.at,item.end,now_secs()]).map_err(storage)?;
        (None, Some(tx.last_insert_rowid()))
    };
    tx.execute(
        "INSERT INTO crm_sync_links(source_key,activity_id,work_item_id) VALUES(?1,?2,?3)",
        params![key, mail, event],
    )
    .map_err(storage)?;
    Ok(())
}
