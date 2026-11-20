//! OAuth credentials and provider cursors live on a private volume, outside CRM backups.
use super::{Failure, Result, now, value};
use meibo_core::{Engine, activity_sync::Item};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{fs, path::Path};

const DDL:&str="
PRAGMA secure_delete=ON;
CREATE TABLE IF NOT EXISTS pending(scope TEXT,provider TEXT,nonce TEXT NOT NULL,verifier TEXT NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(scope,provider));
CREATE TABLE IF NOT EXISTS grants(scope TEXT,provider TEXT,account TEXT NOT NULL,address TEXT NOT NULL,refresh TEXT NOT NULL,access TEXT NOT NULL,
 expires INTEGER NOT NULL,since INTEGER NOT NULL,cursors TEXT NOT NULL,mask INTEGER NOT NULL DEFAULT 0,turn INTEGER NOT NULL DEFAULT 0,
 last INTEGER,reauth INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(scope,provider));
CREATE TABLE IF NOT EXISTS tracked(scope TEXT,provider TEXT,event TEXT,epoch TEXT NOT NULL,PRIMARY KEY(scope,provider,event));";

pub(super) fn open(dir: &Path) -> Result<Connection> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let meta = fs::symlink_metadata(dir).map_err(|_| Failure::Setup)?;
        if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
            return Err(Failure::Setup);
        }
        let file = dir.join("connections.sqlite");
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(Failure::Storage),
        }
        let meta = fs::symlink_metadata(&file).map_err(|_| Failure::Setup)?;
        if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
            return Err(Failure::Setup);
        }
        let conn = Connection::open(file)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.execute_batch(DDL)?;
        Ok(conn)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Err(Failure::Setup)
    }
}

pub(super) fn status(conn: &Connection, scope: &str, provider: &str) -> Result<Value> {
    Ok(conn
        .query_row(
            "SELECT address,last,reauth FROM grants WHERE scope=?1 AND provider=?2",
            (scope, provider),
            |r| {
                Ok(
                    json!({"configured":true,"connected":true,"account":r.get::<_,String>(0)?,
            "last_sync":r.get::<_,Option<i64>>(1)?,"reauth":r.get::<_,bool>(2)?}),
                )
            },
        )
        .optional()?
        .unwrap_or_else(|| json!({"configured":true,"connected":false})))
}

type Worker<'a> = dyn Fn(Value) -> Result<Value> + 'a;
pub(super) fn apply(
    conn: &mut Connection,
    engine: &mut Engine,
    scope: &str,
    provider: &str,
    action: &str,
    body: &Value,
    worker: impl Fn(Value) -> Result<Value>,
) -> Result<Value> {
    if action == "pull" {
        refresh(conn, scope, provider, &worker)?;
        return page(conn, engine, scope, provider, &worker);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = match action {
        "begin" => {
            let out = worker(json!({"operation":"begin"}))?;
            tx.execute("INSERT INTO pending VALUES(?1,?2,?3,?4,?5) ON CONFLICT(scope,provider) DO UPDATE SET nonce=excluded.nonce,verifier=excluded.verifier,expires=excluded.expires",
                params![scope,provider,value(&out,"state")?,value(&out,"verifier")?,now()+600])?;
            json!({"url":value(&out,"url")?,"state":value(&out,"state")?})
        }
        "finish" => return finish(tx, scope, provider, body, &worker),
        "disconnect" => {
            for sql in [
                "DELETE FROM pending WHERE scope=?1 AND provider=?2",
                "DELETE FROM tracked WHERE scope=?1 AND provider=?2",
                "DELETE FROM grants WHERE scope=?1 AND provider=?2",
            ] {
                tx.execute(sql, (scope, provider))?;
            }
            json!({"connected":false})
        }
        _ => return Err(Failure::Input),
    };
    tx.commit()?;
    Ok(result)
}

fn finish(
    tx: Transaction<'_>,
    scope: &str,
    provider: &str,
    body: &Value,
    worker: &Worker<'_>,
) -> Result<Value> {
    let state = body["state"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or(Failure::Input)?;
    let code = body["code"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 8192)
        .ok_or(Failure::Input)?;
    let verifier:String=tx.query_row("SELECT verifier FROM pending WHERE scope=?1 AND provider=?2 AND nonce=?3 AND expires>?4",
        params![scope,provider,state,now()],|r|r.get(0)).optional()?.ok_or(Failure::Stale)?;
    tx.execute(
        "DELETE FROM pending WHERE scope=?1 AND provider=?2",
        (scope, provider),
    )?;
    let since = now() * 1000;
    let out = match worker(json!({"operation":"finish","verifier":verifier,"code":code})) {
        Ok(v) => v,
        Err(e) => {
            tx.commit()?;
            return Err(e);
        }
    };
    let account = value(&out, "account")?;
    let old: Option<String> = tx
        .query_row(
            "SELECT account FROM grants WHERE scope=?1 AND provider=?2",
            (scope, provider),
            |r| r.get(0),
        )
        .optional()?;
    if old.as_deref().is_some_and(|s| s != account) {
        tx.commit()?;
        return Err(Failure::Stale);
    }
    tx.execute("INSERT INTO grants(scope,provider,account,address,refresh,access,expires,since,cursors) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
       ON CONFLICT(scope,provider) DO UPDATE SET refresh=excluded.refresh,access=excluded.access,expires=excluded.expires,address=excluded.address,reauth=0",
       params![scope,provider,account,out["address"].as_str().unwrap_or(""),value(&out,"refresh")?,value(&out,"access")?,
          out["expires"].as_i64().ok_or(Failure::Unavailable)?,since,json!({"inbox":out["initial"]}).to_string()])?;
    tx.commit()?;
    Ok(json!({"connected":true}))
}

fn refresh(conn: &mut Connection, scope: &str, provider: &str, worker: &Worker<'_>) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (secret, expires, reauth): (String, i64, bool) = tx
        .query_row(
            "SELECT refresh,expires,reauth FROM grants WHERE scope=?1 AND provider=?2",
            (scope, provider),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(Failure::Stale)?;
    if reauth {
        return Err(Failure::Consent);
    }
    if expires > now() * 1000 {
        return Ok(());
    }
    let out = match worker(json!({"operation":"refresh","refresh":secret})) {
        Ok(v) => v,
        Err(Failure::Consent) => {
            tx.execute(
                "UPDATE grants SET reauth=1,access='',expires=0 WHERE scope=?1 AND provider=?2",
                (scope, provider),
            )?;
            tx.commit()?;
            return Err(Failure::Consent);
        }
        Err(e) => return Err(e),
    };
    tx.execute(
        "UPDATE grants SET refresh=?3,access=?4,expires=?5 WHERE scope=?1 AND provider=?2",
        params![
            scope,
            provider,
            value(&out, "refresh")?,
            value(&out, "access")?,
            out["expires"].as_i64().ok_or(Failure::Unavailable)?
        ],
    )?;
    // A rotated credential is durable even if the following data request/CRM transaction fails.
    tx.commit()?;
    Ok(())
}

struct Grant {
    account: String,
    address: String,
    access: String,
    since: i64,
    cursors: String,
    mask: u8,
    turn: u8,
}
fn page(
    conn: &mut Connection,
    engine: &mut Engine,
    scope: &str,
    provider: &str,
    worker: &Worker<'_>,
) -> Result<Value> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let grant=tx.query_row("SELECT account,address,access,since,cursors,mask,turn FROM grants WHERE scope=?1 AND provider=?2",
        (scope,provider),|r|Ok(Grant{account:r.get(0)?,address:r.get(1)?,access:r.get(2)?,since:r.get(3)?,cursors:r.get(4)?,mask:r.get(5)?,turn:r.get(6)?})).optional()?.ok_or(Failure::Stale)?;
    let streams: &[&str] = if provider == "google" {
        &["inbox", "calendar"]
    } else {
        &["inbox", "sent", "calendar"]
    };
    let mut mask = if grant.mask == 0 {
        (1_u8 << streams.len()) - 1
    } else {
        grant.mask
    };
    let idx = (0..streams.len())
        .map(|i| (i + usize::from(grant.turn)) % streams.len())
        .find(|i| mask & (1 << i) != 0)
        .ok_or(Failure::Storage)?;
    let stream = streams[idx];
    let mut cursors: Value = serde_json::from_str(&grant.cursors).map_err(|_| Failure::Storage)?;
    let cursor = cursors.get(stream).cloned().unwrap_or_else(|| json!({}));
    let provider_page = if let Some(epoch) = cursor.get("reconcile_epoch") {
        Ok(json!({"items":[],"reconcile":true,"epoch":epoch,"cursor":cursor["next"],"more":false}))
    } else {
        worker(
            json!({"operation":"page","stream":stream,"access":grant.access,"since":grant.since,
            "address":grant.address,"cursor":cursor}),
        )
    };
    let out = match provider_page {
        Ok(out) => out,
        Err(Failure::Consent) => {
            tx.execute(
                "UPDATE grants SET expires=0 WHERE scope=?1 AND provider=?2",
                (scope, provider),
            )?;
            tx.commit()?;
            return Err(Failure::Unavailable);
        }
        Err(e) => return Err(e),
    };
    let items: Vec<Item> =
        serde_json::from_value(out["items"].clone()).map_err(|_| Failure::Unavailable)?;
    if !out["cursor"].is_object() {
        return Err(Failure::Unavailable);
    }
    let mut next = out["cursor"].clone();
    let mut more = out["more"].as_bool().ok_or(Failure::Unavailable)?;
    let mut changes = engine
        .sync_activity_page(provider, &grant.account, &items)
        .map_err(|_| Failure::Storage)?;
    if let Some(epoch) = out["epoch"].as_str() {
        for event in &changes.tracked_events {
            tx.execute("INSERT INTO tracked VALUES(?1,?2,?3,?4) ON CONFLICT(scope,provider,event) DO UPDATE SET epoch=excluded.epoch",
                (scope,provider,event,epoch))?;
        }
        if out["reconcile"] == true {
            let (removed, remaining) =
                reconcile(&tx, engine, scope, provider, &grant.account, epoch)?;
            changes.updated += removed;
            if remaining {
                next = json!({"reconcile_epoch":epoch,"next":next});
                more = true;
            }
        }
    }
    cursors[stream] = next;
    if !more {
        mask &= !(1 << idx);
    }
    tx.execute("UPDATE grants SET cursors=?3,turn=?4,mask=?5,last=CASE WHEN ?5=0 THEN ?6 ELSE last END WHERE scope=?1 AND provider=?2",
        params![scope,provider,cursors.to_string(),u8::try_from((idx+1)%streams.len()).map_err(|_|Failure::Storage)?,mask,now()])?;
    tx.commit()?;
    Ok(json!({"more":mask!=0,"changes":changes}))
}

fn reconcile(
    tx: &Transaction<'_>,
    engine: &mut Engine,
    scope: &str,
    provider: &str,
    account: &str,
    epoch: &str,
) -> Result<(usize, bool)> {
    let mut stmt = tx.prepare(
        "SELECT event FROM tracked WHERE scope=?1 AND provider=?2 AND epoch<>?3 LIMIT 51",
    )?;
    let mut ids: Vec<String> = stmt
        .query_map((scope, provider, epoch), |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if ids.is_empty() {
        return Ok((0, false));
    }
    let remaining = ids.len() > 50;
    ids.truncate(50);
    let events: Vec<Item> = serde_json::from_value(json!(
        ids.iter()
            .map(|id| json!({"kind":"removed_event","id":id}))
            .collect::<Vec<_>>()
    ))
    .map_err(|_| Failure::Storage)?;
    let total = engine
        .sync_activity_page(provider, account, &events)
        .map_err(|_| Failure::Storage)?
        .updated;
    for id in ids {
        tx.execute(
            "DELETE FROM tracked WHERE scope=?1 AND provider=?2 AND event=?3",
            (scope, provider, id),
        )?;
    }
    Ok((total, remaining))
}
