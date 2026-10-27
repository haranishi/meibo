//! Shared local/remote authentication records, independent of filesystem paths.
use serde_json::{Value, json};

use super::{
    HostedResult, Role, auth, now,
    sql::{Connection, OptionalExtension, params},
    store::username,
};

pub(super) struct ControlStore {
    pub(super) conn: Connection,
}

impl ControlStore {
    pub(super) fn tenant_exists(&self, slug: &str) -> HostedResult<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tenants WHERE slug=?1)",
            [slug],
            |row| row.get(0),
        )?)
    }
    pub(super) fn add_user(&mut self, name: &str, password: &str) -> HostedResult<()> {
        let name = username(name)?;
        let hash = auth::hash_password(password)?;
        self.conn.execute(
            "INSERT INTO users(username,password_hash) VALUES(?1,?2)",
            params![name, hash],
        )?;
        self.audit("maintenance", None, "user.add", "success")
    }

    pub(super) fn set_password(&mut self, name: &str, password: &str) -> HostedResult<()> {
        let name = username(name)?;
        let hash = auth::hash_password(password)?;
        let tx = self.conn.transaction()?;
        if tx.execute(
            "UPDATE users SET password_hash=?2 WHERE username=?1",
            params![name, hash],
        )? != 1
        {
            return Err("利用者が見つかりません。".into());
        }
        tx.execute(
            "DELETE FROM sessions WHERE user_id=(SELECT id FROM users WHERE username=?1)",
            [name],
        )?;
        tx.commit()?;
        self.audit("maintenance", None, "password.change", "success")
    }

    pub(super) fn disable_user(&mut self, name: &str) -> HostedResult<()> {
        let name = username(name)?;
        let tx = self.conn.transaction()?;
        if tx.execute("UPDATE users SET disabled=1 WHERE username=?1", [&name])? != 1 {
            return Err("利用者が見つかりません。".into());
        }
        tx.execute(
            "DELETE FROM sessions WHERE user_id=(SELECT id FROM users WHERE username=?1)",
            [name],
        )?;
        tx.commit()?;
        self.audit("maintenance", None, "user.disable", "success")
    }

    pub(super) fn set_member_record(
        &mut self,
        name: &str,
        tenant: &str,
        role: Option<Role>,
    ) -> HostedResult<()> {
        let name = username(name)?;
        if !self.tenant_exists(tenant)? {
            return Err("組織が見つかりません。".into());
        }
        let user: i64 = self
            .conn
            .query_row(
                "SELECT id FROM users WHERE username=?1 AND disabled=0",
                [name],
                |r| r.get(0),
            )
            .optional()?
            .ok_or("利用者が見つかりません。")?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM sessions WHERE user_id=?1 AND tenant=?2",
            params![user, tenant],
        )?;
        if let Some(role) = role {
            tx.execute("INSERT INTO memberships(user_id,tenant,role) VALUES(?1,?2,?3) ON CONFLICT(user_id,tenant) DO UPDATE SET role=excluded.role",params![user,tenant,role.name()])?;
        } else {
            tx.execute(
                "DELETE FROM memberships WHERE user_id=?1 AND tenant=?2",
                params![user, tenant],
            )?;
        }
        tx.commit()?;
        self.audit("maintenance", Some(tenant), "membership.change", "success")
    }

    pub(super) fn revoke_user(&self, name: &str) -> HostedResult<()> {
        let name = username(name)?;
        self.conn.execute(
            "DELETE FROM sessions WHERE user_id=(SELECT id FROM users WHERE username=?1)",
            [name],
        )?;
        self.audit("maintenance", None, "session.revoke", "success")
    }

    pub(super) fn audit(
        &self,
        actor: &str,
        tenant: Option<&str>,
        operation: &str,
        outcome: &str,
    ) -> HostedResult<()> {
        self.conn.execute(
            "INSERT INTO audit(at,actor,tenant,operation,outcome) VALUES(?1,?2,?3,?4,?5)",
            params![now()?, actor, tenant, operation, outcome],
        )?;
        Ok(())
    }

    pub(super) fn audit_rows(&self, tenant: Option<&str>, limit: u32) -> HostedResult<Vec<Value>> {
        let mut stmt=self.conn.prepare("SELECT id,at,actor,tenant,operation,outcome FROM audit WHERE (?1 IS NULL OR tenant=?1) ORDER BY id DESC LIMIT ?2")?;
        Ok(stmt.query_map(params![tenant,limit.min(200)],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"at":r.get::<_,i64>(1)?,"actor":r.get::<_,String>(2)?,"tenant":r.get::<_,Option<String>>(3)?,"operation":r.get::<_,String>(4)?,"outcome":r.get::<_,String>(5)?})))?.collect::<std::result::Result<_,_>>()?)
    }
}
