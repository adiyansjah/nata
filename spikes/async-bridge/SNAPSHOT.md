# Snapshot of the operator-trait spike

`src/api.rs`, `src/batch.rs`, `src/sched.rs` are copies of the COMMITTED files (not the working tree, which another
agent was editing):

    git show HEAD:spikes/operator-trait/src/{api,batch,sched,lib}.rs     # HEAD = 7f2e3be8b61c72855ab9afbbfd95bc40f59b0d34

sha256 of the committed originals (before any edit here):

    b84b2c713421f828552196e01900f48a20d43a73cd2b9f7fcde22869a4d92b95  api.rs
    328f849057456701ba3064bdcef1ce5df0d438205af7f5ab9633afb45a701733  batch.rs
    5faf49462aa8397f7b090cecaf01edf1a47ac443b3bae90d12f2329a4fe200f7  sched.rs
    7369455f51be364aedd6086b52bdb449566802f4fb4e1491b5b08360c20179af  lib.rs

Differences from the committed files (`diff` of original against the copy here). `api.rs` and `batch.rs` are
unchanged. `lib.rs` is replaced (module list). `ops.rs` was not copied. `sched.rs` differs only by:

1. its imports now come from `src/sync.rs`, which is `std` normally and `loom` under `--cfg nata_loom`
   (Mutex, Condvar, AtomicBool, AtomicUsize, thread::spawn / JoinHandle);
2. one extra field `in_run` and an assertion that a task is never run by two workers at once
   (compiled under `--cfg nata_loom` or the `invariants` feature, which is on by default).

```diff
@@ -6,9 +6,11 @@
 //! `notified`, so no wake-up is lost.
 
 use std::collections::VecDeque;
-use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
-use std::sync::{Arc, Condvar, Mutex, MutexGuard};
-use std::thread::JoinHandle;
+use std::sync::Arc;
+use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
+
+use crate::sync::thread::{self, JoinHandle};
+use crate::sync::{AtomicBool, AtomicUsize, Condvar, Mutex, MutexGuard};
 
 use crate::api::*;
 use crate::batch::Batch;
@@ -43,6 +45,9 @@
     pipeline: usize,
     state: Mutex<TaskState>,
     body: Mutex<TaskBody>,
+    /// Added for the async-bridge spike: set while a worker is inside `run_task` for this task.
+    #[cfg(any(nata_loom, feature = "invariants"))]
+    in_run: AtomicBool,
 }
 
 struct PipelineRt {
@@ -138,6 +143,8 @@
                 pipeline: p,
                 state: Mutex::new(TaskState::Queued),
                 body: Mutex::new(body),
+                #[cfg(any(nata_loom, feature = "invariants"))]
+                in_run: AtomicBool::new(false),
             });
             lock(&self.all_tasks).push(Arc::clone(&cell));
             self.enqueue(cell);
@@ -263,6 +270,11 @@
 }
 
 fn run_task(shared: &Arc<Shared>, task: Arc<TaskCell>) {
+    #[cfg(any(nata_loom, feature = "invariants"))]
+    assert!(
+        !task.in_run.swap(true, SeqCst),
+        "invariant violated: task run by two workers at once"
+    );
     *lock(&task.state) = TaskState::Running { notified: false };
     let waker = task.waker();
     let p = &shared.pipelines[task.pipeline];
@@ -270,6 +282,9 @@
     match result {
         Ok(Step::Blocked) => {
             let mut st = lock(&task.state);
+            // Cleared while holding the state lock, before the task becomes runnable again.
+            #[cfg(any(nata_loom, feature = "invariants"))]
+            task.in_run.store(false, SeqCst);
             let notified = matches!(*st, TaskState::Running { notified: true });
             if notified {
                 *st = TaskState::Queued;
@@ -409,7 +424,7 @@
         let workers = (0..threads.max(1))
             .map(|_| {
                 let s = Arc::clone(&shared);
-                std::thread::spawn(move || worker(s))
+                thread::spawn(move || worker(s))
             })
             .collect();
         for r in roots {
```

Check: `diff <(git show HEAD:spikes/operator-trait/src/api.rs) src/api.rs` and the same for batch.rs print nothing.
