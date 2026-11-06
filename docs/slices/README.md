# Slice verification cards

One card per development point, written before the implementation and accepted by the owner. The rule and the five parts of a card
are in `docs/way-of-working.md`, section 2 ("The slice verification card"). Copy `0000-template.md` to `NNNN-name.md`.

| Card | Slice | Status | ADRs |
|---|---|---|---|
| [0001](0001-parquet-scan.md) | Parquet scan with projection pushdown and row-group pruning (local files); slice 3 of the feature map | closed 2026-11-06 (accepted 2026-11-05) | 0001, 0002, 0003 |
| [0002](0002-select-where-limit.md) | Parser and binder for `SELECT ... FROM <parquet> WHERE ... LIMIT`, CLI results, `EXPLAIN`; slice 4 of the feature map | closed 2026-11-07 with measured deviations ([closeout](../../bench/results/20261106-slice4-close.md)) | 0001, 0002, 0003, 0006; creates 0007 |
| [0003](0003-pgwire-minimal.md) | A minimal Postgres-wire server for slice 4's queries ("4b") and real-client compatibility (plan item 35) | draft 2026-11-06; part 5 selected 2026-11-07, acceptance separate | 0001, 0002, 0007; creates 0008 |

Order decided by the owner on 2026-11-05: slice 3 (card 0001), then slice 4 (parser and binder). The engineering checks of verification-plan item 34 (`cargo-deny`, `audit`, `geiger`, sanitizers, Miri) are introduced on the card of slice 4, when real code exists to run them on.
