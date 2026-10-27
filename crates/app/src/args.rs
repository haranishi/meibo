use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use meibo_core::{DuplicatePolicy, types::DataOrigin};
use meibo_import::{Encoding, ExportOptions, ImportOptions};

pub(crate) const HELP: &str = "meibo — company list CRM (early development release)

Usage:
  meibo init DB --origin synthetic|real
  meibo demo DB [--rows 1..100000]
  meibo import DB CSV [--encoding utf-8|shift-jis] [--duplicates error|skip]
  meibo export DB CSV [--encoding utf-8|shift-jis] [--include-personal] [--include-excluded]
  meibo list DB [--limit 1..200] [--search TERM]
  meibo verify DB
  meibo serve DB [--port 4317] [--allow-real]
  meibo sync-nta DB CATALOG [--limit COUNT]
  meibo mcp DB [--allow-write]
  meibo hosted ROOT COMMAND [OPTIONS]

Import commits batches of up to 500 rows / 8 MiB. Earlier batches remain on error.
Export creates a new file, omits sole proprietors unless requested, and guards formulas.
CSV is a company snapshot, not a backup of contacts or activities.
";

pub(crate) enum Command {
    Help,
    Init(PathBuf, DataOrigin),
    Demo(PathBuf, u64),
    Import(PathBuf, PathBuf, ImportOptions),
    Export(PathBuf, PathBuf, ExportOptions),
    List(PathBuf, u32, Option<String>),
    Verify(PathBuf),
    Serve(PathBuf, u16, bool),
    SyncNta(PathBuf, PathBuf, Option<u64>),
    Mcp(PathBuf, bool),
    Hosted(Vec<OsString>),
    #[cfg(feature = "cloud")]
    HostedRemote(Vec<OsString>),
}

struct Arguments {
    paths: Vec<PathBuf>,
    options: BTreeMap<String, String>,
}

impl Arguments {
    fn parse(args: Vec<OsString>, values: &[&str], flags: &[&str]) -> Result<Self, String> {
        let mut paths = Vec::new();
        let mut options = BTreeMap::new();
        let mut rest = args.into_iter();
        let mut positional_only = false;
        while let Some(arg) = rest.next() {
            let name = arg.to_str().unwrap_or_default();
            if !positional_only && name == "--" {
                positional_only = true;
            } else if !positional_only && name.starts_with('-') {
                let value = if values.contains(&name) {
                    rest.next()
                        .and_then(|s| s.into_string().ok())
                        .filter(|s| !s.starts_with("--"))
                        .ok_or("option requires a value")?
                } else if flags.contains(&name) {
                    String::new()
                } else {
                    return Err("unknown option".into());
                };
                if options.insert(name.to_owned(), value).is_some() {
                    return Err("repeated option".into());
                }
            } else {
                paths.push(PathBuf::from(arg));
            }
        }
        Ok(Self { paths, options })
    }

    fn paths(&self, expected: usize) -> Result<(), String> {
        if self.paths.len() == expected {
            Ok(())
        } else {
            Err("incorrect number of positional arguments".into())
        }
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.options.get(key).map(String::as_str)
    }

    fn encoding(&self) -> Result<Encoding, String> {
        match self.value("--encoding").unwrap_or("utf-8") {
            "utf-8" => Ok(Encoding::Utf8),
            "shift-jis" => Ok(Encoding::ShiftJis),
            _ => Err("encoding must be utf-8 or shift-jis".into()),
        }
    }
}

pub(crate) fn parse(mut args: Vec<OsString>) -> Result<Command, String> {
    if args.is_empty() {
        return Ok(Command::Help);
    }
    let name = args
        .remove(0)
        .into_string()
        .map_err(|_| "invalid command")?;
    let command = name.as_str();
    if command == "hosted" {
        return Ok(Command::Hosted(args));
    }
    #[cfg(feature = "cloud")]
    if command == "hosted-remote" {
        return Ok(Command::HostedRemote(args));
    }
    let (values, flags): (&[&str], &[&str]) = match command {
        "--help" | "-h" | "help" if args.is_empty() => return Ok(Command::Help),
        "init" => (&["--origin"], &[]),
        "demo" => (&["--rows"], &[]),
        "import" => (&["--encoding", "--duplicates"], &[]),
        "export" => (
            &["--encoding"],
            &["--include-personal", "--include-excluded"],
        ),
        "list" => (&["--limit", "--search"], &[]),
        "verify" => (&[], &[]),
        "serve" => (&["--port"], &["--allow-real"]),
        "sync-nta" => (&["--limit"], &[]),
        "mcp" => (&[], &["--allow-write"]),
        _ => return Err("unknown command".into()),
    };
    let a = Arguments::parse(args, values, flags)?;
    a.paths(usize::from(matches!(command, "import" | "export" | "sync-nta")) + 1)?;
    parsed_command(command, &a)
}

fn parsed_command(command: &str, a: &Arguments) -> Result<Command, String> {
    let db = a.paths[0].clone();
    match command {
        "init" => Ok(Command::Init(
            db,
            match a.value("--origin") {
                Some("synthetic") => DataOrigin::Synthetic,
                Some("real") => DataOrigin::Real,
                _ => return Err("init requires --origin synthetic|real".into()),
            },
        )),
        "demo" => {
            let rows = a
                .value("--rows")
                .unwrap_or("100")
                .parse::<u64>()
                .ok()
                .filter(|n| (1..=100_000).contains(n))
                .ok_or("rows must be 1..100000")?;
            Ok(Command::Demo(db, rows))
        }
        "import" => Ok(Command::Import(
            db,
            a.paths[1].clone(),
            ImportOptions {
                encoding: a.encoding()?,
                duplicates: match a.value("--duplicates").unwrap_or("error") {
                    "error" => DuplicatePolicy::Error,
                    "skip" => DuplicatePolicy::Skip,
                    _ => return Err("duplicates must be error or skip".into()),
                },
            },
        )),
        "export" => Ok(Command::Export(
            db,
            a.paths[1].clone(),
            ExportOptions {
                encoding: a.encoding()?,
                include_personal: a.options.contains_key("--include-personal"),
                include_excluded: a.options.contains_key("--include-excluded"),
            },
        )),
        "list" => {
            let limit = a
                .value("--limit")
                .unwrap_or("50")
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=200).contains(n))
                .ok_or("limit must be 1..200")?;
            Ok(Command::List(
                db,
                limit,
                a.value("--search").map(str::to_owned),
            ))
        }
        "verify" => Ok(Command::Verify(db)),
        "serve" => Ok(Command::Serve(
            db,
            a.value("--port")
                .unwrap_or("4317")
                .parse::<u16>()
                .map_err(|_| "port must be 0..65535")?,
            a.options.contains_key("--allow-real"),
        )),
        "sync-nta" => Ok(Command::SyncNta(
            db,
            a.paths[1].clone(),
            a.value("--limit")
                .map(|v| {
                    v.parse::<u64>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("limit must be positive")
                })
                .transpose()?,
        )),
        "mcp" => Ok(Command::Mcp(db, a.options.contains_key("--allow-write"))),
        _ => unreachable!(),
    }
}
