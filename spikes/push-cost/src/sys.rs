//! What else is running on the machine. Linux only; everything degrades to `None`/NaN elsewhere.
//!
//! The 1-minute load average INCLUDES this benchmark's own worker threads (4 busy threads push it towards 4 within a
//! minute), so on its own it cannot say whether anyone else is using the machine. Two measures are therefore used:
//! the load average read BEFORE the benchmark starts any thread (the run-level "quiet" test, threshold 0.5, the same as ADR 0006 rule 7), and the
//! CPU time that was used by other processes (or stolen by the hypervisor) during each cell: the machine's busy time
//! from /proc/stat minus this process's own CPU time. USER_HZ is assumed to be 100 (true on Linux x86-64 and ARM64).

use std::time::Instant;

pub const QUIET_LOAD_LIMIT: f64 = 0.5;
/// A cell is "quiet" only if less than this many cores' worth of CPU went to other processes during the cell.
pub const QUIET_FOREIGN_CORES: f64 = 0.5;
const USER_HZ: f64 = 100.0;

pub fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok()))
        .unwrap_or(f64::NAN)
}

pub fn ncpu() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Parse the first line of /proc/stat: busy jiffies of the whole machine (user + nice + system + irq + softirq + steal;
/// guest time is already inside user).
pub fn parse_busy_jiffies(stat: &str) -> Option<u64> {
    let line = stat.lines().next()?;
    let mut it = line.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    let v: Vec<u64> = it.map_while(|x| x.parse().ok()).collect();
    if v.len() < 7 {
        return None;
    }
    let steal = v.get(7).copied().unwrap_or(0);
    Some(v[0] + v[1] + v[2] + v[5] + v[6] + steal)
}

/// Parse /proc/self/stat: utime + stime of the whole process (all its threads, including finished ones), in jiffies.
pub fn parse_own_jiffies(stat: &str) -> Option<u64> {
    let rest = stat.rsplit_once(')')?.1;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // after ")": state is field 3 (index 0), utime is field 14 (index 11), stime field 15 (index 12)
    Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
}

#[derive(Clone, Copy)]
pub struct CpuSnap {
    at: Instant,
    busy: u64,
    own: u64,
}

pub fn snap() -> Option<CpuSnap> {
    let busy = parse_busy_jiffies(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let own = parse_own_jiffies(&std::fs::read_to_string("/proc/self/stat").ok()?)?;
    Some(CpuSnap { at: Instant::now(), busy, own })
}

/// Cores' worth of CPU used by everything except this process between two snapshots (NaN if unknown).
pub fn foreign_cores(a: Option<CpuSnap>, b: Option<CpuSnap>) -> f64 {
    let (Some(a), Some(b)) = (a, b) else { return f64::NAN };
    let secs = b.at.duration_since(a.at).as_secs_f64();
    if secs <= 0.0 {
        return f64::NAN;
    }
    let all = b.busy.saturating_sub(a.busy) as f64;
    let own = b.own.saturating_sub(a.own) as f64;
    ((all - own) / USER_HZ / secs).max(0.0)
}

/// The label a cell (or the run) may carry. "quiet" is refused when the load average before the run was >= 2, or when
/// other processes used >= 0.5 core during the cell, or when either is unknown.
pub fn label(load_at_start: f64, foreign: f64) -> &'static str {
    if load_at_start < QUIET_LOAD_LIMIT && foreign < QUIET_FOREIGN_CORES {
        "quiet"
    } else {
        "SANITY ONLY"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_lines_parse() {
        let s = "cpu  100 10 50 1000 20 5 5 7 0 0\ncpu0 1 2 3\n";
        assert_eq!(parse_busy_jiffies(s), Some(100 + 10 + 50 + 5 + 5 + 7));
        assert_eq!(parse_busy_jiffies("intr 1 2 3"), None);
        let own = "123 (push cost) S 1 2 3 4 5 6 7 8 9 10 111 222 0 0 20 0 4 0 9";
        assert_eq!(parse_own_jiffies(own), Some(333));
    }

    #[test]
    fn quiet_is_refused_under_load_or_foreign_use_or_unknown() {
        assert_eq!(label(0.1, 0.0), "quiet");
        assert_eq!(label(0.49, 0.49), "quiet");
        // 0.72 was called "quiet" by the old limit of 2 in the 2026-11-05 ARM64 run; the settle rule is below 0.5
        assert_eq!(label(0.72, 0.0), "SANITY ONLY");
        assert_eq!(label(0.5, 0.0), "SANITY ONLY");
        assert_eq!(label(2.0, 0.0), "SANITY ONLY");
        assert_eq!(label(2.5, 0.0), "SANITY ONLY");
        assert_eq!(label(0.1, 0.5), "SANITY ONLY");
        assert_eq!(label(f64::NAN, 0.0), "SANITY ONLY");
        assert_eq!(label(0.1, f64::NAN), "SANITY ONLY");
    }

    #[test]
    fn foreign_cpu_is_machine_busy_minus_own() {
        let t = Instant::now();
        let a = CpuSnap { at: t, busy: 1000, own: 100 };
        let b = CpuSnap { at: t + std::time::Duration::from_secs(10), busy: 1000 + 4100, own: 100 + 4000 };
        // 41 core-seconds busy, 40 of them ours, over 10 s = 0.1 core
        let f = foreign_cores(Some(a), Some(b));
        assert!((f - 0.1).abs() < 1e-9, "{f}");
        assert!(foreign_cores(None, Some(b)).is_nan());
    }
}
