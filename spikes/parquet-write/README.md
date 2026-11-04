# parquet-write (throwaway spike, verification-plan item 30)

Question: **are the Parquet files we write correct?** Round trip with other readers, SOUND column statistics (a stored min is never above a value, a
stored max never below one, counts exact), page indexes, sort order, field ids, bloom filters, file sizing. The writer under test is parquet-rs 60.1.0
(the `parquet` crate), the independent readers/writers are DuckDB 2.0.0 and the verifier in this crate. pyarrow is not available here.

| Path | What |
|---|---|
| `src/write.rs` | the writer wrapper: row group and page sizes, statistics level and truncation, bloom filters, dictionary, compression, sorting (with `sorting_columns`), rolling files at a target size, and the all-NaN mitigation |
| `src/verify.rs` | the INDEPENDENT verifier: decodes the data and recomputes truth for every column chunk and every page with its own comparators (signed, unsigned, IEEE with NaN apart, decimal big-endian, unsigned bytes), then checks chunk statistics, page index (min, max, null pages, null counts, boundary order), `num_values`, sorting columns, field ids, UTF-8 validity of bounds, bloom filters (no false negatives) |
| `src/faulty.rs` | 11 known-bad statistic files (footer patched with wrong statistics) and 2 controls with legitimately loose or absent bounds |
| `src/data.rs` | adversarial tables: 31 columns (all integer widths signed and unsigned, f32/f64 with NaN, +-0.0, +-inf, subnormals, an all-NaN column, strings with multi-byte characters and long common prefixes, binary with 0xFF runs, decimals at precision 9, 18 and 38, dates, timestamps in three units, time, dictionary, list, struct), NULLs, constant and all-NULL columns, field ids on every field |
| `xcheck.py` | end-to-end through DuckDB: same rows as text, global sort order across files, and a predicate suite (about 1,200 predicates over 29 columns: values at, beyond and near min and max, truncated prefixes, NaN, infinities, zeros) where DuckDB's row-group pruning on OUR statistics must give the same count as a full scan |
| `check.sh`, `gcp-check.sh` | everything, on the dev box or a clean VM (job `parquetwrite`) |
| `tests/` | `good` (the writer passes the verifier under 10 option sets), `faults` (the verifier flags every bad file, passes the controls, flags a false `sorting_columns` and wrong or missing field ids) |

## Status (2026-11-04)

Run on the dev box (100,000 rows, 33 rolled files, 6 min 43 s), all GATING phases PASS: 3 cargo tests; for each of 10 writer configurations
(default, small row groups and pages, truncation at 8, chunk-level statistics only, bloom filters, no dictionary and no compression, no truncation,
sorted by `id` descending, sorted by `u8` then `id`, rolling files): independent verification, DuckDB reads exactly the same rows, and the pruning
suite finds 0 wrong counts. The sorted-descending files are in order across files. Rolling files at a 600 KB target came out at 629 to 636 KB (+5 to
6%), the last file the remainder. NOT yet run on a clean VM or at larger sizes (job `parquetwrite`, 500,000 rows).

## Findings

1. **parquet-rs 60.1.0 writes NaN as the minimum and maximum of a float chunk that holds only NaN.** The format forbids NaN in min and max. The verifier
   flags it as `stat-nan` (586 chunks in the 100,000-row test). DuckDB's pruning did NOT lose rows on those files (0 wrong counts in 135 predicates over
   the float columns), so for DuckDB 2.0.0 it is a spec violation without a wrong answer; other readers were not tested. Mitigation in the wrapper: the
   writer cuts the row groups itself, so it pre-scans and turns statistics off for float columns that have an all-NaN row group (a missing statistic is
   sound, a wrong one is not). DuckDB's own writer omits float statistics in the cases here, which is the same trade.
2. **The library cannot be talked into writing bounds narrower than the data**: `write_batch_with_statistics` only widens them. That is a safety
   property, and it is why the known-bad files are made by patching the footer.
3. A column holding both -0.0 and +0.0 gets +0.0 as its minimum where the format asks for -0.0 (warning `zero-sign`; harmless for readers that
   compare numerically, not for readers with a total order).
4. A binary column whose maximum is all 0xFF bytes cannot be truncated and incremented, so parquet-rs keeps the full value, longer than the truncation
   length (warning `stat-too-long`; sound).
5. **DuckDB as the writer passes the verifier** (0 errors in 4 settings), so the verifier is not over strict. DuckDB writes no page index (0 pages checked on
   its files) and omits float statistics here.
6. **The checks can fail**: all 11 known-bad statistic files are flagged by the verifier (loose max after truncation without increment, min too large,
   NaN max, off-by-one min and max, signed byte comparison, unsigned decimal comparison, signed u64 comparison, statistics on an all-NULL column, a bound
   that cuts a UTF-8 character, a wrong null count), the 2 controls pass, a false `sorting_columns` and wrong or missing field ids are flagged, and
   DuckDB's own pruning returns wrong counts on 5 of the 11 bad files, so the end-to-end suite is shown to fail too.
7. DuckDB 2.0.0 cannot read Timestamp(ms) values beyond about year 9999 (the conversion to microseconds overflows); the generator keeps within that.

## Not covered

Page-level statistics cannot be made wrong with this library (no known-bad page index files); the page-level checks run on every good file and on
DuckDB's none. Bloom filters are checked for false negatives on integer and byte columns only, and only parquet-rs's own filters (DuckDB's pruning on them is
not separated from statistics). No second independent decoder of the statistics (pyarrow, parquet-java) was available: the verifier is independent of
parquet-rs's statistics code but shares its Thrift decoding. Iceberg's own metrics (bounds truncated to 16 characters with an incremented upper bound,
NaN counts, value counts) are not the Parquet statistics and are item 25/26's concern: the engine should compute them from the Arrow data it writes
and not read them back from the footer. Nested columns are verified for min/max soundness only (no null counts). Sorting is a full in-memory sort, not
an external one. Compression codecs other than snappy and none, encryption and Parquet versions other than the crate's default are not tested.
