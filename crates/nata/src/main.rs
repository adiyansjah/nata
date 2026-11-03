//! `nata`: run a node, or act as a CLI client.
//!
//! Nothing is implemented yet. Every command fails with a clear
//! `not supported yet` error (see `docs/way-of-working.md`, section 3).

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "Nata: lakehouse SQL engine (pre-alpha, no queries yet)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a node.
    Run,
    /// Run SQL and print the result.
    Sql {
        /// SQL text to run.
        #[arg(short = 'c', long = "command")]
        command: String,
        /// Print the result as CSV (header row, then data rows).
        #[arg(long)]
        csv: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let what = match cli.command {
        Command::Run => "`nata run` (node startup)",
        Command::Sql { .. } => "SQL execution",
    };
    eprintln!("error: not supported yet: {what}");
    ExitCode::from(2)
}
