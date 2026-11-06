# iceberg-commit (throwaway spike, verification-plan item 24)

Question: **is the Iceberg commit atomic and safe under concurrent writers, retries and crashes?** Full write-up, facts with
sources, model, bounds, mutants and the GCP results: `bench/results/20261104-iceberg-commit.md`.

| Path | What |
|---|---|
| `model/IcebergCommit.tla`, `MC.tla`, `cfg/*.cfg` | TLA+ model (TLC): 2-3 writers, append / delete-where / compaction, CAS commit (REST server or object-store conditional create), crashes, lost and duplicated requests, unknown outcomes, orphans, readers; invariants I1-I7, liveness, coverage canaries |
| `model/mutate_model.py` | 10 mutants of the model (8 required + 2 extra), each applied to a fresh copy, real spec byte-compared; CAUGHT / SURVIVED / INVALID |
| `model/run_model.py`, `tlc_runner.py` | runs the unmutated model: every configuration, the one that must fail (I4 under snapshot isolation), liveness, canaries |
| `model/lint_spec.py`, `test_model_tools.py` | static checks and tests of the harness (no TLC needed) |
| `src/`, `tests/` | Rust: writers (correct + 3 known-bad), fault injection, REST and object-store catalogs, the checker, a mock catalog for unit tests |
| `src/trace.rs`, `tests/trace.rs` | the trace instrumentation (`IC_TRACE=<dir>`, off by default) and its tests |
| `model/TraceSpec.tla`, `trace_to_tla.py`, `trace_check.py` | trace validation, the code gate: the model restricted to what the code did (below) |
| `model/mutate_trace_check.py`, `testdata/traces/` | mutation check of the trace checker: 20 corrupted traces and 23 mutants of the translator, `TraceSpec` and the runner |
| `scripts/` | pinned tool download (`tools.env`), reference catalog build and start, pyiceberg interop check |
| `gcp-check.sh <results-dir>` | everything above on a clean Debian VM; exit 0 only if every gating phase passed |

Nothing in this directory has been RUN on the dev box (rule: all runs on GCP). Checked locally: `cargo check --tests --bins`,
SANY parse of the spec and of all 10 mutants (`mutate_model.py --dry`, twice in a row), `lint_spec.py`.

Local quick checks that are allowed (no model checker, no tests, no catalog):

```bash
export JAVA=<a java>; export JAR=<tla2tools.jar>
cd spikes/iceberg-commit/model
python3 -I lint_spec.py
python3 -I mutate_model.py --dry --jar "$JAR" --java "$JAVA"      # edits apply once, every mutant parses
cd .. && ../../scripts/local-heavy.sh cargo check --tests --bins
```

GCP: see "How to run on GCP" in the results file (the job needs three lines added to `scripts/gcp/job.sh` and one to
`orchestrate.sh`, which this task was not allowed to edit).

## Modes of `gcp-check.sh` (`IC_MODE`) and the GCP jobs

| mode | job | what runs |
|---|---|---|
| `full` (default) | `iceberg` | everything: model, Apache REST fixture suite, object-store suite, interop |
| `model` | `icebergmodel` | only the TLA+ runs named in `IC_MODEL_ONLY` |
| `spikes` | `icebergspikes` | everything except the model; the pyiceberg interop is gating |
| `polaris` | `icebergpolaris` | the commit suite and the pyiceberg interop against **Apache Polaris 1.8.0**, a product catalog |

**Polaris** (`scripts/setup-polaris.sh`, `scripts/polaris.sh`): JDK 21 and the Polaris binary release are downloaded and checked against the checksums in `scripts/tools.env`
(the PGP signature was not checked), the server is started as a THROWAWAY on loopback with an in-memory metastore and local-disk storage, a catalog is created through its management
API, and the suite reaches it with OAuth client credentials from the environment (`IC_REST_CLIENT_ID`, `IC_REST_CLIENT_SECRET`, `IC_REST_SCOPE`, `IC_REST_WAREHOUSE`; read by `src/catalog.rs` and
`scripts/interop_check.py`). Polaris refuses local-disk storage and an in-memory metastore unless told otherwise (`ALLOW_INSECURE_STORAGE_TYPES`, `ignore-severe-issues`); those settings are
the reason this proves the commit path of the REST protocol against Polaris's implementation and says nothing about a production deployment of it (database metastore, object storage, authorization at
scale). Tests of the scripts: `scripts/gcp/test/test-icebergpolaris.sh` (against a fake server that checks the credential format, the token request and the order of the management calls).

## Trace validation (the code against the model)

Record: section 15 of `bench/results/20261104-iceberg-commit.md`. Object-store backend only. Needs a JDK and `tla2tools.jar` (`scripts/setup-tools.sh <dir>`).

```bash
cd spikes/iceberg-commit && cargo build --release
IC_TRACE=$PWD/../../target-traces ./target/release/ic-spike suite --backend store --work /path/to/work     # one trace per table
python3 -I model/trace_check.py --jar <tla2tools.jar> --java <java> --traces <the IC_TRACE dir> --out <results dir>
python3 -I model/mutate_trace_check.py --jar <tla2tools.jar> --java <java> --out <results dir> --model-mutants    # the checker is itself mutation-checked
```
