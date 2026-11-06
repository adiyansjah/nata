//! `nata`: run a node, or act as a CLI client.
//!
//! SQL and the node are not implemented yet: those commands fail with a clear
//! `not supported yet` error (see `docs/way-of-working.md`, section 3).
//! `nata scan` is the minimal developer command of slice 3 (`docs/slices/0001-parquet-scan.md`):
//! it reads Parquet files and prints CSV.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use nata_parquet::csv;
use nata_parquet::{ErrorKind, ParallelConfig, ScanError, ScanOptions, ScanPlan, run_parallel};

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
        /// Print the result as CSV.
        #[arg(long)]
        csv: bool,
        /// Print only the number of result rows (one line), not the rows: measures the scan without CSV rendering.
        #[arg(long, conflicts_with = "csv")]
        count: bool,
        /// Do not use row-group statistics to skip row groups (for checking the pruning; the filter still applies).
        #[arg(long)]
        no_prune: bool,
        /// Worker threads (default: the available parallelism). Memory is bounded by the window (default: this number) times the largest row group.
        #[arg(long)]
        threads: Option<usize>,
        /// Print rows as row groups finish, in no promised order (default: the order of the files, row groups and rows, for any thread count).
        #[arg(long)]
        unordered: bool,
        /// Row groups in flight at once, running or finished and waiting for the writer (default: the thread count). Developer option.
        #[arg(long)]
        window: Option<usize>,
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

/// Highest resident memory of this process so far, from `/proc` (Linux); `None` elsewhere.
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

struct ScanArgs {
    count: bool,
    files: Vec<PathBuf>,
    columns: Option<Vec<String>>,
    filters: Vec<String>,
    limit: Option<u64>,
    no_prune: bool,
    threads: Option<usize>,
    unordered: bool,
    window: Option<usize>,
}

fn scan(a: ScanArgs) -> ExitCode {
    let ScanArgs {
        count,
        files,
        columns,
        filters,
        limit,
        no_prune,
        threads,
        unordered,
        window,
    } = a;
    if threads == Some(0) || window == Some(0) {
        let _ = writeln!(
            std::io::stderr(),
            "error: --threads and --window must be at least 1"
        );
        return ExitCode::from(2);
    }
    let threads = threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
    // a panic must not print its message (it could hold a value from a file): location only
    std::panic::set_hook(Box::new(|info| {
        let at = info
            .location()
            .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        let _ = writeln!(std::io::stderr(), "error: internal error{at}");
    }));
    let options = ScanOptions {
        columns,
        filters,
        prune: !no_prune,
    };
    let result = (|| {
        let plan = Arc::new(ScanPlan::open(&files, &options)?);
        let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
        let stdout = std::io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        let io_error = |e: std::io::Error| ScanError::new(ErrorKind::Io(e.to_string()));
        let cfg = ParallelConfig {
            threads,
            window: window.unwrap_or(threads),
            ordered: !unordered,
        };
        let report = if count {
            let mut rows: u64 = 0;
            let report = run_parallel(plan, cfg, limit, |batch| {
                rows = rows.saturating_add(u64::try_from(batch.num_rows()).unwrap_or(u64::MAX));
                Ok(())
            })?;
            writeln!(out, "{rows}").map_err(io_error)?;
            report
        } else {
            csv::write_header(&mut out, &names).map_err(io_error)?;
            run_parallel(plan, cfg, limit, |batch| csv::write_batch(&mut out, batch))?
        };
        out.flush().map_err(io_error)?;
        Ok::<_, ScanError>(report)
    })();
    match result {
        Ok(report) => {
            // writeln, not eprintln: a closed stderr must not be a panic
            let rss = peak_rss_kib().map_or(String::new(), |k| format!(" peak_rss_kib={k}"));
            let _ = writeln!(std::io::stderr(), "{}{rss}", report.line());
            ExitCode::SUCCESS
        }
        // `nata scan ... | head`: the reader went away, which is not a failure of the scan
        Err(ScanError {
            kind: ErrorKind::Io(m),
            file: None,
            ..
        }) if m.contains("Broken pipe") => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "error: {e}");
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
            count,
            no_prune,
            threads,
            unordered,
            window,
        } => {
            if !csv && !count {
                eprintln!("error: not supported yet: output other than --csv or --count");
                return ExitCode::from(2);
            }
            return scan(ScanArgs {
                count,
                files,
                columns,
                filters,
                limit,
                no_prune,
                threads,
                unordered,
                window,
            });
        }
    };
    eprintln!("error: not supported yet: {what}");
    ExitCode::from(2)
}
