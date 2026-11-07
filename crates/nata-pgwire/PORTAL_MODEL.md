# Suspended portal model

`tests/portal_loom.rs` includes `src/server/lifecycle.rs` directly. The same file
is compiled into the production server. Under `nata_portal_loom` only its Mutex and
Condvar are replaced with loom primitives (loom 0.7.2 is already locked).

Production callers:

- `fetch::Gate` uses `row` and `Credit` for positive-limit/exact-end suspension,
  unlimited Execute and retained-producer resume. Credit is one outstanding
  Execute, not an unbounded queue; `Execution::drop` closes it persistently.
- `Session` uses `Slot<Active>` for cancel selection, installation, removal on
  suspension and statement finish. The slot lock covers cancellation itself, so
  suspension cannot publish while an already selected cancel is still pending.
  `Deadline::start` clones the selected target before spawning its timer.
- Expiry and `Execution::drop` share `Once<join>` for cleanup ownership. The join
  handle cannot be copied in safe Rust; a second take returns None.
- `fetch::Run` uses `Phase<Instant>` for bound, running, suspended, complete and
  expired state. Run branches on the core's `Enter` decision, including terminal
  Execute handling; no outer guard can mask its mutant. The event loop uses `Exchange<Instant>` for the first-message
  deadline (its begin guard is the sole owner), timeout selection, error drain and Sync (which clears portals only
  outside a compatibility transaction). Models use logical ticks for the clock.

The seven models cover a four-row stream in two exact-boundary fetches, inert
suspended cancel, unlimited credit, resume/expiry/cleanup races, wake before/after
park, cancel versus suspension publication, replacement-token isolation, timeout
through suspension, ignore-until-Sync and transaction versus idle Sync cleanup.
Every model executes a reachable assertion, not just a no-panic check.

This is a bounded model of the shared transition seam. It does not instrument
Tokio, sockets, the scan cancel implementation, engine admissions/reservations or
wall-clock delivery. Native protocol/reliability tests cover that wiring; loaded
resource and closing-commit bound-3 runs remain separate acceptance gates. There
is one producer, two concurrent boundary actors and at most two positive fetches,
not an unbounded proof over all portals/sessions.

Run serially from the repository root:

```sh
LOOM_MAX_PREEMPTIONS=2 bash crates/nata-pgwire/run_portal_loom.sh
# Heavy VM only:
LOOM_MAX_PREEMPTIONS=3 bash crates/nata-pgwire/run_portal_loom.sh
```

The script requires all seven models, reports explored executions and fails on
build error, assertion, loom deadlock, timeout or a missing model. Keep a separate
target for private-copy mutants. The focused controls registered in `mutate.py`
remove resume notification, lose sticky close, remove idempotent cleanup,
retain/target stale tokens, cancel after suspension publication, preserve a
suspended cancel target, restart timeout, lose error drain or fail to reset Sync.
The double-release control makes a second cleanup try to own an already taken
join (a panic); it cannot duplicate a non-Clone join handle in safe Rust. Compile
errors/timeouts are INVALID, never catches. Full sweep is a heavy step.

The additional native `portal-deadline-follows-replacement` mutant moves target
selection inside the Tokio timer, after its sleep. The existing real-Active
unit test must fail it; this separately checks the asynchronous timer wiring
outside the model. `nata_portal_loom` is deliberately different from the
scheduler's cfg: instrumenting the entire scheduler would change its public Arc
API for downstream Parquet and prevent this server package from compiling.
