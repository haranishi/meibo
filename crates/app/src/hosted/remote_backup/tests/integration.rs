use super::*;
use crate::hosted::{
    LoginOutcome, Role,
    remote::{
        config::{EndpointPolicy, Endpoints, RemoteConfig},
        operation::RemoteOperation,
        store::RemoteStore,
    },
};
use meibo_core::types::DataOrigin;
use meibo_db::{Connection, RemoteOptions, params};
use std::sync::Mutex;

#[allow(dead_code)]
mod client {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/client.rs"
    ));
}
#[allow(dead_code)]
mod proxy {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/proxy.rs"
    ));
}
#[allow(dead_code)]
mod sqld {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/sqld.rs"
    ));
}

const PASSWORD: &str = "synthetic-backup-password-only";
const TOKEN: &str = "synthetic-backup-token-only";
const HIGH_WATER: i64 = 9_007_199_254_740_993;
static FIXTURES: Mutex<()> = Mutex::new(());

mod restore;

fn endpoints(control: &str, alpha: &str, beta: &str) -> Endpoints {
    RemoteConfig::parse(br#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"},"beta":{"url_env":"MEIBO_DB_BETA_URL","token_env":"MEIBO_DB_BETA_TOKEN"}}}"#).unwrap().resolve_with(|key| Some(if key.ends_with("TOKEN") { TOKEN.into() } else if key.contains("CONTROL") { control.into() } else if key.contains("ALPHA") { alpha.into() } else { beta.into() }), EndpointPolicy::LoopbackTests).unwrap()
}
fn connection(endpoint: &str) -> Connection {
    Connection::open_remote(RemoteOptions::new(endpoint, TOKEN).allow_loopback_http_for_tests())
        .unwrap()
}
struct Fixture {
    control: sqld::Sqld,
    alpha: sqld::Sqld,
    beta: sqld::Sqld,
}
impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            control: sqld::Sqld::start().unwrap(),
            alpha: sqld::Sqld::start().unwrap(),
            beta: sqld::Sqld::start().unwrap(),
        };
        let mut store = RemoteStore::initialize(fixture.endpoints()).unwrap();
        for slug in ["alpha", "beta"] {
            store.add_empty_tenant(slug, DataOrigin::Synthetic).unwrap();
        }
        for (name, role) in [
            ("admin", Role::Admin),
            ("editor", Role::Editor),
            ("viewer", Role::Viewer),
        ] {
            store.add_user(name, PASSWORD).unwrap();
            for slug in ["alpha", "beta"] {
                store.set_member(name, slug, Some(role)).unwrap();
            }
        }
        assert!(matches!(
            store
                .login("alpha", "admin", PASSWORD, 1_800_000_000)
                .unwrap(),
            LoginOutcome::Success(_)
        ));
        for n in 0..25 {
            store
                .conn
                .execute(
                    "INSERT INTO meta(key,value) VALUES(?1,?2)",
                    params![format!("synthetic_{n:03}"), format!("value_{n}")],
                )
                .unwrap();
            store
                .conn
                .execute(
                    "INSERT INTO login_attempts VALUES(?1,?2,?3)",
                    params![format!("bucket_{n:03}"), 1_800_000_000_i64, n],
                )
                .unwrap();
            store
                .audit("synthetic", Some("alpha"), "fixture", "success")
                .unwrap();
        }
        for server in [&fixture.alpha, &fixture.beta] {
            seed(&connection(&server.endpoint()));
        }
        fixture
    }
    fn endpoints(&self) -> Endpoints {
        endpoints(
            &self.control.endpoint(),
            &self.alpha.endpoint(),
            &self.beta.endpoint(),
        )
    }
    fn store(&self) -> RemoteStore {
        RemoteStore::open(self.endpoints()).unwrap()
    }
}

fn seed(conn: &Connection) {
    for id in 1..=17 {
        conn.execute("INSERT INTO companies(id,name,name_key,prefecture_code,created_at,updated_at,phone) VALUES(?1,?2,?2,13,1800000000,1800000001,?3)", params![id, format!("synthetic {id} 日本語\ntext"), if id % 2 == 0 { Some("000-synthetic") } else { None }]).unwrap();
    }
    conn.execute_batch("INSERT INTO contacts(id,company_id,name,created_at) VALUES(1,1,'synthetic contact',1800000000);
        INSERT INTO activities(id,company_id,contact_id,kind,at,note) VALUES(1,1,1,1,1800000000,'synthetic note');
        INSERT INTO lists VALUES(1,'synthetic list',1800000000);
        INSERT INTO list_members VALUES(1,1);
        INSERT INTO crm_work_items(id,company_id,contact_id,kind,subject,due_at,completed,created_at,updated_at) VALUES(1,1,1,1,'synthetic task',1800000002,0,1800000000,1800000000);
        INSERT INTO crm_opportunities(id,company_id,contact_id,name,amount_jpy,stage,close_date,created_at,updated_at) VALUES(1,1,1,'synthetic opportunity',42,1,'2026-10-27',1800000000,1800000000);
        INSERT INTO custom_field_definitions(id,name,kind,is_indexed) VALUES(1,'synthetic number',2,1);
        INSERT INTO custom_field_values(field_id,company_id,kind,is_indexed,number_value) VALUES(1,1,2,1,1.25);
        INSERT INTO erased_corporate_numbers VALUES(x'0001ff');").unwrap();
    conn.execute("INSERT INTO custom_field_definitions(id,name,kind,is_indexed) VALUES(?1,'synthetic deleted',1,0)", [HIGH_WATER]).unwrap();
    conn.execute(
        "DELETE FROM custom_field_definitions WHERE id=?1",
        [HIGH_WATER],
    )
    .unwrap();
}

fn assert_rows(
    snapshot: &ValidatedSnapshot,
    slug: Option<&str>,
    conn: &Connection,
    profiles: impl Iterator<Item = profile::Profile>,
) {
    for profile in profiles {
        let mut statement = conn
            .prepare(&format!(
                "SELECT {} FROM {} ORDER BY {}",
                profile.columns.join(","),
                profile.name,
                profile.keys.join(",")
            ))
            .unwrap();
        let mut rows = statement.query([]).unwrap();
        let mut seen = 0;
        snapshot
            .visit_rows(slug, profile.name, |values| {
                let row = rows.next().unwrap().expect("source row was retained");
                let expected = (0..profile.columns.len())
                    .map(|index| row.get::<_, Sql>(index).unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(encode_row(values).unwrap(), encode_row(&expected).unwrap());
                seen += 1;
                Ok(())
            })
            .unwrap();
        assert!(rows.next().unwrap().is_none(), "all source rows were saved");
        assert!(seen > 0, "table {} has a real row", profile.name);
    }
}

#[test]
fn real_service_streams_all_control_and_crm_rows_and_sequence_under_one_gate() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let store = fixture.store();
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("snapshot");
    let mut operation = RemoteOperation::begin(&store, "test.backup").unwrap();
    let snapshot = create(&store, &mut operation, &destination).unwrap();
    assert!(store.gate_status().unwrap().is_some());
    assert_eq!(snapshot.manifest().tenants.len(), 2);
    assert_rows(
        &snapshot,
        None,
        &store.conn,
        profile::CONTROL.iter().copied(),
    );
    for (slug, server) in [("alpha", &fixture.alpha), ("beta", &fixture.beta)] {
        assert_rows(
            &snapshot,
            Some(slug),
            &connection(&server.endpoint()),
            profile::crm(),
        );
        let tenant = snapshot
            .manifest()
            .tenants
            .iter()
            .find(|tenant| tenant.slug == slug)
            .unwrap();
        assert_eq!(
            tenant
                .database
                .sequences
                .as_ref()
                .unwrap()
                .custom_field_definitions,
            HIGH_WATER.to_string()
        );
        assert!(
            tenant
                .database
                .tables
                .iter()
                .all(|table| !table.name.starts_with("name_fts") && table.name != "erase_gate")
        );
        let companies =
            fs::read_to_string(destination.join(format!("tenants/{slug}/companies.jsonl")))
                .unwrap();
        let first = companies.lines().next().unwrap();
        assert_eq!(
            meibo_core::remote_snapshot::decode_row(first.as_bytes(), 20)
                .unwrap()
                .len(),
            20
        );
    }
    assert!(!destination.join("control/sessions.jsonl").exists());
    assert!(!destination.join("control/remote_gate.jsonl").exists());
    assert!(
        store
            .conn
            .query_row("SELECT count(*) FROM sessions", [], |row| row
                .get::<_, i64>(0))
            .unwrap()
            > 0
    );
    let manifest = fs::read_to_string(destination.join("manifest.json")).unwrap();
    assert!(
        !manifest.contains(TOKEN)
            && !manifest.contains("url_env")
            && !manifest.contains(&fixture.control.endpoint())
    );
    operation.finish(&store, Ok(())).unwrap();
    assert!(store.gate_status().unwrap().is_none());
    let mut operation = RemoteOperation::begin(&store, "test.backup-existing").unwrap();
    let result = create(&store, &mut operation, &destination);
    assert!(result.is_err());
    assert!(operation.finish(&store, result).is_err());
    assert!(store.gate_status().unwrap().is_none());
}

#[test]
fn invalid_source_rows_erase_gate_and_sequence_fail_and_release_known_gate() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let store = fixture.store();
    let temporary = tempfile::tempdir().unwrap();
    let tenant = connection(&fixture.alpha.endpoint());
    let cases = [
        (
            "null-key",
            "INSERT INTO meta(key,value) VALUES(NULL,'bad')",
            "DELETE FROM meta WHERE key IS NULL",
            true,
        ),
        (
            "huge-row",
            "INSERT INTO meta(key,value) VALUES('too_large',printf('%.*c',1048577,'x'))",
            "DELETE FROM meta WHERE key='too_large'",
            true,
        ),
        (
            "duplicate-page-boundary",
            "ALTER TABLE login_attempts RENAME TO saved_login_attempts; CREATE TABLE login_attempts(bucket TEXT,window_start INTEGER,failures INTEGER); INSERT INTO login_attempts SELECT * FROM saved_login_attempts; INSERT INTO login_attempts SELECT * FROM saved_login_attempts WHERE bucket='bucket_007'",
            "DROP TABLE login_attempts; ALTER TABLE saved_login_attempts RENAME TO login_attempts",
            true,
        ),
        (
            "erase-gate",
            "INSERT INTO erase_gate VALUES(1)",
            "DELETE FROM erase_gate",
            false,
        ),
        (
            "sequence",
            "UPDATE sqlite_sequence SET seq=-1 WHERE name='custom_field_definitions'",
            "UPDATE sqlite_sequence SET seq=9007199254740993 WHERE name='custom_field_definitions'",
            false,
        ),
    ];
    for (name, change, reset, control) in cases {
        let conn = if control { &store.conn } else { &tenant };
        conn.execute_batch(change).unwrap();
        let mut operation = RemoteOperation::begin(&store, "test.invalid-backup").unwrap();
        let result = create(&store, &mut operation, &temporary.path().join(name));
        assert!(result.is_err(), "invalid source {name}");
        assert!(operation.finish(&store, result).is_err());
        assert!(store.gate_status().unwrap().is_none());
        conn.execute_batch(reset).unwrap();
    }
}

#[test]
fn lost_snapshot_transaction_completion_keeps_gate_and_sends_no_later_sql() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for control_fault in [true, false] {
        let fixture = Fixture::new();
        let control_proxy = proxy::FaultProxy::start(fixture.control.address()).unwrap();
        let tenant_proxy = proxy::FaultProxy::start(fixture.alpha.address()).unwrap();
        let store = RemoteStore::open(endpoints(
            &control_proxy.endpoint(),
            &tenant_proxy.endpoint(),
            &fixture.beta.endpoint(),
        ))
        .unwrap();
        let mut operation = RemoteOperation::begin(&store, "test.lost-backup").unwrap();
        let fault = if control_fault {
            &control_proxy
        } else {
            &tenant_proxy
        };
        fault.arm(proxy::Fault::LoseRollbackResponse);
        let temporary = tempfile::tempdir().unwrap();
        let result = create(&store, &mut operation, &temporary.path().join("snapshot"));
        assert!(result.is_err());
        assert_eq!(fault.triggered(), 1);
        let sent = (control_proxy.requests(), tenant_proxy.requests());
        let error = operation.finish(&store, result).err().unwrap().to_string();
        assert!(error.starts_with("outcome_unknown:"));
        assert_eq!((control_proxy.requests(), tenant_proxy.requests()), sent);
        assert!(fixture.store().gate_status().unwrap().is_some());
        assert!(!temporary.path().join("snapshot/manifest.json").exists());
    }
}
