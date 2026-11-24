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
  user-add USER --cloudflare-access (cloud build)
  password-set USER --password-stdin
  user-disable USER
  member-set USER SLUG viewer|editor|admin|none
  revoke USER
  audit [SLUG]
  sync-nta SLUG CATALOG [--limit COUNT]
  backup NEW_DIRECTORY
  restore SNAPSHOT_DIRECTORY
  serve --origin https://domain [--bind 127.0.0.1:4317] [--access-config PRIVATE_JSON] [--public-demo] [--demo-contact URL] [--demo-idle-seconds N] [--demo-max-uptime-seconds N] [--demo-stall-seconds N]

restore writes to ROOT, which must not exist. Passwords are read from stdin.
HTTP is allowed only with --allow-http-loopback and loopback origin + bind.
";

pub(super) fn password() -> HostedResult<String> {
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
    if command == "serve" {
        let config = serve_config(&rest)?;
        let store = if config.demo.is_some() {
            super::demo::bootstrap(&root, super::now()?)?
        } else {
            Store::open(&root)?
        };
        return Hosted::new(store, config).serve();
    }
    let mut store = Store::open(&root)?;
    if command == "backup" && rest.len() == 1 {
        backup::create(&store, std::path::Path::new(&rest[0]))?;
        println!("backed-up");
        return Ok(());
    }
    let _guard = store.lock()?;
    match (command.as_str(), rest.as_slice()) {
        ("tenant-add", [slug, options @ ..]) => {
            add_tenant(&mut store, slug, options)?;
            println!("tenant-created");
        }
        ("user-add", [name, flag]) if flag == "--password-stdin" => {
            store.add_user(name, &password()?)?;
            println!("user-created");
        }
        #[cfg(feature = "cloud")]
        ("user-add", [name, flag]) if flag == "--cloudflare-access" => {
            store.add_user(name, &super::auth::random_hex(32)?)?;
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

fn add_tenant(store: &mut Store, slug: &str, options: &[String]) -> HostedResult<()> {
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
    store.add_tenant(slug, origin, rows)
}

pub(super) fn print_audit(
    store: &super::control::ControlStore,
    tenant: Option<&str>,
) -> HostedResult<()> {
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

pub(super) fn serve_config(rest: &[String]) -> HostedResult<HostedConfig> {
    let mut origin = None;
    let mut bind = "127.0.0.1:4317".to_owned();
    let mut development = false;
    let mut access_config = None;
    let mut public_demo = false;
    let mut demo_contact = None;
    let mut demo_idle_seconds = None;
    let mut demo_max_uptime_seconds = None;
    let mut demo_stall_seconds = None;
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
            "--access-config" if access_config.is_none() => {
                i += 1;
                access_config = Some(std::path::PathBuf::from(
                    rest.get(i).ok_or("Access設定が必要です。")?,
                ));
            }
            "--public-demo" if !public_demo => public_demo = true,
            "--demo-contact" if demo_contact.is_none() => {
                i += 1;
                demo_contact = Some(rest.get(i).ok_or("デモの連絡先URLが必要です。")?.clone());
            }
            "--demo-idle-seconds" if demo_idle_seconds.is_none() => {
                i += 1;
                demo_idle_seconds = Some(
                    rest.get(i)
                        .ok_or("--demo-idle-secondsの秒数が必要です。")?
                        .clone(),
                );
            }
            "--demo-max-uptime-seconds" if demo_max_uptime_seconds.is_none() => {
                i += 1;
                demo_max_uptime_seconds = Some(
                    rest.get(i)
                        .ok_or("--demo-max-uptime-secondsの秒数が必要です。")?
                        .clone(),
                );
            }
            "--demo-stall-seconds" if demo_stall_seconds.is_none() => {
                i += 1;
                demo_stall_seconds = Some(
                    rest.get(i)
                        .ok_or("--demo-stall-secondsの秒数が必要です。")?
                        .clone(),
                );
            }
            _ => return Err("serveの引数を確認してください。".into()),
        }
        i += 1;
    }
    if public_demo && access_config.is_some() {
        return Err("公開デモでは--access-configを併用できません。".into());
    }
    let demo = build_demo_config(
        public_demo,
        demo_contact,
        demo_idle_seconds,
        demo_max_uptime_seconds,
        demo_stall_seconds,
    )?;
    let mut config =
        HostedConfig::parse(&origin.ok_or("--originが必要です。")?, &bind, development)?;
    config.demo = demo;
    match access_config {
        None => Ok(config),
        Some(path) => {
            #[cfg(feature = "cloud")]
            {
                config.access = Some(super::access::Access::new(super::access::Config::load(
                    &path,
                )?)?);
                Ok(config)
            }
            #[cfg(not(feature = "cloud"))]
            {
                let _ = path;
                Err("Cloudflareログインにはcloud buildが必要です。".into())
            }
        }
    }
}

fn build_demo_config(
    public_demo: bool,
    contact: Option<String>,
    idle_seconds: Option<String>,
    max_uptime_seconds: Option<String>,
    stall_seconds: Option<String>,
) -> HostedResult<Option<super::demo::DemoConfig>> {
    if !public_demo {
        for (present, flag) in [
            (contact.is_some(), "--demo-contact"),
            (idle_seconds.is_some(), "--demo-idle-seconds"),
            (max_uptime_seconds.is_some(), "--demo-max-uptime-seconds"),
            (stall_seconds.is_some(), "--demo-stall-seconds"),
        ] {
            if present {
                return Err(format!("{flag}は--public-demoと併用してください。").into());
            }
        }
        return Ok(None);
    }
    let mut demo = super::demo::DemoConfig::new(contact)?;
    if let Some(raw) = idle_seconds {
        demo.idle_timeout = super::demo::parse_seconds(
            &raw,
            "--demo-idle-seconds",
            super::demo::MIN_IDLE_SECONDS,
            super::demo::MAX_IDLE_SECONDS,
        )?;
    }
    if let Some(raw) = max_uptime_seconds {
        demo.max_uptime = super::demo::parse_seconds(
            &raw,
            "--demo-max-uptime-seconds",
            super::demo::MIN_MAX_UPTIME_SECONDS,
            super::demo::MAX_MAX_UPTIME_SECONDS,
        )?;
    }
    if let Some(raw) = stall_seconds {
        demo.stall_timeout = super::demo::parse_seconds(
            &raw,
            "--demo-stall-seconds",
            super::demo::MIN_STALL_SECONDS,
            super::demo::MAX_STALL_SECONDS,
        )?;
    }
    Ok(Some(demo))
}
