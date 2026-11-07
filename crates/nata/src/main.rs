//! `nata`: run a node, or act as a CLI client.
//!
//! The node is not implemented yet: `nata run` fails with a clear `not supported yet` error (see `docs/way-of-working.md`, section 3).
//! `nata sql` runs `SELECT ... FROM '<parquet>' [WHERE ...] [LIMIT n [OFFSET m]]` and `EXPLAIN [ANALYZE]` of it (slice 4, `docs/supported-sql.md`).
//! `nata scan` is the minimal developer command of slice 3 (`docs/slices/0001-parquet-scan.md`):
//! it reads Parquet files and prints CSV.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

mod sql;

use clap::{Parser, Subcommand};
use nata_parquet::csv;
use nata_parquet::{ErrorKind, ParallelConfig, ScanError, ScanOptions, ScanPlan, run_parallel};

#[derive(Parser)]
#[command(
    version,
    about = "Nata: lakehouse SQL engine (pre-alpha: SELECT ... FROM '<parquet>' [WHERE ...] [LIMIT ...] only)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve Postgres wire 3.0 simple and unnamed extended queries (trust authentication, no TLS).
    Serve {
        /// Bind address. Loopback only unless --unsafe-listen is explicitly given.
        #[arg(long, default_value = "127.0.0.1:5432")]
        listen: std::net::SocketAddr,
        /// UNSAFE: allow non-loopback clients without TLS or authentication. Local trusted development only.
        #[arg(long)]
        unsafe_listen: bool,
        /// Authenticated sessions (1..=64); eight additional bounded startup/cancel slots.
        #[arg(long, default_value_t = 64)]
        max_connections: usize,
        /// Total CPU worker budget shared by admitted queries (default: available parallelism).
        #[arg(long)]
        threads: Option<usize>,
        /// Concurrent queries (default: CPU worker budget).
        #[arg(long)]
        max_queries: Option<usize>,
    },
    /// Start a node.
    Run,
    /// Run one SQL statement and print the result: `SELECT ... FROM '<file or glob>' [WHERE ...] [LIMIT n [OFFSET m]]`, or `EXPLAIN [ANALYZE]` of it.
    /// The query is the argument of -c, or stdin. Exit codes: 0 success, 1 a data or I/O error, 2 not supported yet / syntax / bind / usage, 130 Ctrl-C.
    Sql {
        /// SQL text to run (default: read the query from stdin).
        #[arg(short = 'c', long = "command")]
        command: Option<String>,
        /// Print the result as CSV (header row, then data rows; NULL as `NULL`) instead of an aligned table.
        #[arg(long)]
        csv: bool,
        /// Worker threads (default: the available parallelism).
        #[arg(long)]
        threads: Option<usize>,
        /// Row groups in flight at once (default: twice the thread count). Developer option.
        #[arg(long)]
        window: Option<usize>,
        /// Print rows as row groups finish, in no promised order (default: the order of the files, row groups and rows). Developer option.
        #[arg(long)]
        unordered: bool,
        /// After a successful run print the one-line report of the scan (as `nata scan`) and the peak resident memory on stderr. Developer option.
        #[arg(long)]
        report: bool,
        /// Decode the filter columns first and the other columns only for the rows that pass the pushed filter (register item P1). Same rows as without.
        /// Developer option, off by default (the slice-4 benchmark failed the keep rule); also on when the environment variable NATA_LATE_MATERIALIZATION is 1.
        #[arg(long)]
        late_materialization: bool,
        /// Files whose footers may be pending at once: read ahead while earlier row groups run (register item P2). Default 8; set 1
        /// for one file at a time. Developer option; the environment variable NATA_FOOTERS_AHEAD gives the default.
        #[arg(long)]
        footers_ahead: Option<usize>,
        /// Check the schema of EVERY file of a glob before the query starts (what the binder did before register item P10), instead of reading only the first
        /// file's footer and letting the scan check the others as it reaches them. Developer option, for the benchmark.
        #[arg(long)]
        bind_all_footers: bool,
        /// Query-wide aggregate memory cap, in bytes.
        #[arg(long)]
        aggregate_memory_bytes: Option<usize>,
        /// Explicit bounded aggregate spill quota, in bytes.
        #[arg(long)]
        spill_disk_bytes: Option<u64>,
        /// Existing disk directory for private query spill files.
        #[arg(long)]
        spill_directory: Option<PathBuf>,
        /// Differential-test mode: route aggregate states through spill IPC.
        #[arg(long, hide = true)]
        force_aggregate_spill: bool,
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
        /// Worker threads (default: the available parallelism). Memory is bounded by the window (default: twice this number) times the largest row group.
        #[arg(long)]
        threads: Option<usize>,
        /// Print rows as row groups finish, in no promised order (default: the order of the files, row groups and rows, for any thread count).
        #[arg(long)]
        unordered: bool,
        /// Row groups in flight at once, running or finished and waiting for the writer (default: twice the thread count, chosen from the slice 3 benchmark: bench/results/20261106-scan-bench.md). Developer option.
        #[arg(long)]
        window: Option<usize>,
        /// Decode the filter columns first and the other columns only for the rows that pass the filter (register item P1). Same rows as without.
        /// Developer option, off by default (the slice-4 benchmark failed the keep rule); also on when the environment variable NATA_LATE_MATERIALIZATION is 1.
        #[arg(long)]
        late_materialization: bool,
        /// Files whose footers may be pending at once: read ahead while earlier row groups run (register item P2). Default 8; set 1
        /// for one file at a time. Developer option; the environment variable NATA_FOOTERS_AHEAD gives the default.
        #[arg(long)]
        footers_ahead: Option<usize>,
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
pub(crate) fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// The developer switch for late materialization: the flag, or `NATA_LATE_MATERIALIZATION=1` (so that `nata-check` and the fuzzers, which run the
/// command as a process, can run a whole query set in the late mode).
pub(crate) fn late_materialization(flag: bool) -> bool {
    flag || std::env::var("NATA_LATE_MATERIALIZATION").is_ok_and(|v| v == "1")
}

/// `--footers-ahead`, else `NATA_FOOTERS_AHEAD`, else 8 (slice-4 benchmark; set 1 for one file at a time).
pub(crate) fn footers_ahead_of(flag: Option<usize>) -> usize {
    flag.or_else(|| {
        std::env::var("NATA_FOOTERS_AHEAD")
            .ok()
            .and_then(|v| v.parse().ok())
    })
    .unwrap_or(8)
}

struct ScanArgs {
    late: bool,
    footers_ahead: usize,
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
        late,
        footers_ahead,
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
    if threads == Some(0) || window == Some(0) || footers_ahead == 0 {
        let _ = writeln!(
            std::io::stderr(),
            "error: --threads, --window and --footers-ahead must be at least 1"
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
        late_materialization: late,
        strict_schema: false,
    };
    let result = (|| {
        let plan = Arc::new(ScanPlan::open(&files, &options)?);
        let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
        let stdout = std::io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        let io_error = |e: std::io::Error| ScanError::new(ErrorKind::Io(e.to_string()));
        let cfg = ParallelConfig {
            threads,
            // 2 x threads: on SF10 the 8-thread scan is 1.4x faster than with window = threads on both CPU types (scan benchmark)
            window: window.unwrap_or(threads.saturating_mul(2)),
            ordered: !unordered,
            footers_ahead,
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
        Command::Serve {
            listen,
            unsafe_listen,
            max_connections,
            threads,
            max_queries,
        } => {
            std::panic::set_hook(Box::new(|_| {
                let _ = writeln!(std::io::stderr(), "error: internal server error");
            }));
            let config = nata_pgwire::server::Config {
                listen,
                unsafe_listen,
                max_connections,
                max_queries: max_queries.unwrap_or_else(|| {
                    threads.unwrap_or_else(|| {
                        std::thread::available_parallelism().map_or(1, usize::from)
                    })
                }),
                threads: threads.unwrap_or_else(|| {
                    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
                }),
                metrics: None,
            };
            let result = config.validate().and_then(|()| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .and_then(|r| r.block_on(nata_pgwire::server::serve(config)))
            });
            return match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    let _ = writeln!(std::io::stderr(), "error: {e}");
                    ExitCode::from(2)
                }
            };
        }
        Command::Run => "`nata run` (node startup)",
        Command::Sql {
            command,
            csv,
            threads,
            window,
            unordered,
            report,
            late_materialization: late,
            footers_ahead,
            bind_all_footers,
            aggregate_memory_bytes,
            spill_disk_bytes,
            spill_directory,
            force_aggregate_spill,
        } => {
            return sql::run(sql::SqlArgs {
                command,
                csv,
                threads,
                window,
                unordered,
                report,
                late: late_materialization(late),
                footers_ahead: footers_ahead_of(footers_ahead),
                bind_all_footers,
                aggregate_memory_bytes,
                spill_disk_bytes,
                spill_directory,
                force_aggregate_spill,
            });
        }
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
            late_materialization: late,
            footers_ahead,
        } => {
            if !csv && !count {
                eprintln!("error: not supported yet: output other than --csv or --count");
                return ExitCode::from(2);
            }
            return scan(ScanArgs {
                late: late_materialization(late),
                footers_ahead: footers_ahead_of(footers_ahead),
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
