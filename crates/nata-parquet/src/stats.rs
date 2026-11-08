//! What a scan counts while it runs: events (files opened, row groups read, bytes), and two gauges that give the numbers of the resource bounds
//! (card row "Resource bounds"): bytes held by tasks and by finished results waiting for the consumer, and file handles open.
//! Everything here is plain atomics; none of it takes part in scheduling (the scheduler is `nata-sched`).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A counter that goes up and down and remembers its highest value.
#[derive(Debug, Default)]
pub struct Gauge {
    now: AtomicU64,
    peak: AtomicU64,
}

impl Gauge {
    pub fn add(&self, n: u64) {
        let now = self.now.fetch_add(n, Ordering::SeqCst) + n;
        self.peak.fetch_max(now, Ordering::SeqCst);
    }

    pub fn sub(&self, n: u64) {
        self.now.fetch_sub(n, Ordering::SeqCst);
    }

    pub fn now(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::SeqCst)
    }
}

/// Bytes counted on a [`Gauge`] while this value is alive. A task holds one for what it decodes; the finished result carries it to the consumer, and it
/// is released when the consumer has dropped the result. `grow`/`shrink` follow the task as it decodes batch after batch.
pub struct Reservation {
    gauge: Arc<Gauge>,
    bytes: u64,
    /// The highest value `bytes` had.
    high: u64,
}

impl Reservation {
    pub fn new(gauge: &Arc<Gauge>) -> Reservation {
        Reservation {
            gauge: Arc::clone(gauge),
            bytes: 0,
            high: 0,
        }
    }

    pub fn grow(&mut self, n: u64) {
        self.gauge.add(n);
        self.bytes += n;
        self.high = self.high.max(self.bytes);
    }

    pub fn shrink(&mut self, n: u64) {
        let n = n.min(self.bytes);
        self.gauge.sub(n);
        self.bytes -= n;
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn high(&self) -> u64 {
        self.high
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.gauge.sub(self.bytes);
    }
}

impl fmt::Debug for Reservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Reservation({} bytes)", self.bytes)
    }
}

/// One count on a gauge for as long as the guard lives (a file handle, a footer held in memory).
#[derive(Debug)]
pub struct Counted(Arc<Gauge>);

impl Counted {
    pub fn new(g: &Arc<Gauge>) -> Counted {
        g.add(1);
        Counted(Arc::clone(g))
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.sub(1);
    }
}

/// The counters of one scan, shared by the plan, the tasks and the report.
#[derive(Debug, Default)]
pub struct ScanStats {
    pub query_stats: std::sync::OnceLock<std::sync::Arc<nata_sched::query_stats::QueryStats>>,
    pub files_opened: AtomicU64,
    pub row_groups_total: AtomicU64,
    pub row_groups_skipped: AtomicU64,
    /// Row groups decoded to the end.
    pub row_groups_read: AtomicU64,
    /// Row groups whose task started or was claimed and stopped on cancellation.
    pub row_groups_abandoned: AtomicU64,
    /// Rows decoded, also by tasks that were cancelled halfway.
    pub rows_read: AtomicU64,
    /// Bytes requested from the files by the row-group tasks (counted at the handle), also of cancelled tasks.
    pub bytes_read: AtomicU64,
    /// Decoded bytes held by running tasks and by results not yet released (the scan's own accounting: Arrow's `get_array_memory_size`).
    pub memory: Arc<Gauge>,
    /// File handles open (footer reads and row-group reads).
    pub handles: Arc<Gauge>,
    /// Footers (file metadata) held in memory.
    pub footers: Arc<Gauge>,
    /// The highest bytes any one task held at once.
    pub largest_task: AtomicU64,
}

impl ScanStats {
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::SeqCst);
    }

    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_counts_while_alive_and_remembers_its_high() {
        let g = Arc::new(Gauge::default());
        let mut a = Reservation::new(&g);
        a.grow(100);
        let mut b = Reservation::new(&g);
        b.grow(50);
        assert_eq!((g.now(), g.peak()), (150, 150));
        a.shrink(60);
        assert_eq!((g.now(), a.bytes(), a.high()), (90, 40, 100));
        drop(a);
        assert_eq!((g.now(), g.peak()), (50, 150));
        drop(b);
        assert_eq!(g.now(), 0);
        let c1 = Counted::new(&g);
        let c2 = Counted::new(&g);
        assert_eq!(g.now(), 2);
        drop((c1, c2));
        assert_eq!(g.now(), 0);
    }
}
