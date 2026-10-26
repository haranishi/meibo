//! Server-owned remote control and CRM registrations.
use std::{
    collections::BTreeSet,
    error::Error,
    fmt,
    ops::{Deref, DerefMut},
};

use argon2::password_hash::PasswordHash;
use meibo_core::{Engine, RemoteIdentity, types::DataOrigin};
use meibo_db::{Connection, TransactionBehavior, params};

use super::{
    super::{
        HostedResult, Role, auth,
        control::ControlStore,
        store::{DDL, engine_options, valid_slug},
    },
    config::{Endpoints, MAX_TENANTS},
};

const FORMAT: &str = "meibo-control-remote-v1";
const CONTROL_ERROR: &str = "認証DBの保存形式・組織対応を確認してください。";
const TENANT_ERROR: &str = "登録された組織DBを確認してください。";
const REMOTE_DDL: &str = "CREATE TABLE remote_tenants(
    slug TEXT PRIMARY KEY REFERENCES tenants(slug),
    database_id TEXT NOT NULL UNIQUE
);";
const METADATA: &str = "SELECT key,value FROM meta WHERE key IN (
    'remote_format','schema_version','database_id','database_kind',
    'instance','state','dummy_hash')";
const SCHEMA_PROBE: &str = "SELECT users.id,users.username,users.password_hash,users.disabled,
    tenants.slug,memberships.user_id,memberships.tenant,memberships.role,
    sessions.digest,sessions.user_id,sessions.tenant,sessions.created_at,
    sessions.last_seen,sessions.expires_at,login_attempts.bucket,
    login_attempts.window_start,login_attempts.failures,audit.id,audit.at,
    audit.actor,audit.tenant,audit.operation,audit.outcome,
    remote_tenants.slug,remote_tenants.database_id,
    remote_gate.slot,remote_gate.owner,remote_gate.at,remote_gate.operation
    FROM users,tenants,memberships,sessions,login_attempts,audit,remote_tenants,remote_gate WHERE 0";

// The source is retained for operation-gate classification, while every
// metadata/shape diagnostic stays fixed and excludes database contents.
struct ControlError(Box<dyn Error>);
impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(CONTROL_ERROR)
    }
}
impl fmt::Debug for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ControlError")
    }
}
impl Error for ControlError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}
fn control_error(error: impl Error + 'static) -> Box<dyn Error> {
    Box::new(ControlError(Box::new(error)))
}

struct ServiceIdentity {
    database_id: String,
    instance: String,
}

pub(in crate::hosted) struct RemoteStore {
    pub(super) endpoints: Endpoints,
    pub(super) control: ControlStore,
    identity: ServiceIdentity,
}

impl RemoteStore {
    pub(in crate::hosted) fn initialize(endpoints: Endpoints) -> HostedResult<Self> {
        let mut conn = Connection::open_remote(endpoints.control.clone())?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let occupied: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
            [],
            |row| row.get(0),
        )?;
        if occupied {
            return Err("初期化には空の認証DBが必要です。".into());
        }
        tx.execute_batch(DDL)?;
        tx.execute_batch(REMOTE_DDL)?;
        tx.execute_batch(crate::hosted::remote_gate::DDL)?;
        let identity = ServiceIdentity {
            database_id: auth::random_hex(16)?,
            instance: auth::random_hex(16)?,
        };
        let dummy = auth::hash_password(&auth::random_hex(32)?)?;
        for (key, value) in [
            ("remote_format", FORMAT.to_owned()),
            ("schema_version", "1".to_owned()),
            ("database_id", identity.database_id.clone()),
            ("database_kind", "control".to_owned()),
            ("instance", identity.instance.clone()),
            ("state", "ready".to_owned()),
            ("dummy_hash", dummy),
        ] {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)",
                params![key, value],
            )?;
        }
        tx.commit()?;
        let store = Self {
            endpoints,
            control: ControlStore { conn },
            identity,
        };
        store.audit("maintenance", None, "initialize", "success")?;
        Ok(store)
    }

    pub(in crate::hosted) fn open(endpoints: Endpoints) -> HostedResult<Self> {
        let conn = Connection::open_remote(endpoints.control.clone())?;
        let identity = validate_control(&conn)?;
        let store = Self {
            endpoints,
            control: ControlStore { conn },
            identity,
        };
        for (slug, identity) in store.registered_tenants()? {
            Engine::open_remote(store.endpoint(&slug)?, &identity, &engine_options())?;
        }
        Ok(store)
    }

    pub(in crate::hosted) fn database_id(&self) -> &str {
        &self.identity.database_id
    }
    pub(in crate::hosted) fn instance(&self) -> &str {
        &self.identity.instance
    }

    pub(in crate::hosted) fn check_ready(&self) -> HostedResult<()> {
        let identity = validate_control(&self.conn)?;
        if identity.database_id != self.identity.database_id
            || identity.instance != self.identity.instance
        {
            return Err(CONTROL_ERROR.into());
        }
        Ok(())
    }

    fn endpoint(&self, slug: &str) -> HostedResult<meibo_db::RemoteOptions> {
        if !valid_slug(slug) {
            return Err(TENANT_ERROR.into());
        }
        self.endpoints
            .tenants
            .get(slug)
            .cloned()
            .ok_or_else(|| TENANT_ERROR.into())
    }

    pub(in crate::hosted) fn registered_tenants(
        &self,
    ) -> HostedResult<Vec<(String, RemoteIdentity)>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut statement = tx.prepare("SELECT t.slug,r.database_id FROM tenants t LEFT JOIN remote_tenants r ON r.slug=t.slug ORDER BY t.slug LIMIT 1001").map_err(control_error)?;
        let mut registrations = Vec::new();
        let mut seen = BTreeSet::new();
        for row in statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(control_error)?
        {
            let (slug, database_id) = row.map_err(control_error)?;
            if registrations.len() >= MAX_TENANTS
                || !valid_slug(&slug)
                || !self.endpoints.tenants.contains_key(&slug)
                || database_id == self.identity.database_id
                || !seen.insert(database_id.clone())
            {
                return Err(CONTROL_ERROR.into());
            }
            let identity = RemoteIdentity::new(database_id, "crm").map_err(|_| CONTROL_ERROR)?;
            registrations.push((slug, identity));
        }
        drop(statement);
        let count: i64 = tx
            .query_row("SELECT count(*) FROM remote_tenants", [], |row| row.get(0))
            .map_err(control_error)?;
        if usize::try_from(count).ok() != Some(registrations.len()) {
            return Err(CONTROL_ERROR.into());
        }
        tx.commit()?;
        Ok(registrations)
    }

    /// Create only in the configured empty endpoint. Registration is separate
    /// so callers can finish initialization before making a tenant accessible.
    pub(in crate::hosted) fn create_tenant(
        &self,
        slug: &str,
        origin: DataOrigin,
    ) -> HostedResult<(RemoteIdentity, Engine)> {
        self.check_ready()?;
        let endpoint = self.endpoint(slug)?;
        if self.tenant_exists(slug)? {
            return Err("組織は登録済みです。".into());
        }
        let identity = RemoteIdentity::new(auth::random_hex(16)?, "crm")?;
        let engine = Engine::create_remote(endpoint, &identity, origin, &engine_options())?;
        Ok((identity, engine))
    }

    pub(in crate::hosted) fn register_tenant(
        &mut self,
        slug: &str,
        identity: &RemoteIdentity,
    ) -> HostedResult<()> {
        self.check_ready()?;
        if identity.database_id() == self.identity.database_id {
            return Err(TENANT_ERROR.into());
        }
        Engine::open_remote(self.endpoint(slug)?, identity, &engine_options())?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO tenants(slug) VALUES(?1)", [slug])?;
        tx.execute(
            "INSERT INTO remote_tenants(slug,database_id) VALUES(?1,?2)",
            params![slug, identity.database_id()],
        )?;
        tx.commit()?;
        self.audit("maintenance", Some(slug), "tenant.add", "success")
    }

    pub(in crate::hosted) fn add_empty_tenant(
        &mut self,
        slug: &str,
        origin: DataOrigin,
    ) -> HostedResult<()> {
        let (identity, engine) = self.create_tenant(slug, origin)?;
        drop(engine);
        self.register_tenant(slug, &identity)
    }

    pub(in crate::hosted) fn open_tenant(&self, slug: &str) -> HostedResult<Engine> {
        self.check_ready()?;
        let identity = self
            .registered_tenants()?
            .into_iter()
            .find(|(registered, _)| registered == slug)
            .map(|(_, identity)| identity)
            .ok_or(TENANT_ERROR)?;
        Ok(Engine::open_remote(
            self.endpoint(slug)?,
            &identity,
            &engine_options(),
        )?)
    }

    pub(in crate::hosted) fn set_member(
        &mut self,
        name: &str,
        tenant: &str,
        role: Option<Role>,
    ) -> HostedResult<()> {
        self.open_tenant(tenant)?;
        self.control.set_member_record(name, tenant, role)
    }
}

impl Deref for RemoteStore {
    type Target = ControlStore;
    fn deref(&self) -> &Self::Target {
        &self.control
    }
}
impl DerefMut for RemoteStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.control
    }
}

fn validate_control(conn: &Connection) -> HostedResult<ServiceIdentity> {
    let tx = conn.unchecked_transaction()?;
    let mut metadata = std::collections::BTreeMap::new();
    let mut statement = tx.prepare(METADATA).map_err(control_error)?;
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(control_error)?
    {
        let (key, value) = row.map_err(control_error)?;
        if metadata.insert(key, value).is_some() {
            return Err(CONTROL_ERROR.into());
        }
    }
    drop(statement);
    let required = |key| metadata.get(key).map(String::as_str).ok_or(CONTROL_ERROR);
    if required("remote_format")? != FORMAT
        || required("schema_version")? != "1"
        || required("database_kind")? != "control"
        || required("state")? != "ready"
        || !opaque_id(required("database_id")?)
        || !opaque_id(required("instance")?)
        || !valid_dummy_hash(required("dummy_hash")?)
    {
        return Err(CONTROL_ERROR.into());
    }
    tx.prepare(SCHEMA_PROBE).map_err(control_error)?;
    let identity = ServiceIdentity {
        database_id: required("database_id")?.into(),
        instance: required("instance")?.into(),
    };
    tx.commit()?;
    Ok(identity)
}

fn opaque_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_dummy_hash(value: &str) -> bool {
    PasswordHash::new(value).is_ok_and(|hash| {
        hash.algorithm.as_str() == "argon2id"
            && hash.version == Some(19)
            && hash.params.get_decimal("m") == Some(19456)
            && hash.params.get_decimal("t") == Some(2)
            && hash.params.get_decimal("p") == Some(1)
            && hash.salt.is_some()
            && hash.hash.is_some()
    })
}
