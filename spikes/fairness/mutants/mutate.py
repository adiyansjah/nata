#!/usr/bin/env python3
"""Apply one named mutation of the NEW (fairness) code to a COPY of the crate, never the real sources.
usage: mutate.py <name> <crate-dir> <dest-dir>   -- prints the unified diff, exits 1 if the anchor is not found."""
import difflib, os, shutil, sys

YIELD_ARM = """            *st = TaskState::Queued;
            drop(st);
            shared.yields.fetch_add(1, SeqCst);
            shared.enqueue(task); // back of the queue
"""

M = {
 # the yielding task is dropped: never run again
 "yield_no_requeue": ("src/sched.rs", "            shared.enqueue(task); // back of the queue\n",
  "            let _ = task; // MUTATION: yield without re-queue\n"),
 # yield leaves the task Parked (and queued): a Parked task is started / woken a second time
 "yield_parks": ("src/sched.rs", YIELD_ARM,
  YIELD_ARM.replace("*st = TaskState::Queued;", "*st = TaskState::Parked; // MUTATION: yield re-queues a Parked task")),
 # yield handled like Blocked: only re-queued when a wake had arrived
 "yield_as_blocked": ("src/sched.rs", YIELD_ARM,
  """            let notified = matches!(*st, TaskState::Running { notified: true });
            // MUTATION: yield goes through the Blocked logic (re-queue only if notified)
            if notified {
                *st = TaskState::Queued;
                drop(st);
                shared.yields.fetch_add(1, SeqCst);
                shared.enqueue(task);
            } else {
                *st = TaskState::Parked;
            }
"""),
 # the task is put on the queue while its state is still Running; the state is updated afterwards
 "yield_enqueue_before_state": ("src/sched.rs", """            // A wake that arrived while Running (`notified`) is subsumed: the task is runnable
            // anyway. Not `Parked`: nobody would wake a task that is not waiting for anything.
            *st = TaskState::Queued;
            drop(st);
            shared.yields.fetch_add(1, SeqCst);
            shared.enqueue(task); // back of the queue
""", """            // MUTATION: enqueue while still Running, set Queued afterwards
            drop(st);
            shared.yields.fetch_add(1, SeqCst);
            shared.enqueue(Arc::clone(&task));
            *lock(&task.state) = TaskState::Queued;
"""),
 # the slice budget is never fresh: every slice starts exhausted, so a task yields before doing any work
 "yield_budget_not_reset": ("src/sched.rs", "        let mut morsels = 0usize;\n        let mut started = slice_clock(&slice);",
  "        let mut morsels = slice.max_morsels; // MUTATION: the budget starts used up\n        let mut started = slice_clock(&slice);"),
 "yield_front_of_queue": ("src/sched.rs", "            shared.enqueue(task); // back of the queue\n",
  "            lock(&shared.queue).push_front(task); // MUTATION: front, not back\n            shared.queue_cv.notify_one();\n"),
 # inverted emptiness check: yield only when nobody is waiting
 "yield_check_inverted": ("src/sched.rs", "                        if !shared.queue_is_empty() {\n                            return Ok(Step::Yield);",
  "                        if shared.queue_is_empty() { // MUTATION: inverted\n                            return Ok(Step::Yield);"),
 # the budget is never checked: no slicing at all
 "yield_never": ("src/sched.rs", "                    if morsels >= 1 && slice.exhausted(morsels, started) {",
  "                    if false && morsels >= 1 && slice.exhausted(morsels, started) { // MUTATION: never yield"),
 # --- spill files (ops.rs): the open-file growth bug. One writer per partition is kept open per task until combine.
 "spill_keep_open": "OPS",

}


OPS = "src/ops.rs"
M["spill_keep_open"] = [
 (OPS, "use std::io::{BufReader, Read, Write};", "use std::io::{BufReader, BufWriter, Read, Write};"),
 (OPS, "    spilled: bool,\n}", "    spilled: bool,\n    writers: HashMap<usize, (BufWriter<File>, FdGuard)>, // MUTATION: writers kept open\n}"),
 (OPS, "            map: HashMap::new(),\n            spilled: false,\n", "            map: HashMap::new(),\n            spilled: false,\n            writers: HashMap::new(),\n"),
 (OPS, """            {
                // Counted before the open and released after the close (it never undercounts).
                let _open = self.g.fds.enter();
                let mut f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(io)?;
                f.write_all(&buf).map_err(io)?;
            } // closed here
""", """            if !self.writers.contains_key(&p) {
                let guard = self.g.fds.enter();
                let f = OpenOptions::new().create(true).append(true).open(&path).map_err(io)?;
                self.writers.insert(p, (BufWriter::new(f), guard));
            }
            self.writers.get_mut(&p).expect("just inserted").0.write_all(&buf).map_err(io)?;
"""),
 (OPS, "            self.spill()?;\n        } else {", """            self.spill()?;
            for (w, _) in self.writers.values_mut() {
                w.flush().map_err(|e| QueryError(format!("spill failed: {e}")))?;
            }
        } else {"""),
]

name, src, dst = sys.argv[1:4]
edits = M[name] if isinstance(M[name], list) else [M[name]]
rel = edits[0][0]
assert all(e[0] == rel for e in edits)

if os.path.exists(dst):
    shutil.rmtree(dst)
os.makedirs(dst)
for item in ("src", "tests", "examples", "Cargo.toml", "Cargo.lock"):
    s = os.path.join(src, item)
    if os.path.isdir(s):
        shutil.copytree(s, os.path.join(dst, item))
    elif os.path.exists(s):
        shutil.copy(s, dst)
p = os.path.join(dst, rel)
text = open(p).read()
new_text = text
for _, old, new in edits:
    if new_text.count(old) != 1:
        print(f"anchor for {name} found {new_text.count(old)} times", file=sys.stderr)
        sys.exit(1)
    new_text = new_text.replace(old, new)
open(p, "w").write(new_text)
sys.stdout.writelines(difflib.unified_diff(text.splitlines(True), new_text.splitlines(True), "a/" + rel, "b/" + rel, n=1))
