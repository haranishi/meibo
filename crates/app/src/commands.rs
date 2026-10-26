use std::{
    error::Error,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use meibo_core::{
    Engine, OpenOptions,
    synth::{Profile, SynthConfig},
    types::{DataOrigin, Filters, ListQuery, SearchOptions, Sort},
};
use meibo_import::{ExportOptions, export_csv, import_csv};
use serde_json::json;

use crate::args::{Command, HELP};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub(crate) fn run(command: Command) -> Result<()> {
    let options = OpenOptions::default();
    match command {
        Command::Help => print!("{HELP}"),
        Command::Serve(db, port, allow_real) => {
            if allow_real {
                crate::server::serve_with_options(&db, port, true)?;
            } else {
                crate::server::serve(&db, port)?;
            }
        }
        Command::Mcp(db, allow_write) => crate::mcp::serve(&db, allow_write)?,
        Command::Hosted(args) => crate::hosted::run(args)?,
        Command::SyncNta(db, source, limit) => {
            let mut engine = Engine::open(&db, &options)?;
            let report = meibo_import::sync_nta(&mut engine, &source, limit)?;
            println!(
                "inserted={} updated={} skipped={}",
                report.inserted, report.updated, report.skipped
            );
        }
        Command::Init(db, origin) => {
            let _engine = Engine::create(&db, origin, &options)?;
            println!("created");
        }
        Command::Demo(db, rows) => {
            let mut engine = Engine::create(&db, DataOrigin::Synthetic, &options)?;
            let report = engine.load_synthetic(SynthConfig {
                rows,
                seed: 42,
                profile: Profile::Dense,
            })?;
            println!("synthetic_companies={}", report.companies);
        }
        Command::Import(db, input, opt) => {
            let reader = File::open(input)?;
            let mut engine = Engine::open(&db, &options)?;
            let report = import_csv(&mut engine, reader, opt)?;
            println!("inserted={} skipped={}", report.inserted, report.skipped);
        }
        Command::Export(db, output, opt) => {
            // Refuse existing destinations before doing any work; persist_noclobber also
            // closes the race if another process creates the destination meanwhile.
            if output.symlink_metadata().is_ok() {
                return Err("output already exists".into());
            }
            let engine = Engine::open(&db, &options)?;
            let rows = export_file(&engine, &output, opt)?;
            println!("exported={rows}");
        }
        Command::List(db, limit, search) => {
            let engine = Engine::open(&db, &options)?;
            show_list(&engine, limit, search.as_deref())?;
        }
        Command::Verify(db) => {
            let engine = Engine::open(&db, &options)?;
            let report = engine.verify()?;
            if !(report.integrity_ok && report.counts_match && report.folds_match) {
                return Err("verification failed".into());
            }
            println!("integrity=ok counts=ok activities=ok");
        }
    }
    Ok(())
}

fn export_file(engine: &Engine, output: &Path, options: ExportOptions) -> Result<u64> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let rows = {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        let rows = export_csv(engine, &mut writer, options)?;
        writer.flush()?;
        rows
    };
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(output)?;
    Ok(rows)
}

fn show_list(engine: &Engine, limit: u32, search: Option<&str>) -> Result<()> {
    let rows = if let Some(term) = search {
        engine
            .search_name(
                term,
                &SearchOptions {
                    after: None,
                    limit,
                    fields: vec![],
                    include_excluded: false,
                    with_count: false,
                },
            )?
            .rows
    } else {
        engine
            .list(&ListQuery {
                filters: Filters::default(),
                sort: Sort::ShapeDefault,
                after: None,
                limit,
                fields: vec![],
            })?
            .rows
    };
    let mut out = std::io::stdout().lock();
    for row in rows {
        writeln!(
            out,
            "{}",
            json!({
                "id":row.id.0, "corporate_number":row.corporate_number, "name":row.name,
                "prefecture":row.prefecture, "status":row.status.code(), "phone":row.phone,
            })
        )?;
    }
    Ok(())
}
