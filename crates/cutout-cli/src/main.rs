use arguments::{Cli, Command};
use clap::Parser;

mod arguments;
mod batch_bench;
mod commands;

fn main() {
    // The installed shell protocol remains shared with cutout-core and accepts
    // literal OsString selections after --, including non-UTF-8 paths.
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("context-cutout")) {
        std::process::exit(cutout_cli::context_main(std::env::args_os().skip(2)));
    }
    if let Err(error) = commands::run(Cli::parse().command.unwrap_or(Command::Info)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
