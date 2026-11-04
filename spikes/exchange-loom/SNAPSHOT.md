# Snapshot of the operator-trait spike (loom model of the exchange)

The sources here are copies of the COMMITTED files, not the working tree:

    git show HEAD:spikes/operator-trait/src/<file>      # HEAD = ce5f7153c992a2a11f7e541ec0d5c9683461ab00

`spikes/operator-trait/` and `spikes/async-bridge/` are not touched. Own `[workspace]`, own `target/`. The
`--cfg nata_loom` convention is the one of `spikes/async-bridge` (a `--cfg loom` build would also switch on
other crates' loom code).

sha256 of the committed originals (before any edit here):

    b84b2c713421f828552196e01900f48a20d43a73cd2b9f7fcde22869a4d92b95  api.rs
    328f849057456701ba3064bdcef1ce5df0d438205af7f5ab9633afb45a701733  batch.rs
    f128ba1cfe77cececbfdfb35cbc8b983f03813616713bb550cb0898ec3147677  sched.rs
    ac34ee49004e8a8ecc6e24a621c6b834575c0a0285ada3bfb6ff48e4958e84a1  sync.rs
    1f4bc458d4d244541c19f94dc046655abd9fe374e52065d8b21bc51a32bda712  ops.rs

sha256 of the copies in `src/` at the time of the results file:

    b84b2c713421f828552196e01900f48a20d43a73cd2b9f7fcde22869a4d92b95  api.rs        (identical)
    328f849057456701ba3064bdcef1ce5df0d438205af7f5ab9633afb45a701733  batch.rs      (identical)
    (see `sha256sum src/*.rs` for sched.rs, sync.rs, exchange.rs, limit.rs, lib.rs)

## What differs

1. `api.rs`, `batch.rs`: identical (`diff <(git show HEAD:spikes/operator-trait/src/api.rs) src/api.rs` prints nothing).
2. `sync.rs` and `sched.rs`: ONLY the cfg name `loom` -> `nata_loom` (and the comments that name it):

```diff
# diff <(git show HEAD:spikes/operator-trait/src/sync.rs) src/sync.rs
3c3
< //! Normal builds re-export std. Under `RUSTFLAGS="--cfg loom"` the same names are loom's
---
> //! Normal builds re-export std. Under `RUSTFLAGS="--cfg nata_loom"` the same names are loom's
7c7,9,11 (3 lines)   #[cfg(not(loom))]  ->  #[cfg(not(nata_loom))]
14c14,16,18 (3 lines) #[cfg(loom)]      ->  #[cfg(nata_loom)]

# diff <(git show HEAD:spikes/operator-trait/src/sched.rs) src/sched.rs
13c13   (comment)   `--cfg loom`  ->  `--cfg nata_loom`
288c288 #[cfg(loom)]         ->  #[cfg(nata_loom)]      (assertion in run_task: started only from Queued)
360c360 #[cfg(not(loom))]    ->  #[cfg(not(nata_loom))] (test-delay sleep)
490c490 #[cfg(loom)]         ->  #[cfg(nata_loom)]      (assertion in wait: queue empty, every task Done)
```

   Behaviour of the scheduler is unchanged. The five `bug-*` features of the original stay (all off).
3. `ops.rs` is NOT compiled as a whole (it uses std primitives and file I/O). Two verbatim sections of it are:
   - `src/exchange.rs` = ops.rs lines 237-390 (`ChanState`, `Channel`, `wake_all`, `exchange()`, `ExchangeSink`,
     `ExchangeSource` and their trait impls), under a new header. Differences: the imports, and ONE FIX (finding F1, added after the first pass):
     in `close_consumers`, `let stale = take(consumer_wakers)` before `drop(s)` and `drop(stale)` after `wake_all`
     (marked `FIX for finding F1`). Imports:
     (`Mutex`, `AtomicUsize`, `Ordering::SeqCst` now from `crate::sync`; `Arc` stays `std::sync::Arc` because
     `Arc<dyn GlobalSink>` / `Arc<dyn SourceFactory>` cannot be loom `Arc`s: loom's `Arc` has no unsizing coercion;
     the scheduler already keeps those on std's `Arc`).
   - `src/limit.rs` = ops.rs lines 141-184 (`Limit` / `LimitOp`), imports only (`Arc`, `AtomicUsize` from `crate::sync`).
   - ADDED at the end of `exchange.rs`, clearly marked, nothing in the original code path:
     `Channel::queue_len()`, `capacity()`, `stored_wakers()` (read-only probes for the bound check and for
     the "no waker left after cancel" check; see the results file, finding F1).
   Check: `diff <(git show HEAD:spikes/operator-trait/src/ops.rs | sed -n 237,390p) <(sed -n '/^struct ChanState/,/^\/\/ -* ADDED/p' src/exchange.rs | sed '$d')`
   now prints only the marked FIX block in `close_consumers` (plus the blank/separator line before the ADDED marker).
   sha256 of the fixed `exchange.rs`: 3c7f07a7a79f065be636847514bd5f046d792031237dd93ba13a1d026b04cf73 (pre-fix copy: 52afc229d00baad5bbf8a335c0e8661dbf90ffdb1ff87a7db878e0967c5f2e85).
4. `lib.rs` is new (module list). `Cargo.toml` is new (loom only under `cfg(nata_loom)`).
