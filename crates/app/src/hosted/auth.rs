use super::sql::{OptionalExtension, params};
use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use sha2::{Digest, Sha256};

use super::{HostedResult, control::ControlStore, store};

const ABSOLUTE: i64 = 8 * 3600;
const IDLE: i64 = 30 * 60;
const WINDOW: i64 = 15 * 60;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Viewer,
    Editor,
    Admin,
}
impl Role {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "viewer" => Some(Self::Viewer),
            "editor" => Some(Self::Editor),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::Admin => "admin",
        }
    }
}

pub(super) struct Identity {
    pub(super) user_id: i64,
    pub(super) username: String,
    pub(super) tenant: String,
    pub(super) role: Role,
    pub(super) scope: String,
}
pub(super) struct Login {
    pub(super) token: String,
    pub(super) identity: Identity,
}
pub(super) enum LoginOutcome {
    Success(Login),
    Invalid,
    Throttled,
}

pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(value, "{byte:02x}").expect("String write cannot fail");
    }
    value
}

pub(super) fn random_hex(bytes: usize) -> HostedResult<String> {
    let mut value = vec![0; bytes];
    getrandom::fill(&mut value).map_err(|_| "安全な乱数を取得できません。")?;
    Ok(hex(&value))
}

pub(super) fn digest(value: &[u8]) -> Vec<u8> {
    Sha256::digest(value).to_vec()
}

pub(super) fn hash_password(password: &str) -> HostedResult<String> {
    if !(12..=1024).contains(&password.len()) || password.contains('\0') {
        return Err("passwordは12〜1024bytesで指定してください。".into());
    }
    let mut salt = [0_u8; 16];
    getrandom::fill(&mut salt).map_err(|_| "安全な乱数を取得できません。")?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| "passwordを保護できません。")?;
    let params = Params::new(19456, 2, 1, None).map_err(|_| "password設定が正しくありません。")?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "passwordを保護できません。".into())
}

impl ControlStore {
    fn identity(
        &self,
        user_id: i64,
        name: String,
        tenant: String,
        role: &str,
    ) -> HostedResult<Identity> {
        let instance: String =
            self.conn
                .query_row("SELECT value FROM meta WHERE key='instance'", [], |r| {
                    r.get(0)
                })?;
        let scope = hex(&digest(format!("{instance}:{user_id}:{tenant}").as_bytes()));
        Ok(Identity {
            user_id,
            username: name,
            tenant,
            role: Role::parse(role).ok_or("保存された権限が正しくありません。")?,
            scope,
        })
    }

    pub(super) fn login(
        &mut self,
        tenant: &str,
        name: &str,
        password: &str,
        at: i64,
    ) -> HostedResult<LoginOutcome> {
        if password.len() > 1024 || name.len() > 128 || tenant.len() > 48 {
            return Ok(LoginOutcome::Invalid);
        }
        let name = store::username(name).unwrap_or_default();
        let account = hex(&digest(name.as_bytes()));
        self.conn.execute(
            "DELETE FROM login_attempts WHERE window_start<=?1",
            [at - WINDOW],
        )?;
        for (bucket, limit) in [(account.as_str(), 5), ("global", 100)] {
            let count: i64 = self.conn.query_row(
                "SELECT COALESCE((SELECT failures FROM login_attempts WHERE bucket=?1),0)",
                [bucket],
                |r| r.get(0),
            )?;
            if count >= limit {
                return Ok(LoginOutcome::Throttled);
            }
        }
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM login_attempts", [], |r| r.get(0))?;
        if count >= 4096 {
            return Ok(LoginOutcome::Throttled);
        }
        let user: Option<(i64, String, bool)> = self
            .conn
            .query_row(
                "SELECT id,password_hash,disabled FROM users WHERE username=?1",
                [&name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let dummy: String =
            self.conn
                .query_row("SELECT value FROM meta WHERE key='dummy_hash'", [], |r| {
                    r.get(0)
                })?;
        let hash = user
            .as_ref()
            .map_or(dummy.as_str(), |(_, hash, _)| hash.as_str());
        let valid = PasswordHash::new(hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        });
        let role: Option<String> = if let Some((id, _, false)) = &user {
            self.conn
                .query_row(
                    "SELECT role FROM memberships WHERE user_id=?1 AND tenant=?2",
                    params![id, tenant],
                    |r| r.get(0),
                )
                .optional()?
        } else {
            None
        };
        if !valid || role.is_none() {
            for bucket in [&account, "global"] {
                self.conn.execute("INSERT INTO login_attempts(bucket,window_start,failures) VALUES(?1,?2,1) ON CONFLICT(bucket) DO UPDATE SET failures=failures+1",params![bucket,at])?;
            }
            self.audit("unauthenticated", None, "login", "denied")?;
            return Ok(LoginOutcome::Invalid);
        }
        let (id, _, _) = user.ok_or("認証状態を確認できません。")?;
        let identity = self.identity(
            id,
            name,
            tenant.into(),
            role.as_deref().ok_or("認証状態を確認できません。")?,
        )?;
        let token = random_hex(32)?;
        self.conn.execute(
            "DELETE FROM sessions WHERE expires_at<=?1 OR last_seen<=?2",
            params![at, at - IDLE],
        )?;
        self.conn.execute("DELETE FROM sessions WHERE digest IN (SELECT digest FROM sessions WHERE user_id=?1 ORDER BY created_at DESC LIMIT -1 OFFSET 19)",[id])?;
        self.conn.execute("INSERT INTO sessions(digest,user_id,tenant,created_at,last_seen,expires_at) VALUES(?1,?2,?3,?4,?4,?5)",params![digest(token.as_bytes()),id,tenant,at,at+ABSOLUTE])?;
        self.conn
            .execute("DELETE FROM login_attempts WHERE bucket=?1", [account])?;
        self.audit(&identity.username, Some(tenant), "login", "success")?;
        Ok(LoginOutcome::Success(Login { token, identity }))
    }

    pub(super) fn authenticate(&self, token: &str, at: i64) -> HostedResult<Option<Identity>> {
        if token.len() != 64
            || !token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Ok(None);
        }
        let key = digest(token.as_bytes());
        let row:Option<(i64,String,String,String)>=self.conn.query_row("SELECT u.id,u.username,s.tenant,m.role FROM sessions s JOIN users u ON u.id=s.user_id JOIN memberships m ON m.user_id=s.user_id AND m.tenant=s.tenant WHERE s.digest=?1 AND u.disabled=0 AND s.created_at<=?2 AND s.last_seen<=?2 AND s.expires_at>?2 AND s.last_seen>?3",params![&key,at,at-IDLE],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if let Some((id, name, tenant, role)) = row {
            self.conn.execute(
                "UPDATE sessions SET last_seen=?2 WHERE digest=?1",
                params![key, at],
            )?;
            Ok(Some(self.identity(id, name, tenant, &role)?))
        } else {
            self.conn
                .execute("DELETE FROM sessions WHERE digest=?1", [key])?;
            Ok(None)
        }
    }

    pub(super) fn logout(&self, token: &str) -> HostedResult<()> {
        self.conn.execute(
            "DELETE FROM sessions WHERE digest=?1",
            [digest(token.as_bytes())],
        )?;
        Ok(())
    }
}
