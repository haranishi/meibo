use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use meibo_core::{
    synth::{Profile, SynthConfig},
    types::DataOrigin,
};
use serde_json::json;

use super::{
    config::{Endpoints, MAX_CONFIG, RemoteConfig},
    operation::RemoteOperation,
    store::RemoteStore,
};
use crate::hosted::{
    Hosted, HostedConfig, HostedResult, Role, cli, remote_gate::GateError, store::valid_slug,
};

const HELP: &str = "meibo hosted-remote CONFIG COMMAND

Commands:
  init
  tenant-add SLUG [--origin real|synthetic] [--demo-rows 0..100000]
  user-add USER --password-stdin
  password-set USER --password-stdin
  user-disable USER
  member-set USER SLUG viewer|editor|admin|none
  revoke USER
  audit [SLUG]
  sync-nta SLUG CATALOG [--limit COUNT]
  backup NEW_DIRECTORY
  restore SNAPSHOT_DIRECTORY --target-config TARGET_CONFIG
  gate-status
  gate-recover OWNER --prior-work-terminated
  serve --origin https://domain [--bind 127.0.0.1:4317]

CONFIG contains environment variable references only. Passwords are read from stdin.
HTTP frontend testing requires explicit loopback origin and bind.
";

enum Action {
    Init,
    Tenant {
        slug: String,
        origin: DataOrigin,
        rows: u64,
    },
    AddUser(String, String),
    Password(String, String),
    Disable(String),
    Member(String, String, Option<Role>),
    Revoke(String),
    Audit(Option<String>),
    Sync(String, PathBuf, Option<u64>),
    Backup(PathBuf),
    Restore(PathBuf, PathBuf),
    Status,
    Recover(String),
    Serve(HostedConfig),
}

fn parse(command: &str, args: &[String]) -> HostedResult<Action> {
    match (command, args) {
        ("init", []) => Ok(Action::Init),
        ("tenant-add", [slug, options @ ..]) => {
            let mut origin = DataOrigin::Real;
            let mut rows = 0;
            let mut seen = std::collections::BTreeSet::new();
            let (pairs, remainder) = options.as_chunks::<2>();
            for pair in pairs {
                if !seen.insert(pair[0].as_str()) {
                    return Err("tenant-addの引数を確認してください。".into());
                }
                match pair[0].as_str() {
                    "--origin" => {
                        origin = match pair[1].as_str() {
                            "real" => DataOrigin::Real,
                            "synthetic" => DataOrigin::Synthetic,
                            _ => return Err("originを確認してください。".into()),
                        }
                    }
                    "--demo-rows" => {
                        rows = pair[1]
                            .parse::<u64>()
                            .map_err(|_| "合成件数を確認してください。")?;
                    }
                    _ => return Err("tenant-addの引数を確認してください。".into()),
                }
            }
            if !remainder.is_empty()
                || !valid_slug(slug)
                || rows > 100_000
                || (rows > 0 && origin != DataOrigin::Synthetic)
            {
                return Err("組織ID・データ種別・合成件数を確認してください。".into());
            }
            Ok(Action::Tenant {
                slug: slug.clone(),
                origin,
                rows,
            })
        }
        ("user-add", [name, flag]) if flag == "--password-stdin" => {
            Ok(Action::AddUser(name.clone(), cli::password()?))
        }
        ("password-set", [name, flag]) if flag == "--password-stdin" => {
            Ok(Action::Password(name.clone(), cli::password()?))
        }
        ("user-disable", [name]) => Ok(Action::Disable(name.clone())),
        ("member-set", [name, tenant, role]) => {
            let role = if role == "none" {
                None
            } else {
                Some(Role::parse(role).ok_or("権限を確認してください。")?)
            };
            Ok(Action::Member(name.clone(), tenant.clone(), role))
        }
        ("revoke", [name]) => Ok(Action::Revoke(name.clone())),
        ("audit", []) => Ok(Action::Audit(None)),
        ("audit", [tenant]) => Ok(Action::Audit(Some(tenant.clone()))),
        ("sync-nta", [tenant, catalog, options @ ..]) => {
            let limit = match options {
                [] => None,
                [flag, value] if flag == "--limit" => Some(
                    value
                        .parse::<u64>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("limitは正整数で指定してください。")?,
                ),
                _ => return Err("sync-ntaの引数を確認してください。".into()),
            };
            Ok(Action::Sync(tenant.clone(), catalog.into(), limit))
        }
        ("gate-status", []) => Ok(Action::Status),
        ("backup", [path]) => Ok(Action::Backup(path.into())),
        ("restore", [snapshot, flag, target]) if flag == "--target-config" => {
            Ok(Action::Restore(snapshot.into(), target.into()))
        }
        ("gate-recover", [owner, flag])
            if flag == "--prior-work-terminated"
                && owner.len() == 64
                && owner
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            Ok(Action::Recover(owner.clone()))
        }
        ("serve", options) => Ok(Action::Serve(cli::serve_config(options)?)),
        _ => Err("hosted-remote --helpで引数を確認してください。".into()),
    }
}

fn read_config(path: &Path) -> HostedResult<Endpoints> {
    crate::hosted::store::regular_file(path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_CONFIG + 1) as u64)
        .read_to_end(&mut bytes)?;
    let config = RemoteConfig::parse(&bytes)?;
    #[cfg(feature = "integration-tests")]
    if std::env::var("MEIBO_TEST_REMOTE_LOOPBACK").as_deref() == Ok("1") {
        return config.resolve_with(
            |name| std::env::var(name).ok(),
            super::config::EndpointPolicy::LoopbackTests,
        );
    }
    config.resolve()
}

pub(super) fn run(args: Vec<OsString>) -> HostedResult<()> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help") {
        print!("{HELP}");
        return Ok(());
    }
    let mut args = args.into_iter();
    let path = PathBuf::from(args.next().ok_or("CONFIGが必要です。")?);
    let command = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .ok_or("COMMANDが必要です。")?;
    let rest = args
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "UTF-8で指定してください。".into())
        })
        .collect::<HostedResult<Vec<String>>>()?;
    let action = parse(&command, &rest)?;
    let endpoints = read_config(&path)?;
    if matches!(action, Action::Init) {
        RemoteStore::initialize(endpoints)?;
        println!("initialized");
        return Ok(());
    }
    let mut store = RemoteStore::open(endpoints)?;
    if let Action::Serve(config) = action {
        return Hosted::new(store, config).serve();
    }
    if matches!(action, Action::Status) {
        let status = store.gate_status()?;
        println!("{}", status.map_or_else(|| json!({"occupied":false}), |status| json!({"occupied":true,"owner":status.owner_hex(),"at":status.at,"operation":status.operation})));
        return Ok(());
    }
    if let Action::Recover(owner) = action {
        let observed = store.gate_status()?.ok_or(GateError::OwnerMismatch)?;
        if observed.owner_hex() != owner {
            store.audit("maintenance", None, "gate.recover", "owner-mismatch")?;
            return Err(GateError::OwnerMismatch.into());
        }
        store.recover_gate(&observed, true)?;
        println!("gate-recovered");
        return Ok(());
    }
    let mut operation = RemoteOperation::begin(&store, &format!("cli.{command}"))?;
    let result = execute(&mut store, &mut operation, action);
    let output = operation.finish(&store, result)?;
    if !output.is_empty() {
        println!("{output}");
    }
    std::io::stdout().flush()?;
    Ok(())
}

fn execute(
    store: &mut RemoteStore,
    operation: &mut RemoteOperation,
    action: Action,
) -> HostedResult<String> {
    match action {
        Action::Tenant { slug, origin, rows } => {
            if rows == 0 {
                operation.construction(store.add_empty_tenant(&slug, origin))?;
                return Ok("tenant-created".into());
            }
            let (identity, mut engine) =
                operation.construction(store.create_tenant(&slug, origin))?;
            let seeded = engine.load_synthetic(SynthConfig {
                rows,
                seed: 42,
                profile: Profile::Dense,
            });
            operation.observe(&engine);
            seeded?;
            drop(engine);
            operation.construction(store.register_tenant(&slug, &identity))?;
            Ok("tenant-created".into())
        }
        Action::AddUser(name, password) => {
            store.add_user(&name, &password)?;
            Ok("user-created".into())
        }
        Action::Password(name, password) => {
            store.set_password(&name, &password)?;
            Ok("password-updated".into())
        }
        Action::Disable(name) => {
            store.disable_user(&name)?;
            Ok("user-disabled".into())
        }
        Action::Member(name, tenant, role) => {
            operation.construction(store.set_member(&name, &tenant, role))?;
            Ok("membership-updated".into())
        }
        Action::Revoke(name) => {
            store.revoke_user(&name)?;
            Ok("sessions-revoked".into())
        }
        Action::Audit(tenant) => Ok(store
            .audit_rows(tenant.as_deref(), 200)?
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")),
        Action::Sync(tenant, catalog, limit) => {
            let mut engine = operation.construction(store.open_tenant(&tenant))?;
            let result = meibo_import::sync_nta(&mut engine, &catalog, limit);
            operation.observe(&engine);
            if !engine.is_poisoned() && !store.conn.is_poisoned() {
                store.audit(
                    "operator",
                    Some(&tenant),
                    "sync-nta",
                    if result.is_ok() { "ok" } else { "failed" },
                )?;
            }
            let report = result?;
            Ok(format!(
                "inserted={} updated={} skipped={}",
                report.inserted, report.updated, report.skipped
            ))
        }
        Action::Backup(path) => {
            crate::hosted::remote_backup::create(store, operation, &path)?;
            Ok("backed-up".into())
        }
        Action::Restore(path, target) => {
            crate::hosted::remote_backup::restore(store, operation, &path, read_config(&target)?)?;
            Ok("restored".into())
        }
        Action::Init | Action::Status | Action::Recover(_) | Action::Serve(_) => unreachable!(
            "bootstrap/status/server actions were handled before acquiring a work gate"
        ),
    }
}
