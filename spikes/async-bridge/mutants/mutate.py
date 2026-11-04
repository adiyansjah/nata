#!/usr/bin/env python3
"""Apply one named mutation to a COPY of the crate (never the real sources).
usage: mutate.py <name> <crate-dir> <dest-dir>   -- prints the unified diff, exits 1 if the anchor is not found."""
import difflib, os, shutil, sys

M = {
 # --- channel (chan.rs)
 "chan_no_recheck": ("src/chan.rs",
  """        s.rx_waker = Some(waker.clone());
        drop(s);
        // Window""",
  """        drop(s);
        // MUTATION: the emptiness check and the waker store are no longer one critical section
        // (equivalent to storing the waker without re-checking afterwards).
        hook();
        let mut s = lock(&self.inner.state);
        s.rx_waker = Some(waker.clone());
        drop(s);
        // Window"""),
 "chan_close_no_wake": ("src/chan.rs",
  """        hook();
        if let Some(w) = waker {
            w.wake();
        }
    }
}

impl<T> Sender<T> {""",
  """        hook();
        let _ = waker; // MUTATION: the last sender's drop takes the waker but never fires it
    }
}

impl<T> Sender<T> {"""),
 "chan_send_no_wake": ("src/chan.rs",
  """            hook();
            if let Some(w) = waker {
                w.wake();
            }
            Poll::Ready(Ok(()))""",
  """            hook();
            let _ = waker; // MUTATION: a send never fires the consumer's waker
            Poll::Ready(Ok(()))"""),
 "chan_close_rx_no_wake": ("src/chan.rs",
  """        drop(items);
        hook();
        senders.into_iter().for_each(std::task::Waker::wake);""",
  """        drop(items);
        hook();
        let _ = senders; // MUTATION: closing the receiver does not wake blocked senders"""),
 "chan_cap_off_by_one": ("src/chan.rs",
  "if s.queue.len() < this.inner.cap {",
  "if s.queue.len() <= this.inner.cap { // MUTATION: capacity + 1"),
 # --- scheduler (sched.rs, the snapshot)
 "sched_ignore_notified": ("src/sched.rs",
  "let notified = matches!(*st, TaskState::Running { notified: true });",
  "let notified = false; // MUTATION: a wake that arrived while Running is forgotten"),
 "sched_wake_queued_enqueues": ("src/sched.rs",
  "            TaskState::Queued | TaskState::Done => {}",
  """            TaskState::Queued => {
                // MUTATION: a wake of an already queued task enqueues it a second time
                drop(st);
                self.shared.enqueue(Arc::clone(self));
            }
            TaskState::Done => {}"""),
 "sched_shutdown_without_lock": ("src/sched.rs",
  """        {
            let _queue = lock(&self.shared.queue);
            self.shared.shutdown.store(true, SeqCst);
        }""",
  """        // MUTATION: the lost-shutdown bug of the first prototype (flag set without the queue lock)
        self.shared.shutdown.store(true, SeqCst);"""),
}

name, src, dst = sys.argv[1:4]
rel, old, new = M[name]
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
if text.count(old) != 1:
    print(f"anchor for {name} found {text.count(old)} times", file=sys.stderr)
    sys.exit(1)
open(p, "w").write(text.replace(old, new))
sys.stdout.writelines(difflib.unified_diff(text.splitlines(True), text.replace(old, new).splitlines(True), "a/" + rel, "b/" + rel, n=1))
