# iceberg-commit (throwaway spike, verification-plan item 24)

Question: **is the Iceberg commit atomic and safe under concurrent writers, retries and crashes?** Full write-up, facts with
sources, model, bounds, mutants and the GCP results: `bench/results/20261104-iceberg-commit.md`.

| Path | What |
|---|---|
| `model/IcebergCommit.tla`, `MC.tla`, `cfg/*.cfg` | TLA+ model (TLC): 2-3 writers, append / delete-where / compaction, CAS commit (REST server or object-store conditional create), crashes, lost and duplicated requests, unknown outcomes, orphans, readers; invariants I1-I7, liveness, coverage canaries |
| `model/mutate_model.py` | 9 mutants of the model (7 required + 2 extra), each applied to a fresh copy, real spec byte-compared; CAUGHT / SURVIVED / INVALID |
| `model/run_model.py`, `tlc_runner.py` | runs the unmutated model: every configuration, the one that must fail (I4 under snapshot isolation), liveness, canaries |
| `model/lint_spec.py`, `test_model_tools.py` | static checks and tests of the harness (no TLC needed) |
| `src/`, `tests/` | Rust: writers (correct + 3 known-bad), fault injection, REST and object-store catalogs, the checker, a mock catalog for unit tests |
| `scripts/` | pinned tool download (`tools.env`), reference catalog build and start, pyiceberg interop check |
| `gcp-check.sh <results-dir>` | everything above on a clean Debian VM; exit 0 only if every gating phase passed |

Nothing in this directory has been RUN on the dev box (rule: all runs on GCP). Checked locally: `cargo check --tests --bins`,
SANY parse of the spec and of all 9 mutants (`mutate_model.py --dry`, twice in a row), `lint_spec.py`.

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
