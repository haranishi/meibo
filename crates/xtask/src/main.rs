//! 開発用点検コマンドの入口。
use std::path::Path;
fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = root
        .canonicalize()
        .map_err(Into::into)
        .and_then(|root| xtask::run(&root, &std::env::args().skip(1).collect::<Vec<_>>()));
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
