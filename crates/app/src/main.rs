//! 手元の保存先を操作するコマンド。
mod args;
mod commands;
mod hosted;
mod mcp;
mod server;

fn main() -> std::process::ExitCode {
    let command = match args::parse(std::env::args_os().skip(1).collect()) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}\nUse meibo --help for usage.");
            return std::process::ExitCode::from(2);
        }
    };
    match commands::run(command) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
