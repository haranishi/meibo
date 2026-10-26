use super::*;
use meibo_core::{
    custom_fields::{CustomFieldInput, CustomFieldKind},
    types::SearchOptions,
};

struct Targets {
    control: sqld::Sqld,
    alpha: sqld::Sqld,
    beta: sqld::Sqld,
}
impl Targets {
    fn new() -> Self {
        Self {
            control: sqld::Sqld::start().unwrap(),
            alpha: sqld::Sqld::start().unwrap(),
            beta: sqld::Sqld::start().unwrap(),
        }
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
    fn empty(&self) {
        for server in [&self.control, &self.alpha, &self.beta] {
            assert_eq!(
                connection(&server.endpoint())
                    .query_row(
                        "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
        }
    }
}

fn source() -> Fixture {
    let fixture = Fixture::new();
    for server in [&fixture.alpha, &fixture.beta] {
        let conn = connection(&server.endpoint());
        conn.execute(
            "UPDATE companies SET last_activity_at=1800000000 WHERE id=1",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM counts", []).unwrap();
        for status in 0..6 {
            for phone in 0..2 {
                conn.execute("INSERT INTO counts SELECT ?1,?2,count(*) FROM companies WHERE status=?1 AND has_phone=?2", (status,phone)).unwrap();
            }
        }
        conn.execute("INSERT INTO name_fts(name_fts) VALUES('rebuild')", [])
            .unwrap();
    }
    fixture
}

fn backup(store: &RemoteStore) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let mut operation = RemoteOperation::begin(store, "test.restore-backup").unwrap();
    let result = create(store, &mut operation, &directory.path().join("snapshot"));
    operation.finish(store, result).unwrap();
    directory
}

fn run_restore(store: &RemoteStore, snapshot: &Path, targets: Endpoints) -> HostedResult<()> {
    let mut operation = RemoteOperation::begin(store, "test.restore").unwrap();
    let result = restore(store, &mut operation, snapshot, targets);
    operation.finish(store, result)
}

fn rows(conn: &Connection, profile: profile::Profile) -> Vec<Vec<u8>> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {} FROM {} ORDER BY {}",
            profile.columns.join(","),
            profile.name,
            profile.keys.join(",")
        ))
        .unwrap();
    let mut rows = statement.query([]).unwrap();
    let mut values = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        values.push(
            encode_row(
                &(0..profile.columns.len())
                    .map(|index| row.get::<_, Sql>(index).unwrap())
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        );
    }
    values
}

fn assert_same_rows(
    source: &Connection,
    target: &Connection,
    profiles: impl Iterator<Item = profile::Profile>,
    control: bool,
) {
    for profile in profiles {
        if profile.name == "meta" || (control && matches!(profile.name, "remote_tenants" | "audit"))
        {
            continue;
        }
        assert_eq!(
            rows(source, profile),
            rows(target, profile),
            "complete typed rows in {}",
            profile.name
        );
    }
}

fn rewrite_table(root: &Path, table: &str, bytes: &[u8]) {
    use sha2::{Digest, Sha256};
    let file = format!("tenants/alpha/{table}.jsonl");
    fs::write(root.join(&file), bytes).unwrap();
    let mut manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let entry = manifest
        .tenants
        .iter_mut()
        .find(|tenant| tenant.slug == "alpha")
        .unwrap()
        .database
        .tables
        .iter_mut()
        .find(|entry| entry.name == table)
        .unwrap();
    entry.sha256 = format!("{:x}", Sha256::digest(bytes));
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn restore_rejects_bad_counts_folds_foreign_keys_and_name_keys_without_activating() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = source();
    let store = fixture.store();
    for (table, index, changed) in [
        ("counts", 2, Sql::Integer(999)),
        ("companies", 11, Sql::Integer(999)),
        ("contacts", 1, Sql::Integer(999)),
        ("companies", 3, Sql::Text("wrong key".into())),
    ] {
        let directory = backup(&store);
        let root = directory.path().join("snapshot");
        let file = fs::read_to_string(root.join(format!("tenants/alpha/{table}.jsonl"))).unwrap();
        let mut lines = file.lines();
        let profile = profile::crm()
            .find(|profile| profile.name == table)
            .unwrap();
        let mut row = meibo_core::remote_snapshot::decode_row(
            lines.next().unwrap().as_bytes(),
            profile.columns.len(),
        )
        .unwrap();
        row[index] = changed;
        let mut modified = encode_row(&row).unwrap();
        modified.push(b'\n');
        for line in lines {
            modified.extend_from_slice(line.as_bytes());
            modified.push(b'\n');
        }
        rewrite_table(&root, table, &modified);
        validate(&root).unwrap();
        let targets = Targets::new();
        let error = run_restore(&store, &root, targets.endpoints())
            .unwrap_err()
            .to_string();
        assert!(!error.contains(TOKEN) && !error.contains(&fixture.control.endpoint()));
        assert!(store.gate_status().unwrap().is_none());
        assert!(RemoteStore::open(targets.endpoints()).is_err());
        assert_eq!(
            connection(&targets.control.endpoint())
                .query_row("SELECT value FROM meta WHERE key='state'", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "restoring"
        );
        assert_eq!(
            store
                .open_tenant("alpha")
                .unwrap()
                .count(&meibo_core::types::Filters::default())
                .unwrap()
                .n,
            17
        );
    }
}

fn assert_tenant_roundtrip(
    store: &RemoteStore,
    target: &RemoteStore,
    slug: &str,
    original: &sqld::Sqld,
    restored: &sqld::Sqld,
) {
    assert_same_rows(
        &connection(&original.endpoint()),
        &connection(&restored.endpoint()),
        profile::crm(),
        false,
    );
    let old_id = store
        .registered_tenants()
        .unwrap()
        .into_iter()
        .find(|entry| entry.0 == slug)
        .unwrap()
        .1;
    let new_id = target
        .registered_tenants()
        .unwrap()
        .into_iter()
        .find(|entry| entry.0 == slug)
        .unwrap()
        .1;
    assert_ne!(old_id.database_id(), new_id.database_id());
    let mut engine = target.open_tenant(slug).unwrap();
    let report = engine.verify().unwrap();
    assert!(report.integrity_ok && report.counts_match && report.folds_match);
    assert_eq!(
        engine
            .search_name(
                "synthetic",
                &SearchOptions {
                    after: None,
                    limit: 20,
                    fields: vec![],
                    include_excluded: false,
                    with_count: true
                }
            )
            .unwrap()
            .rows
            .len(),
        17
    );
    assert!(
        engine
            .create_custom_field(&CustomFieldInput {
                name: "after restore".into(),
                kind: CustomFieldKind::Text,
                indexed: false
            })
            .unwrap()
            > HIGH_WATER
    );
}

#[test]
fn restore_round_trips_two_tenants_three_roles_all_rows_and_deleted_high_water() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = source();
    let mut store = fixture.store();
    let LoginOutcome::Success(old) = store
        .login("alpha", "admin", PASSWORD, 1_800_000_000)
        .unwrap()
    else {
        panic!("source login");
    };
    let snapshot = backup(&store);
    let targets = Targets::new();
    run_restore(
        &store,
        &snapshot.path().join("snapshot"),
        targets.endpoints(),
    )
    .unwrap();
    let mut target = targets.store();
    assert_ne!(target.instance(), store.instance());
    assert_ne!(target.database_id(), store.database_id());
    assert_same_rows(
        &store.conn,
        &target.conn,
        profile::CONTROL.iter().copied(),
        true,
    );
    assert_eq!(
        target
            .conn
            .query_row("SELECT count(*) FROM audit", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        store
            .conn
            .query_row("SELECT count(*) FROM audit", [], |row| row.get::<_, i64>(0))
            .unwrap()
            + 1
    );
    assert_eq!(
        target
            .conn
            .query_row("SELECT count(*) FROM sessions", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(target.gate_status().unwrap().is_none());
    for (slug, original, restored) in [
        ("alpha", &fixture.alpha, &targets.alpha),
        ("beta", &fixture.beta, &targets.beta),
    ] {
        assert_tenant_roundtrip(&store, &target, slug, original, restored);
    }
    assert!(
        target
            .authenticate(&old.token, 1_800_000_001)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .authenticate(&old.token, 1_800_000_001)
            .unwrap()
            .is_some()
    );
    for (name, role) in [
        ("admin", Role::Admin),
        ("editor", Role::Editor),
        ("viewer", Role::Viewer),
    ] {
        for slug in ["alpha", "beta"] {
            let LoginOutcome::Success(login) =
                target.login(slug, name, PASSWORD, 1_800_000_001).unwrap()
            else {
                panic!("restored login");
            };
            assert!(login.identity.role == role);
            if slug == "alpha" && name == "admin" {
                assert_ne!(login.identity.scope, old.identity.scope);
            }
        }
    }
}

#[test]
fn restore_validates_every_file_and_every_empty_target_before_first_write() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = source();
    let store = fixture.store();
    let snapshot = backup(&store);
    let root = snapshot.path().join("snapshot");
    let targets = Targets::new();
    let file = root.join("tenants/beta/companies.jsonl");
    let original = fs::read(&file).unwrap();
    fs::write(&file, b"tampered\n").unwrap();
    assert!(run_restore(&store, &root, targets.endpoints()).is_err());
    targets.empty();
    fs::write(&file, original).unwrap();
    connection(&targets.beta.endpoint())
        .execute("CREATE TABLE retained(value TEXT)", [])
        .unwrap();
    assert!(run_restore(&store, &root, targets.endpoints()).is_err());
    for server in [&targets.control, &targets.alpha] {
        assert_eq!(
            connection(&server.endpoint())
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    assert!(store.gate_status().unwrap().is_none());
    assert!(
        run_restore(
            &store,
            &root,
            endpoints(
                &targets.control.endpoint(),
                &targets.alpha.endpoint(),
                &fixture.alpha.endpoint()
            )
        )
        .is_err()
    );
    assert_eq!(
        store
            .open_tenant("alpha")
            .unwrap()
            .count(&meibo_core::types::Filters::default())
            .unwrap()
            .n,
        17
    );
}

#[test]
fn restore_first_control_commit_is_restoring_and_lost_reply_retains_source_gate() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = source();
    let source_proxy = proxy::FaultProxy::start(fixture.control.address()).unwrap();
    let store = RemoteStore::open(endpoints(
        &source_proxy.endpoint(),
        &fixture.alpha.endpoint(),
        &fixture.beta.endpoint(),
    ))
    .unwrap();
    let snapshot = backup(&store);
    let targets = Targets::new();
    let target_proxy = proxy::FaultProxy::start(targets.control.address()).unwrap();
    target_proxy.arm(proxy::Fault::LoseCommitResponse);
    let mut operation = RemoteOperation::begin(&store, "test.restore-fault").unwrap();
    let result = restore(
        &store,
        &mut operation,
        &snapshot.path().join("snapshot"),
        endpoints(
            &target_proxy.endpoint(),
            &targets.alpha.endpoint(),
            &targets.beta.endpoint(),
        ),
    );
    assert!(result.is_err());
    assert_eq!(target_proxy.triggered(), 1);
    let requests = (source_proxy.requests(), target_proxy.requests());
    assert!(
        operation
            .finish(&store, result)
            .unwrap_err()
            .to_string()
            .starts_with("outcome_unknown:")
    );
    assert_eq!((source_proxy.requests(), target_proxy.requests()), requests);
    assert_eq!(
        connection(&targets.control.endpoint())
            .query_row("SELECT value FROM meta WHERE key='state'", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "restoring"
    );
    assert!(RemoteStore::open(targets.endpoints()).is_err());
    assert!(fixture.store().gate_status().unwrap().is_some());
    for server in [&targets.alpha, &targets.beta] {
        assert_eq!(
            connection(&server.endpoint())
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn restore_lost_tenant_write_stops_all_later_sql_and_keeps_candidate_offline() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = source();
    let source_proxy = proxy::FaultProxy::start(fixture.control.address()).unwrap();
    let store = RemoteStore::open(endpoints(
        &source_proxy.endpoint(),
        &fixture.alpha.endpoint(),
        &fixture.beta.endpoint(),
    ))
    .unwrap();
    let snapshot = backup(&store);
    let targets = Targets::new();
    let control_proxy = proxy::FaultProxy::start(targets.control.address()).unwrap();
    let tenant_proxy = proxy::FaultProxy::start(targets.alpha.address()).unwrap();
    tenant_proxy.arm(proxy::Fault::LoseMatchingWriteResponse(
        "INSERT INTO companies(",
    ));
    let mut operation = RemoteOperation::begin(&store, "test.restore-tenant-fault").unwrap();
    let result = restore(
        &store,
        &mut operation,
        &snapshot.path().join("snapshot"),
        endpoints(
            &control_proxy.endpoint(),
            &tenant_proxy.endpoint(),
            &targets.beta.endpoint(),
        ),
    );
    assert!(result.is_err());
    assert_eq!(tenant_proxy.triggered(), 1);
    let requests = (
        source_proxy.requests(),
        control_proxy.requests(),
        tenant_proxy.requests(),
    );
    assert!(
        operation
            .finish(&store, result)
            .unwrap_err()
            .to_string()
            .starts_with("outcome_unknown:")
    );
    assert_eq!(
        (
            source_proxy.requests(),
            control_proxy.requests(),
            tenant_proxy.requests()
        ),
        requests
    );
    assert_eq!(
        connection(&targets.control.endpoint())
            .query_row("SELECT value FROM meta WHERE key='state'", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "restoring"
    );
    assert!(RemoteStore::open(targets.endpoints()).is_err());
    assert!(fixture.store().gate_status().unwrap().is_some());
    assert_eq!(
        connection(&targets.beta.endpoint())
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
