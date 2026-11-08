//! `nata sql`: parse, bind and plan a query (`nata-sql`), run it (`nata-exec`), print the result as CSV or an aligned table.
//!
//! Exit codes (as `nata scan`): 0 success, 1 a data or I/O error (a missing, damaged or truncated file, a division by zero in the data, an overflow),
//! 2 `not supported yet`, a syntax or bind error or a wrong use of the command, 130 interrupted (Ctrl-C). A closed stdout (`nata sql ... | head`) is
//! not a failure: the query is cancelled quietly and the exit code is 0. If rows were printed before an error, the exit code is not 0 and the error
//! says that the output is incomplete.

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nata_exec::explain::analyze_text;
use nata_exec::output::{
    Format, ResultWriter, exit_code_scan, exit_code_sql, render_scan_error, render_sql_error,
};
use nata_exec::{ExecConfig, Observed, execute_observed};
use nata_parquet::schema::Leaf;
use nata_parquet::{ErrorKind, ScanCancel, ScanError};
use nata_sql::ast::ExplainMode;
use nata_sql::{BindOptions, FsSource, RoutedSource, SourceProvider, plan_sql_with};
use signal_hook::consts::SIGINT;
use signal_hook::iterator::Signals;

/// The largest query text read from stdin: the dialect's limit is 1 MiB, one more byte is enough for the parser to refuse it.
const MAX_QUERY_BYTES: u64 = (1 << 20) + 1;

pub struct SqlArgs {
    pub command: Option<String>,
    pub csv: bool,
    pub threads: Option<usize>,
    pub window: Option<usize>,
    pub unordered: bool,
    pub report: bool,
    pub late: bool,
    pub footers_ahead: usize,
    pub bind_all_footers: bool,
    pub aggregate_memory_bytes: Option<usize>,
    pub spill_disk_bytes: Option<u64>,
    pub spill_directory: Option<PathBuf>,
    pub force_aggregate_spill: bool,
    pub object_memory_bytes: Option<usize>,
}

/// Wraps stdout and remembers that the reader went away.
struct Out<W: Write> {
    inner: W,
    closed: Arc<AtomicBool>,
}

impl<W: Write> Write for Out<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf).inspect_err(|e| {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                self.closed.store(true, Ordering::SeqCst);
            }
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().inspect_err(|e| {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                self.closed.store(true, Ordering::SeqCst);
            }
        })
    }
}

/// The file system, except that a pending interrupt stops the reading of footers (a glob of 10,000 files reads 10,000 of them before the query starts).
struct InterruptibleSource {
    interrupted: Arc<AtomicBool>,
}

impl SourceProvider for InterruptibleSource {
    fn expand(&self, pattern: &str) -> Result<Vec<PathBuf>, String> {
        FsSource.expand(pattern)
    }

    fn leaves(&self, file: &Path) -> Result<Vec<Leaf>, ScanError> {
        if self.interrupted.load(Ordering::SeqCst) {
            return Err(ScanError::new(ErrorKind::Cancelled(
                "interrupted while reading the files".into(),
            )));
        }
        FsSource.leaves(file)
    }
}

fn say(text: &str) {
    // not eprintln: a closed stderr must not be a panic
    let _ = std::io::stderr().write_all(text.as_bytes());
}

fn read_query(a: &SqlArgs) -> Result<String, ExitCode> {
    if let Some(c) = &a.command {
        return Ok(c.clone());
    }
    let mut buf = Vec::new();
    if let Err(e) = std::io::stdin()
        .lock()
        .take(MAX_QUERY_BYTES + 1)
        .read_to_end(&mut buf)
    {
        say(&format!("error: cannot read the query from stdin: {e}\n"));
        return Err(ExitCode::from(1));
    }
    match String::from_utf8(buf) {
        Ok(s) => Ok(s),
        Err(_) => {
            say("error: the query on stdin is not valid UTF-8\n");
            Err(ExitCode::from(2))
        }
    }
}

pub fn run(a: SqlArgs) -> ExitCode {
    if a.threads == Some(0) || a.window == Some(0) || a.footers_ahead == 0 {
        say("error: --threads, --window and --footers-ahead must be at least 1\n");
        return ExitCode::from(2);
    }
    // a panic must not print its message (it could hold a value from a file): location only
    std::panic::set_hook(Box::new(|info| {
        let at = info
            .location()
            .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        say(&format!("error: internal error{at}\n"));
    }));
    let sql = match read_query(&a) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let cancel = ScanCancel::new();
    let (signals_handle, watcher) = match Signals::new([SIGINT]) {
        Ok(mut signals) => {
            let handle = signals.handle();
            let (flag, c) = (Arc::clone(&interrupted), cancel.clone());
            let t = std::thread::Builder::new()
                .name("nata-sigint".into())
                .spawn(move || {
                    for _ in signals.forever() {
                        flag.store(true, Ordering::SeqCst);
                        c.cancel();
                    }
                })
                .ok();
            (Some(handle), t)
        }
        Err(_) => (None, None),
    };
    // A destination, rather than a logging flag, opts into query attribution.
    // Keep the owner through sink flush, execution teardown and signal join.
    let destination = std::env::var_os("NATA_STATS_JSON").map(PathBuf::from);
    let stats = (destination.is_some() && nata_exec::query_stats::QueryStats::available())
        .then(nata_exec::query_stats::QueryStats::new);
    let mut report = None;
    let code = run_query(&a, &sql, &interrupted, &cancel, &stats, &mut report);
    // every thread is joined before the process ends
    if let Some(h) = signals_handle {
        h.close();
    }
    if let Some(t) = watcher {
        let _ = t.join();
    }
    if let (Some(path), Some(stats)) = (destination, stats) {
        let record = if let Some(mut report) = report {
            report.query_stats = Some(stats.json());
            report.stats_json().expect("stats owner")
        } else {
            let raw = stats.json();
            format!(
                "{},\"scan\":null,\"aggregate\":null}}",
                raw.trim_end_matches('}')
            )
        };
        // A failed sidecar is observable by exit status and a missing record;
        // never insert instrumentation diagnostics into query stderr.
        if nata_exec::query_stats::write_json(&path, &record).is_err() {
            return ExitCode::from(1);
        }
    }
    code
}

fn run_query(
    a: &SqlArgs,
    sql: &str,
    interrupted: &Arc<AtomicBool>,
    cancel: &ScanCancel,
    query_stats: &Option<Arc<nata_exec::query_stats::QueryStats>>,
    final_report: &mut Option<nata_exec::ExecReport>,
) -> ExitCode {
    let bind_stats = nata_exec::query_stats::Timer::new(
        query_stats,
        nata_exec::query_stats::Stage::BindMetadata,
    );
    let source = InterruptibleSource {
        interrupted: Arc::clone(interrupted),
    };
    // `s3://` and `gs://` paths and globs: read through the object-store reader, which starts on first use with the credentials of the environment
    let remote: Arc<dyn nata_parquet::RemoteIo> = Arc::new(nata_objstore::LazyObjectIo::from_env(
        a.object_memory_bytes
            .unwrap_or_else(|| crate::default_object_memory(1)),
    ));
    let remote = remote.scoped().unwrap_or(remote);
    let stop = || interrupted.load(Ordering::SeqCst);
    let routed = RoutedSource {
        local: &source,
        remote: remote.as_ref(),
        cancel: &stop,
    };
    let planned = match plan_sql_with(
        sql,
        &routed,
        BindOptions {
            all_footers: a.bind_all_footers,
        },
    ) {
        Ok(p) => p,
        Err(e) => {
            if interrupted.load(Ordering::SeqCst) {
                say("interrupted\n");
                return ExitCode::from(130);
            }
            say(&render_sql_error(sql, &e));
            return ExitCode::from(exit_code_sql(&e));
        }
    };
    drop(bind_stats);
    if interrupted.load(Ordering::SeqCst) {
        say("interrupted\n");
        return ExitCode::from(130);
    }
    if planned.mode == ExplainMode::Plan {
        // a closed stdout is not a failure of the query: the result of the write is not looked at
        let text = planned.plan.explain_text();
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
        return ExitCode::SUCCESS;
    }
    let threads = a
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
    let mut cfg = ExecConfig::new(threads);
    cfg.query_stats = query_stats.clone();
    if let Some(w) = a.window {
        cfg.window = w;
    }
    cfg.aggregate_memory_bytes = a.aggregate_memory_bytes;
    cfg.aggregate_spill_disk_bytes = a.spill_disk_bytes;
    cfg.aggregate_spill_directory = a.spill_directory.clone();
    cfg.aggregate_force_spill = a.force_aggregate_spill;
    cfg.ordered = !a.unordered;
    cfg.late_materialization = a.late;
    cfg.footers_ahead = a.footers_ahead;
    cfg.remote = Some(Arc::clone(&remote));
    if planned
        .plan
        .scan
        .files
        .iter()
        .any(|f| f.to_str().is_some_and(nata_parquet::is_remote_location))
    {
        cfg.window = a.window.unwrap_or(16);
        let pool = a
            .aggregate_memory_bytes
            .unwrap_or_else(nata_exec::default_memory_pool);
        cfg.aggregate_memory_bytes = match nata_exec::default_memory_pool()
            .checked_sub(remote.reserved_bytes())
            .filter(|p| *p > 0)
        {
            Some(p) => Some(pool.min(p)),
            None => {
                say("error: object-store reservation exceeds node memory pool\n");
                return ExitCode::from(1);
            }
        };
    }
    let names: Vec<String> = planned.output.iter().map(|c| c.name.clone()).collect();
    let types: Vec<_> = planned.output.iter().map(|c| c.ty).collect();
    let plan = Arc::new(planned.plan);
    let closed = Arc::new(AtomicBool::new(false));
    let stdout = std::io::stdout();
    let format = if a.csv { Format::Csv } else { Format::Table };

    if planned.mode == ExplainMode::Analyze {
        // run it, count what each operator did, print the plan with the numbers; the rows themselves are not printed
        let mut observed = Observed::default();
        return match execute_observed(Arc::clone(&plan), &cfg, Some(cancel), &mut observed, |_| {
            Ok(())
        }) {
            Ok(report) => {
                let text = analyze_text(&plan, &report);
                let mut out = stdout.lock();
                let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
                *final_report = Some(report);
                ExitCode::SUCCESS
            }
            Err(e) => finish_error(sql, &e, 0, interrupted, &closed),
        };
    }

    let mut writer = ResultWriter::new(
        Out {
            inner: BufWriter::new(stdout.lock()),
            closed: Arc::clone(&closed),
        },
        format,
        names,
        &types,
    );
    let mut observed = Observed::default();
    let result = execute_observed(plan, &cfg, Some(cancel), &mut observed, |batch| {
        writer.write_batch(batch)
    });
    match result {
        Ok(report) => {
            let finish = {
                let _finish = nata_exec::query_stats::Timer::new(
                    query_stats,
                    nata_exec::query_stats::Stage::OutputSink,
                );
                writer.finish()
            };
            match finish {
                Ok(()) => {
                    if a.report {
                        let rss = crate::peak_rss_kib()
                            .map_or(String::new(), |k| format!(" peak_rss_kib={k}"));
                        say(&format!("{}{rss}\n", report.scan.line()));
                        if let Some(a) = &report.aggregate {
                            say(&format!("{}\n", a.line()));
                        }
                    }
                    *final_report = Some(report);
                    ExitCode::SUCCESS
                }
                Err(e) => finish_error(sql, &e, writer.rows_written(), interrupted, &closed),
            }
        }
        Err(e) => {
            // what was printed so far goes out before the message, so that the two are in the order they happened
            let printed = writer.rows_written();
            let _ = writer.flush_partial();
            finish_error(sql, &e, printed, interrupted, &closed)
        }
    }
}

fn finish_error(
    sql: &str,
    e: &ScanError,
    printed: u64,
    interrupted: &AtomicBool,
    closed: &AtomicBool,
) -> ExitCode {
    // `nata sql ... | head`: the reader went away, which is not a failure of the query
    if closed.load(Ordering::SeqCst) {
        return ExitCode::SUCCESS;
    }
    if matches!(e.kind, ErrorKind::Cancelled(_)) && interrupted.load(Ordering::SeqCst) {
        let note = if printed > 0 {
            format!(
                "interrupted: the query was stopped after {printed} row{} had been printed; the output is INCOMPLETE\n",
                if printed == 1 { "" } else { "s" }
            )
        } else {
            "interrupted\n".to_string()
        };
        say(&note);
        return ExitCode::from(130);
    }
    say(&render_scan_error(sql, e, printed));
    ExitCode::from(exit_code_scan(e))
}
