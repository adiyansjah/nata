# HC admission regression evidence

Development-VM evidence, not clean-VM timing certification. No GCP commands
were run. See the final section of `../../20261107-hash-aggregate-perf.md` for
the first bad commit, interpretation, compatibility controls and exact settled
shipment/orchestrator commands.

- `historical-revisions.json`: exact archive builds with explicit release
  workspace-package cleans; native stderr includes admitted scan tasks and all
  spill counters. Old revisions predate package 0 and do not emit stats JSON.
- `measurements.json`: alternating `2b7a415` / `02d3fae` runs, with no compiler
  from this task running; load averages, binary SHA256s, counters and full
  package 0 records. Every HC invocation enables CSV and uses SF1, the requested
  thread count, 67,108,864-byte aggregate B, a private spill directory and an
  explicit bounded 4 GiB disk quota, as in the original heavy harness.
- `*.stats.json`: verbatim native `NATA_STATS_JSON` sidecars for good/bad
  controls. Stats remain explicitly incomplete for inventory/CPU scopes; no
  complete allocation-inventory claim is made.
- `controls.json`: SF10 Q1 (1/8 requested threads, 64 MiB), matched against
  DuckDB with the existing tolerance, and SF1 HC (one thread, 4 GiB, no spill).
  Their elapsed fields are labelled as overlapping a build and are not timing
  evidence. HC output count is 1,273,748; Q1 uses exact multiset comparison
  except the already-established AVG tolerance.
- `byte-control.json`: one-thread SF1 HC at 64 MiB with stats off/on: identical
  CSV bytes/hash, peak grant, spill bytes, partitions and depth.
- `cloud-evidence.json`: only the existing W-C baseline primary/twin records
  and their embedded stats, on both CPUs; no infrastructure identifiers.
- Build, Rust, clippy, four CLI gate and inert harness logs; focused mutation
  summary/outcomes and the original-production-policy negative/recovery logs.
- `shipment-manifest.json` and `settled-plan.json`: verified immutable archive
  identities/source contents and the 240-arm paired plan. Generating this plan
  starts no engine or cloud process.

Every local Cargo build/test was serial and used four build jobs. Focused
mutation command (`--in-place` itself runs serially):

```sh
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/tmp/nata-spill-regression/target \
  /home/dev/perf2b/cargo-mutants mutants --in-place --no-config \
  --profile release --jobserver-tasks 4 -C=-j4 -p nata-exec \
  -f crates/nata-exec/src/aggregate.rs \
  -f crates/nata-exec/src/aggregate/pipeline.rs \
  -F 'scan_concurrency|Table::chunk_size' \
  --output /tmp/nata-spill-regression/mutation --timeout 180 \
  -- --lib scan_admission_leaves_table_chunk_headroom
```

The exact pre-fix expression was temporarily restored at the production call
site, then this test was run before and after restoring the fixed source:

```sh
cargo test --release --locked -j 4 -p nata-exec --test aggregate_spill \
  scan_admission_keeps_table_headroom_with_small_footer -- --exact
```

The negative fails with three admitted tasks versus the expected two; the
restored source passes. No negative mutation remains in the committed tree.
