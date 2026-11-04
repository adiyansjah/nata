#!/usr/bin/env python3
"""Apply one named mutation to a COPY of the crate (never the real sources).
usage: mutate.py <name> <crate-dir> <dest-dir>
Copies src/, tests/, Cargo.toml (+ Cargo.lock) to <dest-dir>, applies the mutation, prints the
unified diff. Exits 1 if the anchor is not found exactly once (so a mutation can never silently
become a no-op), 2 for an unknown name. `mutate.py --list` prints the names."""
import difflib, os, shutil, sys

NO_RECHECK = "MUTATION: the emptiness/fullness check and the waker store are no longer one critical section"

M = {
 # ---- wake-up protocol of the exchange (src/exchange.rs)
 "pop_store_waker_without_recheck": ("src/exchange.rs",
  """        s.consumer_wakers.push(waker.clone());
        SourceResult::Blocked""",
  """        drop(s); // %s
        let mut s = lock(&self.state);
        s.consumer_wakers.push(waker.clone());
        SourceResult::Blocked""" % NO_RECHECK),
 "push_store_waker_without_recheck": ("src/exchange.rs",
  """            s.producer_wakers.push(waker.clone());
            return SinkResult::Blocked;""",
  """            drop(s); // %s
            let mut s = lock(&self.state);
            s.producer_wakers.push(waker.clone());
            return SinkResult::Blocked;""" % NO_RECHECK),
 "pop_blocked_without_storing_waker": ("src/exchange.rs",
  "        s.consumer_wakers.push(waker.clone());\n        SourceResult::Blocked",
  "        let _ = waker; // MUTATION: Blocked without storing the waker\n        SourceResult::Blocked"),
 "push_blocked_without_storing_waker": ("src/exchange.rs",
  "            s.producer_wakers.push(waker.clone());\n            return SinkResult::Blocked;",
  "            let _ = waker; // MUTATION: Blocked without storing the waker\n            return SinkResult::Blocked;"),
 "push_does_not_wake_consumer": ("src/exchange.rs",
  """        let to_wake = std::mem::take(&mut s.consumer_wakers);
        drop(s);
        wake_all(to_wake);
        SinkResult::NeedMoreInput""",
  """        let to_wake = std::mem::take(&mut s.consumer_wakers);
        drop(s);
        let _ = to_wake; // MUTATION: a send never fires the consumer's waker
        SinkResult::NeedMoreInput"""),
 "pop_does_not_wake_producer": ("src/exchange.rs",
  """            let to_wake = std::mem::take(&mut s.producer_wakers);
            drop(s);
            wake_all(to_wake);
            return SourceResult::Batch(b);""",
  """            let to_wake = std::mem::take(&mut s.producer_wakers);
            drop(s);
            let _ = to_wake; // MUTATION: a receive never fires the producers' wakers
            return SourceResult::Batch(b);"""),
 "producer_done_does_not_wake_consumer": ("src/exchange.rs",
  """        drop(s);
        wake_all(to_wake);
    }

    fn close_consumers""",
  """        drop(s);
        let _ = to_wake; // MUTATION: the last producer finishing does not wake the consumers
    }

    fn close_consumers"""),
 "close_consumers_does_not_wake_producers": ("src/exchange.rs",
  """        drop(s);
        wake_all(to_wake);
        drop(stale);""",
  """        drop(s);
        let _ = to_wake; // MUTATION: the consumer's early close does not wake blocked producers
        drop(stale);"""),
 "waker_kept_after_close": ("src/exchange.rs",
  "        let stale = std::mem::take(&mut s.consumer_wakers);",
  "        let stale: Vec<Waker> = Vec::new(); // MUTATION: the F1 fix is removed; stale consumer wakers stay in the channel"),
 "wake_all_wakes_only_the_first": ("src/exchange.rs",
  "    wakers.iter().for_each(Waker::wake);",
  "    wakers.iter().take(1).for_each(Waker::wake); // MUTATION: only the first waiter is woken"),
 "consumer_finish_never_closes": ("src/exchange.rs",
  "if self.consumers.fetch_sub(1, SeqCst) == 1 {",
  "if self.consumers.fetch_sub(1, SeqCst) == 99 { // MUTATION: the last consumer never closes the channel"),
 "push_ignores_closed_channel": ("src/exchange.rs",
  "        if s.consumers_closed {\n            return SinkResult::Finished;",
  "        if false && s.consumers_closed { // MUTATION: a closed channel is not noticed by producers\n            return SinkResult::Finished;"),
 # ---- bound and data integrity
 "capacity_off_by_one": ("src/exchange.rs",
  "if s.queue.len() >= self.cap {",
  "if s.queue.len() > self.cap { // MUTATION: capacity + 1"),
 "pop_checks_finished_before_queue": ("src/exchange.rs",
  """        if let Some(b) = s.queue.pop_front() {
            let to_wake = std::mem::take(&mut s.producer_wakers);
            drop(s);
            wake_all(to_wake);
            return SourceResult::Batch(b);
        }
        if s.producers == 0 {
            return SourceResult::Finished;
        }""",
  """        if s.producers == 0 { // MUTATION: end of stream is checked before the queue is drained
            return SourceResult::Finished;
        }
        if let Some(b) = s.queue.pop_front() {
            let to_wake = std::mem::take(&mut s.producer_wakers);
            drop(s);
            wake_all(to_wake);
            return SourceResult::Batch(b);
        }"""),
 "producer_done_drops_queued_items": ("src/exchange.rs",
  "        s.producers -= 1;\n",
  "        s.producers -= 1;\n        if s.producers == 0 { s.queue.clear(); } // MUTATION: items still queued are dropped when the last producer is done\n"),
 "push_to_front_reorders": ("src/exchange.rs",
  "s.queue.push_back(std::mem::take(batch));",
  "s.queue.push_front(std::mem::take(batch)); // MUTATION: newest first"),
 # ---- scheduler side of the early finish across the exchange (src/sched.rs)
 "sched_task_end_skips_source_finish": ("src/sched.rs",
  "        body.source.finish();\n",
  "        // MUTATION: finish() of the source is never called, so a consumer never closes the channel\n"),
}


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "--list":
        print("\n".join(M))
        return 0
    name, crate, dest = sys.argv[1:4]
    if name not in M:
        print("unknown mutation", name, file=sys.stderr)
        return 2
    if os.path.exists(dest):
        shutil.rmtree(dest)
    os.makedirs(dest)
    for item in ("src", "tests", "Cargo.toml", "Cargo.lock"):
        s = os.path.join(crate, item)
        if os.path.isdir(s):
            shutil.copytree(s, os.path.join(dest, item))
        elif os.path.exists(s):
            shutil.copy(s, os.path.join(dest, item))
    f, old, new = M[name]
    path = os.path.join(dest, f)
    text = open(path).read()
    if text.count(old) != 1:
        print(f"{name}: anchor found {text.count(old)} times in {f} (need exactly 1)", file=sys.stderr)
        return 1
    mutated = text.replace(old, new)
    open(path, "w").write(mutated)
    sys.stdout.writelines(
        difflib.unified_diff(text.splitlines(True), mutated.splitlines(True), f"a/{f}", f"b/{f}", n=1)
    )
    return 0


sys.exit(main())
