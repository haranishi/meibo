use super::*;
use meibo_core::remote_snapshot::encode_row;
use meibo_db::types::Value as Sql;
use serde_json::{Value, json};
use std::{fs, path::Path};

const CONTROL_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TENANT_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const INSTANCE: &str = "cccccccccccccccccccccccccccccccc";

fn fixture() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    let root = files::new_directory(&temporary.path().join("snapshot")).unwrap();
    for path in ["control", "tenants", "tenants/alpha"] {
        files::new_directory(&root.join(path)).unwrap();
    }
    let dummy = crate::hosted::auth::hash_password("synthetic-backup-password-only").unwrap();
    let control = [
        ("database_id", CONTROL_ID),
        ("database_kind", "control"),
        ("dummy_hash", &dummy),
        ("instance", INSTANCE),
        ("remote_format", "meibo-control-remote-v1"),
        ("schema_version", "1"),
        ("state", "ready"),
    ];
    let crm = [
        ("created_at", "1800000000"),
        ("crm_schema_version", "1"),
        ("custom_fields_schema_version", "1"),
        ("data_origin", "synthetic"),
        ("database_id", TENANT_ID),
        ("database_kind", "crm"),
        ("public_data_schema_version", "1"),
        ("remote_format", "meibo-remote-v1"),
        ("schema_version", "1"),
    ];
    let manifest = Manifest {
        version: 1,
        service: manifest::Service {
            instance: INSTANCE.into(),
            database_id: CONTROL_ID.into(),
        },
        control: Database {
            database_id: CONTROL_ID.into(),
            database_kind: "control".into(),
            schema_version: 1,
            data_origin: None,
            sequences: None,
            tables: write_tables(&root, "control", profile::CONTROL.to_vec(), &control),
        },
        tenants: vec![Tenant {
            slug: "alpha".into(),
            database: Database {
                database_id: TENANT_ID.into(),
                database_kind: "crm".into(),
                schema_version: 1,
                data_origin: Some("synthetic".into()),
                sequences: Some(manifest::Sequences {
                    custom_field_definitions: "42".into(),
                }),
                tables: write_tables(&root, "tenants/alpha", profile::crm().collect(), &crm),
            },
        }],
    };
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    temporary
}

fn write_tables(
    root: &Path,
    prefix: &str,
    profiles: Vec<profile::Profile>,
    markers: &[(&str, &str)],
) -> Vec<Table> {
    profiles
        .into_iter()
        .map(|profile| {
            let mut writer = files::RowWriter::new(root, prefix, profile.name).unwrap();
            if profile.name == "meta" {
                for (key, value) in markers {
                    writer
                        .write(
                            &encode_row(&[Sql::Text((*key).into()), Sql::Text((*value).into())])
                                .unwrap(),
                        )
                        .unwrap();
                }
            }
            if prefix == "control" && profile.name == "tenants" {
                writer
                    .write(&encode_row(&[Sql::Text("alpha".into())]).unwrap())
                    .unwrap();
            }
            if profile.name == "remote_tenants" {
                writer
                    .write(
                        &encode_row(&[Sql::Text("alpha".into()), Sql::Text(TENANT_ID.into())])
                            .unwrap(),
                    )
                    .unwrap();
            }
            if profile.name == "custom_field_definitions" {
                writer
                    .write(
                        &encode_row(&[
                            Sql::Integer(7),
                            Sql::Text("sample".into()),
                            Sql::Integer(1),
                            Sql::Integer(0),
                        ])
                        .unwrap(),
                    )
                    .unwrap();
            }
            writer.finish().unwrap()
        })
        .collect()
}

fn mutate(root: &Path, change: impl FnOnce(&mut Value)) {
    let path = root.join("manifest.json");
    let mut value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut value);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

#[test]
fn snapshot_manifest_and_reader_verify_all_rows_without_sql() {
    let temporary = fixture();
    let root = temporary.path().join("snapshot");
    let snapshot = validate(&root).unwrap();
    let mut rows = 0;
    snapshot
        .visit_rows(Some("alpha"), "custom_field_definitions", |values| {
            assert_eq!(values[0], Sql::Integer(7));
            rows += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(rows, 1);
    assert!(snapshot.visit_rows(None, "sessions", |_| Ok(())).is_err());
    fs::write(
        root.join("tenants/alpha/custom_field_definitions.jsonl"),
        [],
    )
    .unwrap();
    assert!(
        snapshot
            .visit_rows(Some("alpha"), "custom_field_definitions", |_| Ok(()))
            .is_err()
    );
}

#[test]
fn manifest_rejects_unknown_duplicate_missing_and_inconsistent_fields() {
    let temporary = fixture();
    let root = temporary.path().join("snapshot");
    let original = fs::read(root.join("manifest.json")).unwrap();
    let changes: &[fn(&mut Value)] = &[
        |v| v["version"] = json!(2),
        |v| v["sql"] = json!("untrusted"),
        |v| v["control"]["tables"][0]["file"] = json!("../meta.jsonl"),
        |v| v["control"]["tables"][0]["name"] = json!("sessions"),
        |v| v["control"]["tables"][0]["columns"] = json!(["key"]),
        |v| v["control"]["tables"][0]["rows"] = json!(0),
        |v| v["control"]["tables"][0]["sha256"] = json!("0".repeat(64)),
        |v| {
            v["control"]["tables"].as_array_mut().unwrap().pop();
        },
        |v| v["control"]["tables"][1] = v["control"]["tables"][0].clone(),
        |v| v["tenants"][0]["slug"] = json!("../alpha"),
        |v| v["tenants"][0]["database"]["database_id"] = json!(CONTROL_ID),
        |v| v["tenants"][0]["database"]["data_origin"] = json!("real"),
        |v| v["tenants"][0]["database"]["schema_version"] = json!(2),
        |v| {
            v["tenants"][0]["database"]
                .as_object_mut()
                .unwrap()
                .remove("sequences");
        },
        |v| v["tenants"][0]["database"]["sequences"]["unknown"] = json!("0"),
        |v| v["tenants"][0]["database"]["sequences"]["custom_field_definitions"] = json!("6"),
        |v| v["tenants"][0]["database"]["sequences"]["custom_field_definitions"] = json!("-1"),
        |v| {
            v["tenants"][0]["database"]["sequences"]["custom_field_definitions"] =
                json!("9223372036854775808");
        },
        |v| v["tenants"][0]["database"]["sequences"]["custom_field_definitions"] = json!("042"),
        |v| {
            let tenant = v["tenants"][0].clone();
            v["tenants"] = json!(vec![tenant; 1001]);
        },
    ];
    for (index, change) in changes.iter().enumerate() {
        fs::write(root.join("manifest.json"), &original).unwrap();
        mutate(&root, change);
        assert!(validate(&root).is_err(), "mutation {index}");
    }
    fs::write(root.join("manifest.json"), br#"{"version":1,"version":1}"#).unwrap();
    assert!(validate(&root).is_err());
    fs::write(
        root.join("manifest.json"),
        vec![b' '; manifest::MAX_MANIFEST_BYTES + 1],
    )
    .unwrap();
    assert!(validate(&root).is_err());
}

#[test]
fn snapshot_rows_reject_tampering_even_with_updated_digests() {
    use sha2::{Digest, Sha256};
    let temporary = fixture();
    let root = temporary.path().join("snapshot");
    let original_manifest = fs::read(root.join("manifest.json")).unwrap();
    let original = fs::read(root.join("control/meta.jsonl")).unwrap();
    let mut cases = vec![
        b"{\"version\":2,\"values\":[]}\n".to_vec(),
        encode_row(&[Sql::Null, Sql::Text("bad".into())]).unwrap(),
        encode_row(&[Sql::Text("only-one-column".into())]).unwrap(),
    ];
    cases[1].push(b'\n');
    cases[2].push(b'\n');
    let first = original
        .split_inclusive(|byte| *byte == b'\n')
        .next()
        .unwrap();
    cases.push([first, first].concat());
    cases.push(vec![b'x'; meibo_core::remote_snapshot::MAX_ROW_BYTES + 2]);
    cases.push(b"{\"version\":1,\"values\":[{\"type\":\"unknown\"}]}\n".to_vec());
    for bytes in cases {
        fs::write(root.join("manifest.json"), &original_manifest).unwrap();
        fs::write(root.join("control/meta.jsonl"), &bytes).unwrap();
        mutate(&root, |v| {
            v["control"]["tables"][0]["rows"] =
                json!(bytes.split(|byte| *byte == b'\n').count() - 1);
            v["control"]["tables"][0]["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        });
        assert!(validate(&root).is_err());
    }
}

#[test]
fn snapshot_files_reject_missing_extra_overwrite_and_symlinks() {
    let temporary = fixture();
    let root = temporary.path().join("snapshot");
    assert!(files::new_directory(&root).is_err());
    assert!(files::new_file(&root.join("manifest.json")).is_err());
    fs::write(root.join("extra.jsonl"), []).unwrap();
    assert!(validate(&root).is_err());
    fs::remove_file(root.join("extra.jsonl")).unwrap();
    let path = root.join("control/users.jsonl");
    fs::remove_file(&path).unwrap();
    assert!(validate(&root).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        let target = root.join("control/audit.jsonl");
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        symlink(&target, &path).unwrap();
        assert!(validate(&root).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, []).unwrap();
        let alias = temporary.path().join("alias");
        symlink(&root, &alias).unwrap();
        assert!(validate(&alias).is_err());
        assert!(files::new_directory(&alias.join("new")).is_err());
    }
}

#[cfg(feature = "integration-tests")]
mod integration;
