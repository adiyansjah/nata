# Snapshot notes (spikes/fairness)

Copied, not edited (sha256 equal to the source file at the time of copying), except `ops.rs` (see below):

    spikes/async-bridge/src/{api,batch}.rs                    (which are the committed operator-trait files, 7f2e3be)
    spikes/async-bridge/src/{chan,sync}.rs
    spikes/operator-trait/src/ops.rs, tests/{protocol,shutdown}.rs   (tests: only the crate name changed)
    spikes/async-bridge/tests/loom_bridge.rs                  (crate name changed; new `y_*` models appended)

`src/ops.rs` was edited later the same day (open spill files: `spill()` opens, appends and closes each partition file within one call; an `FdCount` counter was added; see section 10 of `bench/results/20261104-fairness.md`). The diff against `spikes/operator-trait/src/ops.rs` is `diff -u`.

`src/sched.rs` is `spikes/async-bridge/src/sched.rs` (sha256 6dcc6ddef75d62f902f74f2e1ccd3db636006981648bdc2b37ce5ed994dcec22)
plus the fairness change. New files: `src/load.rs`, `tests/{starvation,fairness}.rs`, `examples/overhead.rs`,
`mutants/`. The tokio harness of the bridge spike is not copied (no tokio dependency here).

Diff of `src/sched.rs` against the bridge spike's copy (`diff -u`):

```diff
--- /tmp/sched_baseline.rs	2026-11-04 10:50:52.384491267 +0700
+++ src/sched.rs	2026-11-04 10:55:18.836661396 +0700
@@ -4,10 +4,17 @@
 //! `Blocked` is parked; `Waker::wake` moves it back to the queue. A wake that arrives while the
 //! task is still running (after it stored the waker, before it parked) is remembered in
 //! `notified`, so no wake-up is lost.
+//!
+//! Fairness (added by the fairness spike): operators are synchronous and cannot be preempted, so
+//! fairness is cooperative, at morsel boundaries. A task runs for a bounded *slice* (a number of
+//! morsels, and/or a time budget checked between morsels); then, if another task is waiting, it
+//! returns `Step::Yield` and is re-queued at the BACK of the run queue. `Yield` is a fourth way to
+//! leave `Running`: `Running -> Queued` (never `Parked`, never `Done`), see `run_task`.
 
 use std::collections::VecDeque;
 use std::sync::Arc;
 use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
+use std::time::{Duration, Instant};
 
 use crate::sync::thread::{self, JoinHandle};
 use crate::sync::{AtomicBool, AtomicUsize, Condvar, Mutex, MutexGuard};
@@ -20,6 +27,52 @@
 /// can hit that race every time instead of one run in a thousand. Zero (the default) disables it.
 pub static TEST_WORKER_DELAY_US: AtomicU64 = AtomicU64::new(0);
 
+/// How long a task may run before it must give the pool to other runnable tasks.
+///
+/// A slice ends at the first morsel boundary where `max_morsels` morsels have been processed or
+/// `max_time` has elapsed since the slice began (whichever comes first). At least one morsel is
+/// always processed per slice, so a task makes progress however small the budget is.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub struct SliceConfig {
+    /// Morsels (batches taken from the source) per slice. `usize::MAX` = no limit. Minimum 1.
+    pub max_morsels: usize,
+    /// Wall-clock budget per slice, checked between morsels only. `None` = no limit.
+    pub max_time: Option<Duration>,
+}
+
+impl SliceConfig {
+    /// No time slicing: a task keeps its worker until it blocks or finishes (the behaviour before
+    /// the fairness change; the control arm of the tests and measurements).
+    pub const fn unlimited() -> Self {
+        SliceConfig {
+            max_morsels: usize::MAX,
+            max_time: None,
+        }
+    }
+
+    pub const fn morsels(n: usize) -> Self {
+        SliceConfig {
+            max_morsels: if n == 0 { 1 } else { n },
+            max_time: None,
+        }
+    }
+
+    fn exhausted(&self, morsels: usize, started: Option<Instant>) -> bool {
+        morsels >= self.max_morsels
+            || matches!((self.max_time, started), (Some(t), Some(s)) if s.elapsed() >= t)
+    }
+}
+
+impl Default for SliceConfig {
+    /// 32 morsels or 1 ms, whichever comes first.
+    fn default() -> Self {
+        SliceConfig {
+            max_morsels: 32,
+            max_time: Some(Duration::from_millis(1)),
+        }
+    }
+}
+
 pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
     m.lock().unwrap_or_else(|e| e.into_inner())
 }
@@ -64,6 +117,9 @@
 }
 
 struct Shared {
+    slice: SliceConfig,
+    /// Number of times a task gave up its worker at the end of a slice (statistics; also a test hook).
+    yields: AtomicUsize,
     queue: Mutex<VecDeque<Arc<TaskCell>>>,
     queue_cv: Condvar,
     pipelines: Vec<PipelineRt>,
@@ -78,6 +134,10 @@
 }
 
 impl Shared {
+    fn queue_is_empty(&self) -> bool {
+        lock(&self.queue).is_empty()
+    }
+
     fn enqueue(&self, t: Arc<TaskCell>) {
         lock(&self.queue).push_back(t);
         self.queue_cv.notify_one();
@@ -193,6 +253,8 @@
 enum Step {
     Done,
     Blocked,
+    /// The slice is used up and another task is waiting: re-queue at the back.
+    Yield,
 }
 
 impl TaskBody {
@@ -249,6 +311,9 @@
     }
 
     fn run(&mut self, waker: &Waker, shared: &Shared, p: &PipelineRt) -> Result<Step> {
+        let slice = shared.slice;
+        let mut morsels = 0usize;
+        let mut started = slice_clock(&slice);
         loop {
             if shared.cancelled.load(SeqCst) || p.stop.load(SeqCst) {
                 return Ok(Step::Done);
@@ -259,11 +324,27 @@
                     p.stop.store(true, SeqCst);
                     return Ok(Step::Done);
                 }
-                Flow::NeedSource => match self.source.next(waker)? {
-                    SourceResult::Batch(b) => self.inputs[0] = Some(b),
-                    SourceResult::Blocked => return Ok(Step::Blocked),
-                    SourceResult::Finished => return Ok(Step::Done),
-                },
+                Flow::NeedSource => {
+                    // A morsel boundary: every stage is drained, so nothing is half-processed.
+                    if morsels >= 1 && slice.exhausted(morsels, started) {
+                        if !shared.queue_is_empty() {
+                            return Ok(Step::Yield);
+                        }
+                        // Nobody is waiting: start a new slice instead of paying for a round trip
+                        // through the queue. A task that becomes runnable from now on waits at
+                        // most one more slice.
+                        morsels = 0;
+                        started = slice_clock(&slice);
+                    }
+                    match self.source.next(waker)? {
+                        SourceResult::Batch(b) => {
+                            self.inputs[0] = Some(b);
+                            morsels += 1;
+                        }
+                        SourceResult::Blocked => return Ok(Step::Blocked),
+                        SourceResult::Finished => return Ok(Step::Done),
+                    }
+                }
             }
         }
     }
@@ -275,7 +356,17 @@
         !task.in_run.swap(true, SeqCst),
         "invariant violated: task run by two workers at once"
     );
-    *lock(&task.state) = TaskState::Running { notified: false };
+    {
+        let mut st = lock(&task.state);
+        // Added for the fairness spike: a task is only ever started from `Queued` (so never from
+        // `Parked`, never twice, never after `Done`).
+        #[cfg(any(nata_loom, feature = "invariants"))]
+        assert!(
+            matches!(*st, TaskState::Queued),
+            "invariant violated: task started from a state other than Queued"
+        );
+        *st = TaskState::Running { notified: false };
+    }
     let waker = task.waker();
     let p = &shared.pipelines[task.pipeline];
     let result = lock(&task.body).run(&waker, shared, p);
@@ -294,6 +385,18 @@
                 *st = TaskState::Parked;
             }
         }
+        Ok(Step::Yield) => {
+            let mut st = lock(&task.state);
+            // Cleared while holding the state lock, before the task becomes runnable again.
+            #[cfg(any(nata_loom, feature = "invariants"))]
+            task.in_run.store(false, SeqCst);
+            // A wake that arrived while Running (`notified`) is subsumed: the task is runnable
+            // anyway. Not `Parked`: nobody would wake a task that is not waiting for anything.
+            *st = TaskState::Queued;
+            drop(st);
+            shared.yields.fetch_add(1, SeqCst);
+            shared.enqueue(task); // back of the queue
+        }
         Ok(Step::Done) => finish_task(shared, &task),
         Err(e) => {
             shared.fail(e);
@@ -355,6 +458,7 @@
 
 pub struct Query {
     pipelines: Vec<Pipeline>,
+    slice: SliceConfig,
 }
 
 pub struct RunningQuery {
@@ -364,7 +468,17 @@
 
 impl Query {
     pub fn new(pipelines: Vec<Pipeline>) -> Self {
-        Query { pipelines }
+        Query {
+            pipelines,
+            slice: default_slice(),
+        }
+    }
+
+    /// Set the time-slice policy (default: [`SliceConfig::default`]).
+    pub fn with_slice(mut self, slice: SliceConfig) -> Self {
+        slice_check(&slice);
+        self.slice = slice;
+        self
     }
 
     pub fn run(self, threads: usize) -> Result<()> {
@@ -402,6 +516,8 @@
             })
             .collect();
         let shared = Arc::new(Shared {
+            slice: self.slice,
+            yields: AtomicUsize::new(0),
             queue: Mutex::new(VecDeque::new()),
             queue_cv: Condvar::new(),
             pending_pipelines: AtomicUsize::new(pipelines.len()),
@@ -437,13 +553,54 @@
     }
 }
 
+/// Test hook (feature `slice-env`, off by default): `NATA_SLICE_MORSELS=n` makes every `Query::new`
+/// use `n` morsels per slice and no time budget, so the whole existing test suite can be re-run with
+/// a slice of one morsel, which exercises the yield path at every morsel boundary.
+fn default_slice() -> SliceConfig {
+    #[cfg(feature = "slice-env")]
+    if let Some(n) = std::env::var("NATA_SLICE_MORSELS")
+        .ok()
+        .and_then(|v| v.parse::<usize>().ok())
+    {
+        return SliceConfig::morsels(n);
+    }
+    SliceConfig::default()
+}
+
+/// Start of a slice for the time budget. The model checker has no clock, so under `--cfg nata_loom`
+/// only the morsel budget exists (the time budget changes when the yield decision fires, not what
+/// the yield does).
+fn slice_clock(slice: &SliceConfig) -> Option<Instant> {
+    #[cfg(not(nata_loom))]
+    return slice.max_time.map(|_| Instant::now());
+    #[cfg(nata_loom)]
+    {
+        let _ = slice;
+        None
+    }
+}
+
+fn slice_check(s: &SliceConfig) {
+    assert!(s.max_morsels >= 1, "a slice must allow at least one morsel");
+}
+
 impl RunningQuery {
+    /// How many times a task gave up its worker at the end of a slice so far.
+    pub fn yields(&self) -> usize {
+        self.shared.yields.load(SeqCst)
+    }
+
     /// Ask the query to stop. `wait` then returns the cancellation error.
     pub fn cancel(&self) {
         self.shared.fail(QueryError("cancelled".into()));
     }
 
     pub fn wait(self) -> Result<()> {
+        self.wait_with_stats().0
+    }
+
+    /// Like [`wait`](Self::wait), also returning the total number of yields.
+    pub fn wait_with_stats(self) -> (Result<()>, usize) {
         {
             let mut done = lock(&self.shared.done);
             while !*done {
@@ -468,10 +625,14 @@
         lock(&self.shared.queue).clear();
         lock(&self.shared.all_tasks).clear();
         let err = lock(&self.shared.error).take();
-        match err {
-            Some(e) => Err(e),
-            None => Ok(()),
-        }
+        let yields = self.shared.yields.load(SeqCst);
+        (
+            match err {
+                Some(e) => Err(e),
+                None => Ok(()),
+            },
+            yields,
+        )
     }
 
     /// Names of the pipelines, for diagnostics.
```
