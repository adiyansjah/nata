//! The text of `EXPLAIN ANALYZE`: the plan of `EXPLAIN` with the rows and the time of every operator, then what the scan reports.
//!
//! Times: an operator's time is CPU time summed over the worker threads that ran it (it can exceed the wall time); the scan line carries the wall time of
//! the whole run, which includes the operators below the consumer. Every time is printed as `time=<n.nn>ms`, which is what the golden tests mask.

use nata_sql::plan::LogicalPlan;

use crate::exec::ExecReport;

fn ms(d: std::time::Duration) -> String {
    format!("{:.2}ms", d.as_secs_f64() * 1000.0)
}

/// The annotated plan: each operator line of `plan.explain_text()` gets `(rows ..., time=...)`, the scan's lines are followed by its report.
pub fn analyze_text(plan: &LogicalPlan, r: &ExecReport) -> String {
    let (ops, scan) = (&r.ops, &r.scan);
    let mut out = String::new();
    for line in plan.explain_text().lines() {
        out.push_str(line);
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        if t.starts_with("Limit ") {
            out.push_str(&format!(
                "  (rows in={}, out={})",
                ops.limit_in, ops.rows_out
            ));
        } else if t.starts_with("Project ") {
            out.push_str(&format!(
                "  (rows={}, time={})",
                ops.project_rows,
                ms(ops.project_time)
            ));
        } else if t.starts_with("Filter ") {
            out.push_str(&format!(
                "  (rows in={}, out={}, time={})",
                ops.scan_rows,
                ops.filter_rows,
                ms(ops.filter_time)
            ));
        } else if t.starts_with("Scan ") {
            out.push_str(&format!(
                "  (rows={}, time={} wall, includes the operators above)",
                ops.scan_rows,
                ms(r.elapsed)
            ));
        }
        out.push('\n');
        if t.starts_with("Scan ") {
            let pad = format!("{indent}  ");
            out.push_str(&format!(
                "{pad}files: {} (opened {}), row groups: total={} skipped={} read={} abandoned={}\n",
                scan.files,
                scan.files_opened,
                scan.row_groups_total,
                scan.row_groups_skipped,
                scan.row_groups_read,
                scan.row_groups_abandoned,
            ));
            out.push_str(&format!(
                "{pad}rows read={} (before the pushed filter), rows returned by the scan={}, bytes read={}, columns read={}\n",
                scan.rows_read, ops.scan_rows, scan.bytes_read, scan.columns_read
            ));
            let pruning = if scan.pruning.applied {
                format!(
                    "applied({} of {} terms)",
                    scan.pruning.pushed, scan.pruning.terms
                )
            } else if scan.pruning.terms == 0 {
                "none(no pushed filter)".to_string()
            } else {
                "none(no term can be used)".to_string()
            };
            out.push_str(&format!(
                "{pad}pruning={pruning}, threads={}, window={}, order={}\n",
                scan.threads,
                scan.window,
                if scan.ordered { "ordered" } else { "unordered" }
            ));
            out.push_str(&format!(
                "{pad}memory: peak_inflight_bytes={} largest_task_bytes={} peak_open_files={} peak_footers={}\n",
                scan.peak_inflight_bytes,
                scan.largest_task_bytes,
                scan.peak_open_files,
                scan.peak_footers
            ));
        }
    }
    if let Some(metrics) = &scan.remote_metrics {
        out.push_str(metrics);
        out.push('\n');
    }
    if let Some(a) = &r.aggregate {
        out.push_str(&format!("{}\n", a.line()));
    }
    out.push_str(&format!(
        "total: rows returned={}, time={} wall\n",
        ops.rows_out,
        ms(r.elapsed)
    ));
    out
}
