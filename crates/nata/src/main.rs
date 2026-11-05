//! `nata`: run a node, or act as a CLI client.
//!
//! SQL and the node are not implemented yet: those commands fail with a clear
//! `not supported yet` error (see `docs/way-of-working.md`, section 3).
//! `nata scan` is the minimal developer command of slice 3 (`docs/slices/0001-parquet-scan.md`):
//! it reads Parquet files and prints CSV.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use nata_parquet::csv;
use nata_parquet::{ErrorKind, ScanError, ScanOptions, ScanPlan, run_serial};

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
    /// Developer command: read Parquet files and print the rows as CSV (slice 3). stdout is the CSV (header row, then data rows; NULL as `NULL`),
    /// stderr gets a one-line report: files, row groups total / skipped / read, bytes read, morsel size, whether pruning was applied.
    Scan {
        /// Parquet files, scanned in the order given.
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Columns to print, in this order (default: all columns of the first file).
        #[arg(long, value_delimiter = ',')]
        columns: Option<Vec<String>>,
        /// `column op literal` with op one of < <= > >= = <> (or `column IS [NOT] NULL`); repeat for AND. Example: `l_shipdate >= 1994-01-01`.
        /// Literals are typed from the column: dates as YYYY-MM-DD, decimals exact, strings in single quotes.
        #[arg(long = "filter")]
        filters: Vec<String>,
        /// Stop after this many rows.
        #[arg(long)]
        limit: Option<u64>,
        /// Print the result as CSV (the only output format yet).
        #[arg(long)]
        csv: bool,
        /// Do not use row-group statistics to skip row groups (for checking the pruning; the filter still applies).
        #[arg(long)]
        no_prune: bool,
    },
}

/// Exit codes: 0 success, 1 a file or data error, 2 `not supported yet` or a wrong use of the command.
fn exit_code(e: &ScanError) -> ExitCode {
    match e.kind {
        ErrorKind::NotSupported(_) | ErrorKind::Usage(_) | ErrorKind::Filter(_) => {
            ExitCode::from(2)
        }
        _ => ExitCode::from(1),
    }
}

fn scan(
    files: Vec<PathBuf>,
    columns: Option<Vec<String>>,
    filters: Vec<String>,
    limit: Option<u64>,
    no_prune: bool,
) -> ExitCode {
    // a panic must not print its message (it could hold a value from a file): location only
    std::panic::set_hook(Box::new(|info| {
        let at = info
            .location()
            .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        eprintln!("error: internal error{at}");
    }));
    let options = ScanOptions {
        columns,
        filters,
        prune: !no_prune,
    };
    let result = (|| {
        let plan = ScanPlan::open(&files, &options)?;
        let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
        let stdout = std::io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        let io_error = |e: std::io::Error| ScanError::new(ErrorKind::Io(e.to_string()));
        csv::write_header(&mut out, &names).map_err(io_error)?;
        let report = run_serial(&plan, limit, |batch| csv::write_batch(&mut out, batch))?;
        out.flush().map_err(io_error)?;
        Ok::<_, ScanError>(report)
    })();
    match result {
        Ok(report) => {
            eprintln!("{}", report.line());
            ExitCode::SUCCESS
        }
        // `nata scan ... | head`: the reader went away, which is not a failure of the scan
        Err(ScanError {
            kind: ErrorKind::Io(m),
            file: None,
            ..
        }) if m.contains("Broken pipe") => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            exit_code(&e)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let what = match cli.command {
        Command::Run => "`nata run` (node startup)",
        Command::Sql { .. } => "SQL execution",
        Command::Scan {
            files,
            columns,
            filters,
            limit,
            csv,
            no_prune,
        } => {
            if !csv {
                eprintln!("error: not supported yet: output other than --csv");
                return ExitCode::from(2);
            }
            return scan(files, columns, filters, limit, no_prune);
        }
    };
    eprintln!("error: not supported yet: {what}");
    ExitCode::from(2)
}
