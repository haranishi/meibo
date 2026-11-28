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

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

use crate::Result;

const USAGE: &str = "使い方: cargo xtask cloud-run-admin \
(--project P --region R --service S [--revision NAME] | --describe FILE [--project P]) \
[--meibo BINARY] [--target-describe FILE [--target-project P]] -- COMMAND [ARGS...]
COMMAND は `meibo hosted-remote CONFIG` の管理命令(serve を除く)。";
const CONFIG_ENV: &str = "MEIBO_REMOTE_CONFIG";

#[derive(Debug, PartialEq, Eq)]
enum Source {
    Service {
        region: String,
        service: String,
        revision: Option<String>,
    },
    File(PathBuf),
}

#[derive(Debug)]
struct Options {
    source: Source,
    project: Option<String>,
    target: Option<PathBuf>,
    target_project: Option<String>,
    meibo: Option<PathBuf>,
    command: Vec<String>,
}

/// 配備の1つの環境変数。値そのものか、Secret Manager の参照。
enum Setting {
    Plain(String),
    Secret { name: String, version: String },
}

/// 管理CLIへ渡す設定(環境変数名だけ)と、その値。値は表示しないので `Debug` を持たない。
struct Resolved {
    config: ReferenceConfig,
    env: Vec<(String, String)>,
    secrets: BTreeSet<String>,
    endpoints: BTreeSet<String>,
}

/// 作業中だけ置く、本人だけが読める設定ファイル。
struct Private(PathBuf);

impl Private {
    fn write(directory: &Path, label: &str, config: &ReferenceConfig) -> Result<Self> {
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
        serde_json::to_writer(&mut file, config)?;
        file.flush()?;
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
            "--revision",
            "--describe",
            "--target-describe",
            "--target-project",
            "--meibo",
        ];
        let value = flags.next().ok_or(USAGE)?;
        if !known.contains(&flag.as_str())
            || value.is_empty()
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
    let target_project = values.remove("--target-project");
    if target_project.is_some() && target.is_none() {
        return Err("--target-project には --target-describe が必要です。".into());
    }
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
        values.remove("--revision"),
    ) {
        (Some(file), None, None, None) => Source::File(file.into()),
        (None, Some(region), Some(service), revision) => Source::Service {
            region,
            service,
            revision,
        },
        _ => return Err(USAGE.into()),
    };
    let project = values.remove("--project");
    if matches!(source, Source::Service { .. }) && project.is_none() {
        return Err(USAGE.into());
    }
    Ok(Options {
        source,
        target_project: target_project.or_else(|| project.clone()),
        project,
        target,
        meibo: values.remove("--meibo").map(PathBuf::from),
        command,
    })
}

const REVISION_REQUIRED: &str = "最新の作成版・準備済みの版・通信先の一致を確認できません。--service S --revision NAME で対象の版を指定してください。";

fn latest_serves_all(value: &Value) -> bool {
    let status = &value["status"];
    let Some(latest) = status["latestCreatedRevisionName"]
        .as_str()
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    if status["latestReadyRevisionName"].as_str() != Some(latest)
        || value
            .pointer("/metadata/generation")
            .is_some_and(|generation| Some(generation) != status.get("observedGeneration"))
        || value
            .pointer("/spec/template/metadata/name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty() && name != latest)
        || status["conditions"].as_array().is_some_and(|conditions| {
            conditions
                .iter()
                .any(|condition| condition["type"] == "Ready" && condition["status"] != "True")
        })
    {
        return false;
    }
    let Some(traffic) = status["traffic"].as_array() else {
        return false;
    };
    let mut total = 0_u64;
    for entry in traffic {
        // A tagged URL may be listed with no share of the service's traffic.
        let percent = match entry.get("percent") {
            Some(value) => value.as_u64(),
            None if entry.get("tag").and_then(Value::as_str).is_some() => Some(0),
            None => None,
        };
        let Some(percent @ 0..=100) = percent else {
            return false;
        };
        if percent > 0 && entry["revisionName"].as_str() != Some(latest) {
            return false;
        }
        total += percent;
    }
    total == 100
}

fn deployment(json: &[u8], source: &Source) -> Result<BTreeMap<String, Setting>> {
    let value: Value =
        serde_json::from_slice(json).map_err(|_| "配備の設定をJSONとして読めません。")?;
    let template = if let Source::Service {
        service,
        revision: Some(name),
        ..
    } = source
    {
        if value
            .pointer("/metadata/labels/serving.knative.dev~1service")
            .and_then(Value::as_str)
            != Some(service.as_str())
            || value.pointer("/metadata/name").and_then(Value::as_str) != Some(name.as_str())
        {
            return Err("指定した版とサービスの対応を確認できません。".into());
        }
        &value
    } else {
        if (matches!(source, Source::Service { .. }) || value.get("status").is_some())
            && !latest_serves_all(&value)
        {
            return Err(REVISION_REQUIRED.into());
        }
        &value["spec"]["template"]
    };
    let items = template
        .pointer("/spec/containers/0/env")
        .and_then(Value::as_array)
        .ok_or("配備の設定に環境変数の一覧がありません。")?;
    // A Knative secretKeyRef can name an alias for a fully qualified secret.
    let mut aliases = BTreeMap::new();
    if let Some(annotation) = template.pointer("/metadata/annotations/run.googleapis.com~1secrets")
    {
        let invalid = "配備の秘密の参照先を確認してください。";
        for pair in annotation
            .as_str()
            .ok_or(invalid)?
            .split(',')
            .filter(|pair| !pair.is_empty())
        {
            let (alias, resource) = pair.split_once(':').ok_or(invalid)?;
            if !resource.starts_with("projects/") || aliases.insert(alias, resource).is_some() {
                return Err(invalid.into());
            }
        }
    }
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
                name: aliases.get(secret).copied().unwrap_or(secret).to_owned(),
                version: version.to_owned(),
            }
        } else {
            return Err("配備の環境変数の渡し方を確認してください。".into());
        };
        if settings.insert(name.to_owned(), setting).is_some() {
            return Err("配備の環境変数名が重複しています。".into());
        }
    }
    Ok(settings)
}

const INVALID_CONFIG: &str = "遠隔保存の設定を確認してください。";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DbReferences {
    url_env: String,
    token_env: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReferenceConfig {
    version: u32,
    control: DbReferences,
    #[serde(deserialize_with = "tenant_references")]
    tenants: BTreeMap<String, DbReferences>,
}

// Match hosted/remote/config: duplicate tenant keys must not be collapsed by a map.
fn tenant_references<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, DbReferences>, D::Error> {
    struct Tenants;
    impl<'de> de::Visitor<'de> for Tenants {
        type Value = BTreeMap<String, DbReferences>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique tenant references")
        }
        fn visit_map<M: de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut entries = BTreeMap::new();
            while let Some(name) = map.next_key::<String>()? {
                if entries.len() >= 1000
                    || !(1..=48).contains(&name.len())
                    || name.starts_with('-')
                    || name.ends_with('-')
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    || entries.contains_key(&name)
                {
                    return Err(de::Error::custom(INVALID_CONFIG));
                }
                entries.insert(name, map.next_value()?);
            }
            Ok(entries)
        }
    }
    d.deserialize_map(Tenants)
}

impl ReferenceConfig {
    fn parse(raw: &str) -> Result<Self> {
        if raw.len() > 1024 * 1024 {
            return Err(INVALID_CONFIG.into());
        }
        let parsed: Self = serde_json::from_str(raw).map_err(|_| INVALID_CONFIG)?;
        let mut names = BTreeSet::new();
        if parsed.version != 1 {
            return Err(INVALID_CONFIG.into());
        }
        for db in parsed.databases() {
            for name in [&db.url_env, &db.token_env] {
                let valid = name.strip_prefix("MEIBO_DB_").is_some_and(|rest| {
                    !rest.is_empty()
                        && rest
                            .bytes()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                });
                if !valid || !names.insert(name) {
                    return Err(INVALID_CONFIG.into());
                }
            }
        }
        Ok(parsed)
    }

    fn databases(&self) -> impl Iterator<Item = &DbReferences> {
        std::iter::once(&self.control).chain(self.tenants.values())
    }
}

fn resolve(
    settings: &BTreeMap<String, Setting>,
    project: Option<&str>,
    mut fetch: impl FnMut(&str, Option<&str>, &str, &str) -> Result<String>,
) -> Result<Resolved> {
    let Some(Setting::Plain(config)) = settings.get(CONFIG_ENV) else {
        return Err(format!("配備の設定に {CONFIG_ENV} がありません。").into());
    };
    let config = ReferenceConfig::parse(config)?;
    let mut env = Vec::new();
    let mut secrets = BTreeSet::new();
    let mut endpoints = BTreeSet::new();
    for db in config.databases() {
        for (name, is_url) in [(&db.url_env, true), (&db.token_env, false)] {
            let value = match settings.get(name) {
                Some(Setting::Plain(value)) => value.clone(),
                Some(Setting::Secret {
                    name: secret,
                    version,
                }) => {
                    let project =
                        project.ok_or("秘密を取り出すプロジェクトを指定してください。")?;
                    let (owner, secret) = project_secret(secret)?;
                    if version.is_empty()
                        || version.starts_with('-')
                        || !version
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                    {
                        return Err("秘密の版を確認してください。".into());
                    }
                    secrets.insert(secret.to_owned());
                    fetch(project, owner, secret, version)?
                }
                None => return Err("参照する環境変数が配備の設定にありません。".into()),
            };
            if is_url {
                endpoints.insert(endpoint_identity(&value));
            }
            env.push((name.clone(), value));
        }
    }
    Ok(Resolved {
        config,
        env,
        secrets,
        endpoints,
    })
}

fn project_secret(reference: &str) -> Result<(Option<&str>, &str)> {
    let (owner, name) = if let Some(resource) = reference.strip_prefix("projects/") {
        let (owner, name) = resource
            .split_once("/secrets/")
            .ok_or("秘密の参照を確認してください。")?;
        if owner.is_empty()
            || owner.starts_with('-')
            || !owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("秘密の参照を確認してください。".into());
        }
        (Some(owner), name)
    } else {
        (None, reference)
    };
    if name.is_empty()
        || name.starts_with('-')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("秘密の参照を確認してください。".into());
    }
    Ok((owner, name))
}

// Cloud Run can spell the same project as an ID on the command line and a
// number in its secret annotation. Only a mixed pair needs a lookup.
fn same_project(
    left: &str,
    right: &str,
    gcloud: &mut impl FnMut(&[&str]) -> Result<Vec<u8>>,
) -> Result<bool> {
    if left == right {
        return Ok(true);
    }
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (id, number) = match (numeric(left), numeric(right)) {
        (false, true) => (left, right),
        (true, false) => (right, left),
        _ => return Ok(false),
    };
    let bytes = gcloud(&["projects", "describe", id, "--format=value(projectNumber)"])?;
    let actual = std::str::from_utf8(&bytes)
        .ok()
        .map(str::trim)
        .filter(|s| numeric(s))
        .ok_or("秘密のプロジェクト番号を確認できません。")?;
    Ok(actual == number)
}

fn endpoint_identity(raw: &str) -> String {
    let url = raw.trim_end_matches('/').to_ascii_lowercase();
    if let Some(authority) = url
        .strip_prefix("libsql://")
        .or_else(|| url.strip_prefix("https://"))
    {
        return format!(
            "https://{}",
            authority.strip_suffix(":443").unwrap_or(authority)
        );
    }
    url
}

fn gcloud_output(command: &mut Command) -> Result<Vec<u8>> {
    let output = command
        .env("CLOUDSDK_CORE_DISABLE_FILE_LOGGING", "1")
        .env("CLOUDSDK_CORE_LOG_HTTP", "false")
        .env("CLOUDSDK_CORE_VERBOSITY", "error")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "gcloud を実行できません。Google Cloud CLI を確認してください。")?;
    if !output.status.success() {
        return Err("gcloud が失敗しました。対象の指定と読み取り権限を確認してください。".into());
    }
    Ok(output.stdout)
}

fn gcloud(args: &[&str]) -> Result<Vec<u8>> {
    gcloud_output(Command::new("gcloud").args(args))
}

fn load(
    source: &Source,
    project: Option<&str>,
    mut run_gcloud: impl FnMut(&[&str]) -> Result<Vec<u8>>,
) -> Result<Resolved> {
    let json = match source {
        Source::File(path) => fs::read(path)?,
        Source::Service {
            region,
            service,
            revision,
        } => {
            let project = project.ok_or(USAGE)?;
            let (collection, name) = revision
                .as_ref()
                .map_or(("services", service), |name| ("revisions", name));
            run_gcloud(&[
                "run",
                collection,
                "describe",
                name,
                "--region",
                region,
                "--project",
                project,
                "--format",
                "json",
            ])?
        }
    };
    resolve(
        &deployment(&json, source)?,
        project,
        |project, owner, name, version| {
            if let Some(owner) = owner
                && !same_project(project, owner, &mut run_gcloud)?
            {
                return Err("秘密のプロジェクトと指定が一致しません。".into());
            }
            let bytes = run_gcloud(&[
                "secrets",
                "versions",
                "access",
                version,
                "--secret",
                name,
                "--project",
                project,
            ])?;
            let mut value = String::from_utf8(bytes).map_err(|_| "秘密の値を読めません。")?;
            if value.ends_with('\n') {
                value.pop();
            }
            Ok(value)
        },
    )
}

/// 復旧では元と先を同じ環境に置くので、参照名が重なると値が取り違わる。
fn disjoint(
    source: &Resolved,
    target: &Resolved,
    projects_match: impl FnOnce() -> Result<bool>,
) -> Result<()> {
    if !source.secrets.is_disjoint(&target.secrets) && projects_match()? {
        return Err("元と復旧先が同じプロジェクトの同じ秘密を参照しています。".into());
    }
    if !source.endpoints.is_disjoint(&target.endpoints) {
        return Err("元と復旧先に同じ接続先があります。".into());
    }
    let names: BTreeSet<&str> = source.env.iter().map(|(name, _)| name.as_str()).collect();
    if target
        .env
        .iter()
        .any(|(name, _)| names.contains(name.as_str()))
    {
        return Err("元と復旧先で環境変数名を分けてください。".into());
    }
    Ok(())
}

fn configurations(
    options: &Options,
    mut run_gcloud: impl FnMut(&[&str]) -> Result<Vec<u8>>,
) -> Result<(Resolved, Option<Resolved>)> {
    let source = load(&options.source, options.project.as_deref(), &mut run_gcloud)?;
    let Some(path) = &options.target else {
        return Ok((source, None));
    };
    let target = load(
        &Source::File(path.clone()),
        options.target_project.as_deref(),
        &mut run_gcloud,
    )?;
    disjoint(&source, &target, || {
        match (&options.project, &options.target_project) {
            (Some(a), Some(b)) => same_project(a, b, &mut run_gcloud),
            _ => Ok(false),
        }
    })?;
    Ok((source, Some(target)))
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
    let (mut source, target) = configurations(&options, gcloud)?;
    let source_file = Private::write(&temp, "source", &source.config)?;
    let mut command_args = options.command.clone();
    let mut target_file = None;
    if let Some(target) = target {
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
                service: "s".into(),
                revision: None,
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
        let settings = deployment(&json, &Source::File("f".into())).unwrap();
        let resolved = resolve(&settings, Some("p"), |_, _, name, version| {
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
        assert!(
            !serde_json::to_string(&resolved.config)
                .unwrap()
                .contains("value-of")
        );
    }

    #[test]
    fn a_missing_or_foreign_reference_is_refused_without_values() {
        let json = service(&[plain("MEIBO_DB_CONTROL_URL", "libsql://private.invalid")]);
        let settings = deployment(&json, &Source::File("f".into())).unwrap();
        let error = resolve(&settings, None, |_, _, _, _| Ok(String::new()))
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, "参照する環境変数が配備の設定にありません。");
        assert!(!error.contains("private.invalid"));
        let foreign = CONFIG.replace("MEIBO_DB_ALPHA_TOKEN", "PATH");
        assert!(ReferenceConfig::parse(&foreign).is_err());
    }

    #[test]
    fn restore_source_and_target_names_must_differ() {
        let side = |name: &str| Resolved {
            config: ReferenceConfig::parse(CONFIG).unwrap(),
            env: vec![(name.to_owned(), "value".to_owned())],
            endpoints: BTreeSet::new(),
            secrets: BTreeSet::new(),
        };
        assert!(disjoint(&side("MEIBO_DB_A"), &side("MEIBO_DB_B"), || Ok(true)).is_ok());
        assert!(disjoint(&side("MEIBO_DB_A"), &side("MEIBO_DB_A"), || Ok(true)).is_err());
    }

    #[test]
    fn the_temporary_config_is_private_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let config = ReferenceConfig::parse(CONFIG).unwrap();
        let file = Private::write(directory.path(), "source", &config).unwrap();
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

    #[cfg(unix)]
    #[test]
    fn gcloud_is_private() {
        let script = r#"test "$CLOUDSDK_CORE_DISABLE_FILE_LOGGING" = 1 && test "$CLOUDSDK_CORE_LOG_HTTP" = false && test "$CLOUDSDK_CORE_VERBOSITY" = error"#;
        let mut child = Command::new("sh");
        child
            .env("CLOUDSDK_CORE_LOG_HTTP", "true")
            .args(["-c", script]);
        assert!(gcloud_output(&mut child).is_ok());
        let mut failed = Command::new("sh");
        failed.args(["-c", "echo fixture-stdout; echo fixture-stderr >&2; exit 1"]);
        assert_eq!(
            gcloud_output(&mut failed).unwrap_err().to_string(),
            "gcloud が失敗しました。対象の指定と読み取り権限を確認してください。"
        );
    }

    #[test]
    fn traffic_guard() {
        use serde_json::json;
        let live = Source::Service {
            region: "r".into(),
            service: "s".into(),
            revision: None,
        };
        let file = Source::File("f".into());
        let mut value: Value = serde_json::from_slice(&service(&[])).unwrap();
        assert!(deployment(&service(&[]), &live).is_err());
        assert!(deployment(&service(&[]), &file).is_ok());
        value["status"] = json!({"latestCreatedRevisionName":"new", "latestReadyRevisionName":"new",
            "traffic":[{"revisionName":"new","percent":100},{"revisionName":"old","tag":"previous"}]});
        for source in [&live, &file] {
            assert!(deployment(&serde_json::to_vec(&value).unwrap(), source).is_ok());
            for (key, changed) in [
                ("latestCreatedRevisionName", json!("next")),
                ("latestReadyRevisionName", Value::Null),
                ("conditions", json!([{"type":"Ready","status":"Unknown"}])),
                ("traffic", json!([{"revisionName":"old","percent":100}])),
                (
                    "traffic",
                    json!([{"revisionName":"new","percent":99},{"revisionName":"old","percent":1}]),
                ),
                ("traffic", json!([{"revisionName":"new","percent":99}])),
                ("traffic", json!([{"latestRevision":true,"percent":100}])),
                ("traffic", json!([])),
            ] {
                let mut invalid = value.clone();
                invalid["status"][key] = changed;
                let err = deployment(&serde_json::to_vec(&invalid).unwrap(), source)
                    .err()
                    .unwrap();
                assert!(err.to_string().contains("--revision NAME"));
            }
        }
        value["metadata"]["generation"] = json!(2);
        value["status"]["observedGeneration"] = json!(1);
        assert!(deployment(&serde_json::to_vec(&value).unwrap(), &live).is_err());
    }

    fn side(prefix: &str, tenant_endpoint: &str, token_secret: &str) -> Vec<u8> {
        let plain_control = format!("libsql://{prefix}.invalid");
        let json = service(&[
            plain("MEIBO_DB_CONTROL_URL", &plain_control),
            referenced("MEIBO_DB_CONTROL_TOKEN", token_secret),
            plain("MEIBO_DB_ALPHA_URL", tenant_endpoint),
            referenced("MEIBO_DB_ALPHA_TOKEN", &format!("{prefix}-member")),
        ]);
        String::from_utf8(json)
            .unwrap()
            .replace("MEIBO_DB_", &format!("MEIBO_DB_{prefix}_"))
            .into_bytes()
    }

    #[test]
    fn revision_owner() {
        use serde_json::json;
        let options = parse(&args(&[
            "--project",
            "p",
            "--region",
            "r",
            "--service",
            "s",
            "--revision",
            "old",
            "--",
            "gate-status",
        ]))
        .unwrap();
        let template: Value =
            serde_json::from_slice(&side("OLD", "old-endpoint", "control")).unwrap();
        let mut revision = json!({"metadata":{"name":"old","labels":{"serving.knative.dev/service":"s"},
            "annotations":{"run.googleapis.com/secrets":"control:projects/42/secrets/actual-control"}},
            "spec":{"containers":template["spec"]["template"]["spec"]["containers"], "template":{"spec":{"containers":[]}}}});
        let mut fetched = Vec::new();
        let resolved = load(&options.source, Some("p"), |cmd| {
            if cmd[0] == "run" {
                assert_eq!(
                    cmd,
                    [
                        "run",
                        "revisions",
                        "describe",
                        "old",
                        "--region",
                        "r",
                        "--project",
                        "p",
                        "--format",
                        "json"
                    ]
                );
                Ok(serde_json::to_vec(&revision)?)
            } else if cmd[0] == "projects" {
                assert_eq!(
                    cmd,
                    ["projects", "describe", "p", "--format=value(projectNumber)"]
                );
                Ok(b"42\n".to_vec())
            } else {
                fetched.push(cmd[5].to_owned());
                Ok(b"fixture".to_vec())
            }
        })
        .unwrap();
        assert!(resolved.endpoints.contains("old-endpoint"));
        assert_eq!(fetched, ["actual-control", "OLD-member"]);
        assert!(
            load(&options.source, Some("p"), |cmd| match cmd[0] {
                "run" => Ok(serde_json::to_vec(&revision)?),
                "projects" => Ok(b"43".to_vec()),
                _ => panic!("foreign secret accessed"),
            })
            .is_err()
        );
        revision["metadata"]["labels"]["serving.knative.dev/service"] = json!("other");
        assert!(deployment(&serde_json::to_vec(&revision).unwrap(), &options.source).is_err());
        assert!(
            parse(&args(&[
                "--project",
                "p",
                "--region",
                "r",
                "--revision",
                "old",
                "--",
                "gate-status"
            ]))
            .is_err()
        );
    }

    #[test]
    fn restore_projects() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, side("SRC", "original-tenant", "shared")).unwrap();
        let mut other: Value =
            serde_json::from_slice(&side("DST", "new-tenant", "shared")).unwrap();
        let env_path = "/spec/template/spec/containers/0/env";
        other.pointer_mut(env_path).unwrap()[2]["valueFrom"]["secretKeyRef"]["key"] =
            serde_json::json!("7");
        fs::write(&target, serde_json::to_vec(&other).unwrap()).unwrap();
        let mut flags = args(&[
            "--describe",
            source.to_str().unwrap(),
            "--project",
            "p",
            "--target-describe",
            target.to_str().unwrap(),
        ]);
        let command = args(&["--", "restore", "snapshot"]);
        let same = parse(&[flags.as_slice(), command.as_slice()].concat()).unwrap();
        let result = configurations(&same, |_| Ok(Vec::new()));
        assert!(result.err().unwrap().to_string().contains("同じ秘密"));
        let by_number = Options {
            target_project: Some("42".into()),
            ..same
        };
        let result = configurations(&by_number, |cmd| {
            Ok(if cmd[0] == "projects" {
                b"42".to_vec()
            } else {
                Vec::new()
            })
        });
        assert!(result.err().unwrap().to_string().contains("同じ秘密"));
        flags.extend(args(&["--target-project", "q"]));
        flags.extend(command);
        let separate = parse(&flags).unwrap();
        let mut accesses = Vec::new();
        let (_, restored) = configurations(&separate, |cmd| {
            assert_eq!(&cmd[..3], ["secrets", "versions", "access"]);
            accesses.push((cmd[5].to_owned(), cmd[7].to_owned()));
            Ok(cmd[7].as_bytes().to_vec())
        })
        .unwrap();
        assert_eq!(
            accesses,
            [
                ("shared".to_owned(), "p".to_owned()),
                ("SRC-member".to_owned(), "p".to_owned()),
                ("shared".to_owned(), "q".to_owned()),
                ("DST-member".to_owned(), "q".to_owned())
            ]
        );
        assert!(
            restored
                .unwrap()
                .env
                .contains(&("MEIBO_DB_DST_CONTROL_TOKEN".into(), "q".into()))
        );
        // A target tenant URL from Secret Manager points to the source control DB.
        other.pointer_mut(env_path).unwrap()[3] =
            referenced("MEIBO_DB_DST_ALPHA_URL", "other-endpoint");
        fs::write(&target, serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(
            configurations(&separate, |cmd| Ok(if cmd[5] == "other-endpoint" {
                format!("https://{}.invalid:443/", "src").into_bytes()
            } else {
                Vec::new()
            }))
            .err()
            .unwrap()
            .to_string()
            .contains("同じ接続先")
        );
        assert!(
            parse(&args(&[
                "--describe",
                "f",
                "--target-project",
                "q",
                "--",
                "gate-status"
            ]))
            .is_err()
        );
    }

    #[test]
    fn strict_references() {
        use serde_json::json;
        for pointer in ["", "/control", "/tenants/alpha"] {
            let mut value: Value = serde_json::from_str(CONFIG).unwrap();
            value.pointer_mut(pointer).unwrap()["unknown"] = json!("unexpected-value");
            let settings =
                BTreeMap::from([(CONFIG_ENV.to_owned(), Setting::Plain(value.to_string()))]);
            let err = resolve(&settings, Some("p"), |_, _, _, _| {
                panic!("invalid config reached Secret Manager")
            })
            .err()
            .unwrap();
            assert_eq!(err.to_string(), INVALID_CONFIG);
        }
        for invalid in [
            CONFIG.replace("\"version\":1", "\"version\":2"),
            CONFIG.replace("\"version\":1", "\"version\":1,\"version\":1"),
            CONFIG.replace(
                "\"alpha\":",
                "\"alpha\":{\"url_env\":\"MEIBO_DB_A\",\"token_env\":\"MEIBO_DB_B\"},\"alpha\":",
            ),
            CONFIG.replace("\"alpha\"", "\"bad/slug\""),
            CONFIG.replace("MEIBO_DB_CONTROL_URL", "MEIBO_DB_ALPHA_TOKEN"),
        ] {
            assert!(ReferenceConfig::parse(&invalid).is_err());
        }
        let expected: Value = serde_json::from_str(CONFIG).unwrap();
        let raw = serde_json::to_string_pretty(&expected).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file =
            Private::write(dir.path(), "source", &ReferenceConfig::parse(&raw).unwrap()).unwrap();
        let written = fs::read_to_string(&file.0).unwrap();
        assert_ne!(written, raw, "the temporary file is reconstructed");
        assert_eq!(serde_json::from_str::<Value>(&written).unwrap(), expected);
    }

    fn runbook_order(section: &str, steps: &[&str]) {
        let doc = include_str!("../../../docs/cloud-run-operations.md");
        let mut remaining = doc
            .split_once(section)
            .expect("runbook section")
            .1
            .split("\n## ")
            .next()
            .unwrap();
        for step in steps {
            remaining = remaining
                .split_once(step)
                .unwrap_or_else(|| panic!("missing or out of order: {step}"))
                .1;
        }
    }

    #[test]
    fn restore_order() {
        runbook_order(
            "## バックアップから戻す",
            &[
                "--ingress=internal",
                "進行中の処理が終了",
                "gate-status",
                "\"occupied\":false",
                "保存物の作成後",
                "restore BACKUP_DIRECTORY",
                "`restored`",
                "切り替え",
                "件数と記録を照合",
                "--ingress=all",
            ],
        );
    }

    #[test]
    fn revoke_order() {
        runbook_order(
            "## 鍵の期限と漏えい",
            &[
                "--ingress=internal",
                "turso db tokens invalidate DB_NAME",
                "すべて",
                "turso db tokens create",
                "secrets versions add",
                "--update-secrets",
                "起動",
                "CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1",
                "secrets versions access OLD_VERSION",
                "curl -q -s -o /dev/null",
                "-H @-",
                "401 または 403",
                "--ingress=all",
                "secrets versions destroy OLD_VERSION",
            ],
        );
    }
}
