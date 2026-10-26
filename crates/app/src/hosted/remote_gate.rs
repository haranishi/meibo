//! Persistent service-wide exclusion with explicit, owner-checked recovery.

use std::{error::Error, fmt};

use meibo_db::{Connection, Error as SqlError, params};

use super::{HostedResult, auth, now, remote::store::RemoteStore};

pub(super) const DDL: &str = "CREATE TABLE remote_gate(
    slot INTEGER PRIMARY KEY CHECK(slot=1),
    owner BLOB NOT NULL CHECK(typeof(owner)='blob' AND length(owner)=32),
    at INTEGER NOT NULL,
    operation TEXT NOT NULL
);";
const MAX_OPERATION: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GateError {
    Busy,
    OwnerMismatch,
    PriorWorkNotTerminated,
    InvalidOperation,
    InvalidState,
    UncertainConnection,
}
impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Busy => "保護対象の処理が実行中、または管理者の確認待ちです。",
            Self::OwnerMismatch => "gateの所有情報が一致しません。",
            Self::PriorWorkNotTerminated => "以前の処理の終了を明示してください。",
            Self::InvalidOperation => "gateの操作名を確認してください。",
            Self::InvalidState => "gateの保存形式を確認してください。",
            Self::UncertainConnection => {
                "処理結果を確認できません。再送せず管理者へ連絡してください。"
            }
        })
    }
}
impl Error for GateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GateStatus {
    owner: [u8; 32],
    pub(super) at: i64,
    pub(super) operation: String,
    database_id: String,
    instance: String,
}
impl GateStatus {
    pub(super) fn owner_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.owner {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 15)]));
        }
        output
    }
    fn belongs_to(&self, store: &RemoteStore) -> bool {
        self.database_id == store.database_id() && self.instance == store.instance()
    }
}

/// Owning this value never promises automatic cleanup; dropping it retains the row.
#[derive(Debug)]
pub(super) struct GateGuard {
    status: GateStatus,
}
impl GateGuard {
    pub(super) fn status(&self) -> &GateStatus {
        &self.status
    }
    /// Call only after all protected work and every used connection are known healthy.
    pub(super) fn release(self, store: &RemoteStore) -> HostedResult<()> {
        healthy(store)?;
        if !self.status.belongs_to(store) {
            return Err(GateError::OwnerMismatch.into());
        }
        store.check_ready()?;
        if store.conn.execute(
            "DELETE FROM remote_gate WHERE slot=1 AND owner=?1 AND at=?2 AND operation=?3",
            params![
                self.status.owner.as_slice(),
                self.status.at,
                self.status.operation
            ],
        )? != 1
        {
            return Err(GateError::OwnerMismatch.into());
        }
        Ok(())
    }
}

impl RemoteStore {
    pub(super) fn gate_status(&self) -> HostedResult<Option<GateStatus>> {
        healthy(self)?;
        self.check_ready()?;
        read_status(&self.conn, self)
    }
    pub(super) fn acquire_gate(&self, operation: &str) -> HostedResult<GateGuard> {
        if !valid_operation(operation) {
            return Err(GateError::InvalidOperation.into());
        }
        healthy(self)?;
        self.check_ready()?;
        let mut entropy = [0_u8; 32];
        getrandom::fill(&mut entropy).map_err(|_| "安全な乱数を取得できません。")?;
        let status = GateStatus {
            owner: auth::digest(&entropy)
                .try_into()
                .map_err(|_| GateError::InvalidState)?,
            at: now()?,
            operation: operation.into(),
            database_id: self.database_id().into(),
            instance: self.instance().into(),
        };
        // A single autocommit statement persists ownership before returning it.
        // Any existing row blocks acquisition, including an invalid foreign slot.
        match self.conn.execute(
            "INSERT INTO remote_gate(slot,owner,at,operation) SELECT 1,?1,?2,?3 WHERE NOT EXISTS(SELECT 1 FROM remote_gate)",
            params![status.owner.as_slice(), status.at, status.operation],
        ) {
            Ok(1) => Ok(GateGuard { status }),
            Ok(_) => Err(GateError::Busy.into()),
            Err(SqlError::SqliteFailure(code, _))
                if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY =>
            {
                Err(GateError::Busy.into())
            }
            Err(error) => Err(error.into()),
        }
    }
    pub(super) fn recover_gate(
        &self,
        expected: &GateStatus,
        prior_work_terminated: bool,
    ) -> HostedResult<()> {
        if !prior_work_terminated {
            return Err(GateError::PriorWorkNotTerminated.into());
        }
        healthy(self)?;
        if !expected.belongs_to(self) {
            return Err(GateError::OwnerMismatch.into());
        }
        self.check_ready()?;
        let tx = self.conn.unchecked_transaction()?;
        if read_status(&tx, self)?.as_ref() != Some(expected) {
            audit_recovery(&tx, "owner-mismatch")?;
            tx.commit()?;
            return Err(GateError::OwnerMismatch.into());
        }
        if tx.execute(
            "DELETE FROM remote_gate WHERE slot=1 AND owner=?1 AND at=?2 AND operation=?3",
            params![expected.owner.as_slice(), expected.at, expected.operation],
        )? != 1
        {
            return Err(GateError::OwnerMismatch.into());
        }
        audit_recovery(
            &tx,
            &format!("success:{}:{}", expected.operation, expected.owner_hex()),
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn healthy(store: &RemoteStore) -> HostedResult<()> {
    if store.conn.is_poisoned() {
        return Err(GateError::UncertainConnection.into());
    }
    Ok(())
}

fn audit_recovery(conn: &Connection, outcome: &str) -> HostedResult<()> {
    conn.execute(
        "INSERT INTO audit(at,actor,tenant,operation,outcome) VALUES(?1,'maintenance',NULL,'gate.recover',?2)",
        params![now()?, outcome],
    )?;
    Ok(())
}

fn valid_operation(operation: &str) -> bool {
    (1..=MAX_OPERATION).contains(&operation.len())
        && operation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn read_status(conn: &Connection, store: &RemoteStore) -> HostedResult<Option<GateStatus>> {
    let mut statement =
        conn.prepare("SELECT slot,owner,at,operation FROM remote_gate ORDER BY slot LIMIT 2")?;
    let mut rows = statement.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let slot: i64 = row.get(0)?;
    let owner: Vec<u8> = row.get(1)?;
    let at: i64 = row.get(2)?;
    let operation: String = row.get(3)?;
    if slot != 1 || owner.len() != 32 || !valid_operation(&operation) || rows.next()?.is_some() {
        return Err(GateError::InvalidState.into());
    }
    Ok(Some(GateStatus {
        owner: owner.try_into().map_err(|_| GateError::InvalidState)?,
        at,
        operation,
        database_id: store.database_id().into(),
        instance: store.instance().into(),
    }))
}

#[cfg(all(test, feature = "integration-tests"))]
mod tests;
