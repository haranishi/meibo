use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
};

use meibo_core::types::DataOrigin;

use super::{Hosted, HostedConfig, HostedResult, Role, Store, backup};

const HELP: &str = "meibo hosted ROOT COMMAND

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
  restore SNAPSHOT_DIRECTORY
  serve --origin https://domain [--bind 127.0.0.1:4317]

restore writes to ROOT, which must not exist. Passwords are read from stdin.
HTTP is allowed only with --allow-http-loopback and loopback origin + bind.
";

fn password() -> HostedResult<String> {
    let mut input = String::new();
    std::io::stdin().take(1026).read_to_string(&mut input)?;
    if input.ends_with('\n') {
        input.pop();
        if input.ends_with('\r') {
            input.pop();
        }
    }
    if input.len() > 1024 || input.contains(['\n', '\r']) {
        return Err("passwordは1行、1024bytes以内で指定してください。".into());
    }
    Ok(input)
}

pub(super) fn run(args: Vec<OsString>) -> HostedResult<()> {
    if args.is_empty() || args.iter().any(|s| s == "--help") {
        print!("{HELP}");
        return Ok(());
    }
    let mut args = args.into_iter();
    let root = PathBuf::from(args.next().ok_or("ROOTが必要です。")?);
    let command = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or("COMMANDが必要です。")?;
    let rest = args
        .map(|s| {
            s.into_string()
                .map_err(|_| "UTF-8で指定してください。".into())
        })
        .collect::<HostedResult<Vec<String>>>()?;
    if command == "init" && rest.is_empty() {
        Store::initialize(&root)?;
        println!("initialized");
        return Ok(());
    }
    if command == "restore" && rest.len() == 1 {
        backup::restore(std::path::Path::new(&rest[0]), &root)?;
        println!("restored");
        return Ok(());
    }
    let mut store = Store::open(&root)?;
    if command == "serve" {
        return Hosted::new(store, serve_config(&rest)?).serve();
    }
    if command == "backup" && rest.len() == 1 {
        backup::create(&store, std::path::Path::new(&rest[0]))?;
        println!("backed-up");
        return Ok(());
    }
    let _guard = store.lock()?;
    match (command.as_str(), rest.as_slice()) {
        ("tenant-add", [slug, options @ ..]) => {
            let mut origin = DataOrigin::Real;
            let mut rows = 0;
            let mut i = 0;
            while i < options.len() {
                let value = options.get(i + 1).ok_or("optionの値が必要です。")?;
                match options[i].as_str() {
                    "--origin" => {
                        origin = match value.as_str() {
                            "real" => DataOrigin::Real,
                            "synthetic" => DataOrigin::Synthetic,
                            _ => return Err("originを確認してください。".into()),
                        }
                    }
                    "--demo-rows" => {
                        rows = value.parse().map_err(|_| "合成件数を確認してください。")?;
                    }
                    _ => return Err("tenant-addの引数を確認してください。".into()),
                }
                i += 2;
            }
            store.add_tenant(slug, origin, rows)?;
            println!("tenant-created");
        }
        ("user-add", [name, flag]) if flag == "--password-stdin" => {
            store.add_user(name, &password()?)?;
            println!("user-created");
        }
        ("password-set", [name, flag]) if flag == "--password-stdin" => {
            store.set_password(name, &password()?)?;
            println!("password-updated");
        }
        ("user-disable", [name]) => {
            store.disable_user(name)?;
            println!("user-disabled");
        }
        ("member-set", [name, tenant, role]) => {
            let role = if role == "none" {
                None
            } else {
                Some(Role::parse(role).ok_or("権限を確認してください。")?)
            };
            store.set_member(name, tenant, role)?;
            println!("membership-updated");
        }
        ("revoke", [name]) => {
            store.revoke_user(name)?;
            println!("sessions-revoked");
        }
        ("sync-nta", [tenant, catalog, options @ ..]) => {
            sync_nta(&store, tenant, catalog, options)?;
        }
        ("audit", []) => print_audit(&store, None)?,
        ("audit", [tenant]) => print_audit(&store, Some(tenant))?,
        _ => return Err("hosted --helpで引数を確認してください。".into()),
    }
    std::io::stdout().flush()?;
    Ok(())
}

fn print_audit(store: &Store, tenant: Option<&str>) -> HostedResult<()> {
    for row in store.audit_rows(tenant, 200)? {
        println!("{row}");
    }
    Ok(())
}

fn sync_nta(store: &Store, tenant: &str, catalog: &str, options: &[String]) -> HostedResult<()> {
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
    let path = store.tenant_path(tenant)?;
    let mut engine = meibo_core::Engine::open(&path, &super::store::engine_options())?;
    let result = meibo_import::sync_nta(&mut engine, std::path::Path::new(catalog), limit);
    store.audit(
        "operator",
        Some(tenant),
        "sync-nta",
        if result.is_ok() { "ok" } else { "failed" },
    )?;
    let report = result?;
    println!(
        "inserted={} updated={} skipped={}",
        report.inserted, report.updated, report.skipped
    );
    Ok(())
}

fn serve_config(rest: &[String]) -> HostedResult<HostedConfig> {
    let mut origin = None;
    let mut bind = "127.0.0.1:4317".to_owned();
    let mut development = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--origin" if origin.is_none() => {
                i += 1;
                origin = Some(rest.get(i).ok_or("originが必要です。")?.clone());
            }
            "--bind" => {
                i += 1;
                bind.clone_from(rest.get(i).ok_or("bindが必要です。")?);
            }
            "--allow-http-loopback" if !development => development = true,
            _ => return Err("serveの引数を確認してください。".into()),
        }
        i += 1;
    }
    HostedConfig::parse(&origin.ok_or("--originが必要です。")?, &bind, development)
}
