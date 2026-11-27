//! Cloud Run に配備した遠隔保存版へ管理CLIの命令を実行する(`cargo xtask cloud-run-admin`)。
//!
//! 配備の設定から接続先と鍵の参照を読む。鍵は Secret Manager から取り出して子プロセスの
//! 環境変数にだけ渡し、画面・ファイル・エラー文には出さない。ビルドは鍵を読む前に行う。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use crate::Result;

const USAGE: &str = "使い方: cargo xtask cloud-run-admin \
(--project P --region R --service S | --describe FILE [--project P]) \
[--meibo BINARY] [--target-describe FILE] -- COMMAND [ARGS...]
COMMAND は `meibo hosted-remote CONFIG` の管理命令(serve を除く)。";
const CONFIG_ENV: &str = "MEIBO_REMOTE_CONFIG";

#[derive(Debug, PartialEq, Eq)]
enum Source {
    Service { region: String, service: String },
    File(PathBuf),
}

#[derive(Debug)]
struct Options {
    source: Source,
    project: Option<String>,
    target: Option<PathBuf>,
    meibo: Option<PathBuf>,
    command: Vec<String>,
}

/// 配備の1つの環境変数。値そのものか、Secret Manager の参照。
#[derive(Debug)]
enum Setting {
    Plain(String),
    Secret { name: String, version: String },
}

/// 管理CLIへ渡す設定(環境変数名だけ)と、その値。値は表示しないので `Debug` を持たない。
struct Resolved {
    config: String,
    env: Vec<(String, String)>,
}

/// 作業中だけ置く、本人だけが読める設定ファイル。
struct Private(PathBuf);

impl Private {
    fn write(directory: &Path, label: &str, content: &str) -> Result<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = directory.join(format!(
            "meibo-admin-{}-{nanos}-{label}.json",
            std::process::id()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path)?;
        let guard = Self(path);
        file.write_all(content.as_bytes())?;
        Ok(guard)
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn parse(args: &[String]) -> Result<Options> {
    let split = args.iter().position(|arg| arg == "--").ok_or(USAGE)?;
    let command = args[split + 1..].to_vec();
    let mut values = BTreeMap::new();
    let mut flags = args[..split].iter();
    while let Some(flag) = flags.next() {
        let known = [
            "--project",
            "--region",
            "--service",
            "--describe",
            "--target-describe",
            "--meibo",
        ];
        let value = flags.next().ok_or(USAGE)?;
        if !known.contains(&flag.as_str())
            || value.starts_with('-')
            || values.insert(flag.as_str(), value.clone()).is_some()
        {
            return Err(USAGE.into());
        }
    }
    match command.first().map(String::as_str) {
        None => return Err(USAGE.into()),
        Some("serve") => {
            return Err("serve は配備先で動かします。管理命令を指定してください。".into());
        }
        Some(_) => {}
    }
    let target = values.remove("--target-describe").map(PathBuf::from);
    if target.is_some()
        && (command[0] != "restore" || command.iter().any(|arg| arg == "--target-config"))
    {
        return Err(
            "--target-describe は --target-config を付けない restore でだけ使えます。".into(),
        );
    }
    let source = match (
        values.remove("--describe"),
        values.remove("--region"),
        values.remove("--service"),
    ) {
        (Some(file), None, None) => Source::File(file.into()),
        (None, Some(region), Some(service)) => Source::Service { region, service },
        _ => return Err(USAGE.into()),
    };
    let project = values.remove("--project");
    if matches!(source, Source::Service { .. }) && project.is_none() {
        return Err(USAGE.into());
    }
    Ok(Options {
        source,
        project,
        target,
        meibo: values.remove("--meibo").map(PathBuf::from),
        command,
    })
}

/// `gcloud run services describe --format=json` の形から環境変数を読む。
fn deployment(json: &[u8]) -> Result<BTreeMap<String, Setting>> {
    let value: Value =
        serde_json::from_slice(json).map_err(|_| "配備の設定をJSONとして読めません。")?;
    let items = value
        .pointer("/spec/template/spec/containers/0/env")
        .and_then(Value::as_array)
        .ok_or("配備の設定に環境変数の一覧がありません。")?;
    let mut settings = BTreeMap::new();
    for item in items {
        let name = item["name"]
            .as_str()
            .ok_or("配備の環境変数の名前を確認してください。")?;
        let reference = &item["valueFrom"]["secretKeyRef"];
        let setting = if let Some(value) = item["value"].as_str() {
            Setting::Plain(value.to_owned())
        } else if let (Some(secret), Some(version)) =
            (reference["name"].as_str(), reference["key"].as_str())
        {
            Setting::Secret {
                name: secret.to_owned(),
                version: version.to_owned(),
            }
        } else {
            return Err(format!("{name} の渡し方に対応していません。").into());
        };
        settings.insert(name.to_owned(), setting);
    }
    Ok(settings)
}

/// 遠隔保存の設定が参照する環境変数名。ローダーなどの変数を上書きしない名前に限る。
fn references(config: &str) -> Result<Vec<String>> {
    const INVALID: &str = "遠隔保存の設定を確認してください。";
    let value: Value = serde_json::from_str(config).map_err(|_| INVALID)?;
    let tenants = value["tenants"].as_object().ok_or(INVALID)?;
    let mut names = Vec::new();
    for item in std::iter::once(&value["control"]).chain(tenants.values()) {
        for key in ["url_env", "token_env"] {
            let name = item[key].as_str().ok_or(INVALID)?;
            let allowed = name.strip_prefix("MEIBO_DB_").is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            });
            if !allowed {
                return Err(INVALID.into());
            }
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn resolve(
    settings: &BTreeMap<String, Setting>,
    mut fetch: impl FnMut(&str, &str) -> Result<String>,
) -> Result<Resolved> {
    let Some(Setting::Plain(config)) = settings.get(CONFIG_ENV) else {
        return Err(format!("配備の設定に {CONFIG_ENV} がありません。").into());
    };
    let mut env = Vec::new();
    for name in references(config)? {
        let value = match settings.get(&name) {
            Some(Setting::Plain(value)) => value.clone(),
            Some(Setting::Secret {
                name: secret,
                version,
            }) => fetch(secret, version)?,
            None => return Err(format!("{name} が配備の設定にありません。").into()),
        };
        env.push((name, value));
    }
    Ok(Resolved {
        config: config.clone(),
        env,
    })
}

fn gcloud(args: &[&str]) -> Result<std::process::Output> {
    Command::new("gcloud")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "gcloud を実行できません。Google Cloud CLI を確認してください。".into())
}

/// 値は標準出力で受け取るだけで、どこにも書かない。
fn secret(project: &str, name: &str, version: &str) -> Result<String> {
    let output = gcloud(&[
        "secrets",
        "versions",
        "access",
        version,
        "--secret",
        name,
        "--project",
        project,
    ])?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Secret Manager から {name} を取り出せません: {}",
            reason.trim()
        )
        .into());
    }
    let mut value =
        String::from_utf8(output.stdout).map_err(|_| format!("{name} の値を読めません。"))?;
    if value.ends_with('\n') {
        value.pop();
    }
    Ok(value)
}

fn load(source: &Source, project: Option<&str>) -> Result<Resolved> {
    let json = match source {
        Source::File(path) => fs::read(path)?,
        Source::Service { region, service } => {
            let project = project.ok_or(USAGE)?;
            let output = gcloud(&[
                "run",
                "services",
                "describe",
                service,
                "--region",
                region,
                "--project",
                project,
                "--format",
                "json",
            ])?;
            if !output.status.success() {
                let reason = String::from_utf8_lossy(&output.stderr);
                return Err(format!("配備の設定を読めません: {}", reason.trim()).into());
            }
            output.stdout
        }
    };
    resolve(&deployment(&json)?, |name, version| {
        let project = project.ok_or("秘密を取り出すには --project が必要です。")?;
        secret(project, name, version)
    })
}

/// 復旧では元と先を同じ環境に置くので、参照名が重なると値が取り違わる。
fn disjoint(source: &Resolved, target: &Resolved) -> Result<()> {
    let names: BTreeSet<&str> = source.env.iter().map(|(name, _)| name.as_str()).collect();
    match target
        .env
        .iter()
        .find(|(name, _)| names.contains(name.as_str()))
    {
        Some((name, _)) => Err(format!("元と先で同じ環境変数名 {name} は使えません。").into()),
        None => Ok(()),
    }
}

fn build(root: &Path) -> Result<PathBuf> {
    let status = crate::command(
        root,
        "cargo",
        &[
            "build",
            "--locked",
            "--release",
            "-p",
            "meibo-app",
            "--features",
            "cloud",
        ],
    )
    .stdin(Stdio::null())
    .status()?;
    if !status.success() {
        return Err("meibo のビルドに失敗しました。".into());
    }
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    Ok(target.join("release/meibo"))
}

pub(crate) fn run(root: &Path, args: &[String], temp_dir: &Path) -> Result<i32> {
    let options = parse(args)?;
    // The build must not see any credential, so it happens before they are read.
    let binary = match &options.meibo {
        Some(path) => path.clone(),
        None => build(root)?,
    };
    // The admin CLI rejects paths through symbolic links (macOS temp folders).
    let temp = fs::canonicalize(temp_dir)?;
    let mut source = load(&options.source, options.project.as_deref())?;
    let source_file = Private::write(&temp, "source", &source.config)?;
    let mut command_args = options.command.clone();
    let mut target_file = None;
    if let Some(path) = &options.target {
        let target = load(&Source::File(path.clone()), options.project.as_deref())?;
        disjoint(&source, &target)?;
        let file = Private::write(&temp, "target", &target.config)?;
        let name = file
            .0
            .to_str()
            .ok_or("一時ファイルの場所を確認してください。")?;
        command_args.extend(["--target-config".to_owned(), name.to_owned()]);
        source.env.extend(target.env);
        target_file = Some(file);
    }
    let status = Command::new(&binary)
        .arg("hosted-remote")
        .arg(&source_file.0)
        .args(&command_args)
        .envs(source.env)
        .status()
        .map_err(|_| "meibo を実行できません。--meibo の場所を確認してください。")?;
    drop(target_file);
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    const CONFIG: &str = r#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"}}}"#;

    fn service(extra: &[Value]) -> Vec<u8> {
        let mut env = vec![serde_json::json!({"name": CONFIG_ENV, "value": CONFIG})];
        env.extend_from_slice(extra);
        serde_json::to_vec(
            &serde_json::json!({"spec":{"template":{"spec":{"containers":[{"env":env}]}}}}),
        )
        .unwrap()
    }

    fn plain(name: &str, value: &str) -> Value {
        serde_json::json!({"name": name, "value": value})
    }

    fn referenced(name: &str, secret: &str) -> Value {
        serde_json::json!({"name": name, "valueFrom":{"secretKeyRef":{"name": secret, "key":"latest"}}})
    }

    #[test]
    fn arguments_need_one_source_and_an_admin_command() {
        let options = parse(&args(&[
            "--project",
            "p",
            "--region",
            "r",
            "--service",
            "s",
            "--",
            "backup",
            "/tmp/x",
        ]))
        .unwrap();
        assert_eq!(
            options.source,
            Source::Service {
                region: "r".into(),
                service: "s".into()
            }
        );
        assert_eq!(options.command, args(&["backup", "/tmp/x"]));
        for bad in [
            &["--describe", "f"][..],
            &["--describe", "f", "--"],
            &[
                "--describe",
                "f",
                "--",
                "serve",
                "--origin",
                "https://a.test",
            ],
            &["--region", "r", "--service", "s", "--", "gate-status"],
            &["--describe", "f", "--service", "s", "--", "gate-status"],
            &["--describe", "f", "--unknown", "x", "--", "gate-status"],
            &[
                "--describe",
                "f",
                "--target-describe",
                "t",
                "--",
                "backup",
                "d",
            ],
            &[
                "--describe",
                "f",
                "--target-describe",
                "t",
                "--",
                "restore",
                "d",
                "--target-config",
                "c",
            ],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
        assert!(
            parse(&args(&[
                "--describe",
                "f",
                "--target-describe",
                "t",
                "--",
                "restore",
                "d",
            ]))
            .is_ok()
        );
    }

    #[test]
    fn secrets_are_fetched_only_for_referenced_names() {
        let json = service(&[
            plain("MEIBO_DB_CONTROL_URL", "libsql://control.invalid"),
            referenced("MEIBO_DB_CONTROL_TOKEN", "control-token"),
            plain("MEIBO_DB_ALPHA_URL", "libsql://alpha.invalid"),
            referenced("MEIBO_DB_ALPHA_TOKEN", "alpha-token"),
            referenced("UNRELATED", "unrelated-secret"),
        ]);
        let mut asked = Vec::new();
        let resolved = resolve(&deployment(&json).unwrap(), |name, version| {
            asked.push(format!("{name}:{version}"));
            Ok(format!("value-of-{name}"))
        })
        .unwrap();
        assert_eq!(asked, ["control-token:latest", "alpha-token:latest"]);
        assert_eq!(resolved.env.len(), 4);
        assert!(
            resolved
                .env
                .contains(&("MEIBO_DB_ALPHA_TOKEN".into(), "value-of-alpha-token".into()))
        );
        assert!(!resolved.config.contains("value-of"));
    }

    #[test]
    fn a_missing_or_foreign_reference_is_refused_without_values() {
        let json = service(&[plain("MEIBO_DB_CONTROL_URL", "libsql://private.invalid")]);
        let error = resolve(&deployment(&json).unwrap(), |_, _| Ok(String::new()))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("MEIBO_DB_CONTROL_TOKEN"));
        assert!(!error.contains("private.invalid"));
        let foreign = CONFIG.replace("MEIBO_DB_ALPHA_TOKEN", "PATH");
        assert!(references(&foreign).is_err());
    }

    #[test]
    fn restore_source_and_target_names_must_differ() {
        let side = |name: &str| Resolved {
            config: String::new(),
            env: vec![(name.to_owned(), "value".to_owned())],
        };
        assert!(disjoint(&side("MEIBO_DB_A"), &side("MEIBO_DB_B")).is_ok());
        assert!(disjoint(&side("MEIBO_DB_A"), &side("MEIBO_DB_A")).is_err());
    }

    #[test]
    fn the_temporary_config_is_private_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let file = Private::write(directory.path(), "source", CONFIG).unwrap();
        let path = file.0.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(file);
        assert!(!path.exists());
    }
}
