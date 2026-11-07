# Slice verification cards

One card per development point, written before the implementation and accepted by the owner. The rule and the five parts of a card
are in `docs/way-of-working.md`, section 2 ("The slice verification card"). Copy `0000-template.md` to `NNNN-name.md`.

| Card | Slice | Status | ADRs |
|---|---|---|---|
| [0001](0001-parquet-scan.md) | Parquet scan with projection pushdown and row-group pruning (local files); slice 3 of the feature map | closed 2026-11-06 (accepted 2026-11-05) | 0001, 0002, 0003 |
| [0002](0002-select-where-limit.md) | Parser and binder for `SELECT ... FROM <parquet> WHERE ... LIMIT`, CLI results, `EXPLAIN`; slice 4 of the feature map | closed 2026-11-07 with measured deviations ([closeout](../../bench/results/20261106-slice4-close.md)) | 0001, 0002, 0003, 0006; creates 0007 |
| [0003](0003-pgwire-minimal.md) | A minimal Postgres-wire server for slice 4's queries ("4b") and real-client compatibility (plan item 35) | closed with deviations 2026-11-07; [results](../../bench/results/20261107-pgwire-close.md) | 0001, 0002, 0007; creates 0008 |
| [0004](0004-deep-check-closeout.md) | Deep-check closeout of slices 4 and 4b: the first nightly run's findings (a damaged-file hang, a double-negation overflow, a BETWEEN evaluation, loom m21 time box) and R15 TSan | closed 2026-11-07 (nightly 31/31 on `84e9c83`) | 0003, 0006, 0007, 0008 |
| [0005](0005-hash-aggregate.md) | Hash aggregate with spill; GROUP BY, HAVING, common aggregates; slice 5 of the feature map | accepted 2026-11-07 | 0001–0004, 0006–0008; creates 0009, 0010 (accepted 2026-11-07) |
| [0006](0006-object-store-io.md) | Object-store reads, async I/O and metadata cache; feature-map slice 11, Track B (owner parallel-track decision 2026-11-07) | draft | 0001–0003, 0006–0009; proposes 0011, 0012 |

Order decided by the owner on 2026-11-05: slice 3 (card 0001), then slice 4 (parser and binder). The engineering checks of verification-plan item 34 (`cargo-deny`, `audit`, `geiger`, sanitizers, Miri) are introduced on the card of slice 4, when real code exists to run them on.
