# Decoding Iceberg manifests: generic Avro reader, iceberg-rust, or our own (2026-11-05)

ADR 0005 rule 7 left "reuse `iceberg-rust` or write our own manifest decoder" undecided because there was no bake-off. This is the bake-off. Code: `spikes/manifest-decode/` (committed together with
this file). **Timings are from a development machine that was NOT quiet (8 vCPU, load average 4 to 5 during the final run) and are labelled SANITY ONLY by the settle
rule of ADR 0006; the ratios are 15 to 45 times, so the conclusion does not depend on that, but no absolute number here is final. Memory numbers do not depend on load.**

## What was compared

A synthetic table from `icx synth`: 1,000,000 data files and 10,000 position deletes in 200 manifests of about 10,000 entries, 263 MiB uncompressed, each entry with its path
(about 45 bytes), a two-field partition (date, int) and counts and bounds for 8 columns. Three decoders, each producing everything a scan planner needs (status, sequence numbers, content,
path, partition, record count, size, value, null and NaN counts, lower and upper bounds, equality ids, referenced data file):
- **generic**: `apache-avro` generic values, then typed entries (the spike every ADR 0005 planning number used);
- **iceberg-rust 0.11**: `Manifest::parse_avro`. It does more than the other two (typed `Datum` bounds, schema and partition spec parsed), so it is not an apples-to-apples decoder;
- **hand**: a schema-compiled decoder (`src/hand.rs`, about 600 lines): the writer schema in the file header is compiled once into a keep-or-skip program, records are decoded in place into
  arenas, nothing else is built.

## Speed and memory (1,010,000 entries; one process per arm; "retained" = memory held by the decoded result)

| decoder | 1 thread | retained memory | 8 threads (one manifest per task) | retained memory |
|---|---:|---:|---:|---:|
| generic | 12.18 s | 2,444 MiB | 4.62 s | 3,271 MiB |
| iceberg-rust | 27.67 s | 4,700 MiB | 11.37 s | 4,473 MiB |
| **hand** | **0.67 s** | **1,168 MiB** | **0.30 s** | **1,025 MiB** |
| hand, statistics of 2 of the 8 columns only | 0.59 s | 464 MiB | - | - |

The hand decoder is **18 times faster than the generic reader and 41 times faster than iceberg-rust on one thread** (16 and 38 times on eight), uses **2.1 and 4.0 times less memory**, and with a column
filter 5.3 times less memory than the generic reader. 1.5 million entries per second per thread is 415 MB/s of manifest bytes. The two other decoders scale 2.4 to 2.6 times on eight threads, the hand decoder
2.2 times (it is already close to memory bandwidth).

By codec (202,000 entries, one thread, median of 3; the same table re-written with each Avro codec; size on disk 53 / 9 / 13 / 7 MB):

| codec | generic | iceberg-rust | hand | hand, entries per second |
|---|---:|---:|---:|---:|
| null | 2.47 s | 5.74 s | 0.14 s | 1.42 M |
| deflate | 2.51 s | 5.71 s | 0.23 s | 0.87 M |
| snappy | 2.41 s | 5.75 s | 0.28 s | 0.73 M |
| zstandard | 2.45 s | 5.81 s | 0.20 s | 1.02 M |

Decompression costs the hand decoder 40 to 100% on top of its decode; the other two are unaffected because they are dominated by building their value trees.

**What this means for the planning numbers in ADR 0005.** "Planning 1,000,000 files: 14-22 s on one thread when the filter cannot prune" was measured with the generic reader; its decode alone is
12 s of that here. A hand decoder would take that part to about 0.7 s. That is a projection from this bake-off, NOT a re-measured planning time.

## Correctness

- **All three decoders agree entry for entry** (every field above, bounds byte for byte, delete entries): 1,010,000 entries of the 1M table and 202,000 entries in each of the four codecs.
- **Foreign manifests:** 147 manifests (2,570 entries, deflate) written by pyiceberg 0.12.0 for 19 random tables (every partition transform, appends, partition and schema evolution, deletes by rewrite;
  `spikes/iceberg-plan/interop/others_to_ours.py`): hand and iceberg-rust agree on all 147. 603 partition fields in them have a type the spike decoder does not keep (strings, decimals, ...); it counts them
  and stores null, it does not guess. No manifest written by the Java library was available.
- **Tests of the hand decoder** (`tests/hand.rs`, 11 tests): agreement with the reference reader on every codec; delete manifests keep their referenced files; the column filter changes nothing else; garbage and
  truncated input is an error (a file cut at a block boundary is a valid shorter file, as Avro has no end marker, and never decodes as the full count); 3,000 single-byte corruptions never panic or hang; a block count or
  an array of zero-size items with a count of 2^60 is rejected at once; a wrong sync marker is an error. **6 of 6 deliberate bugs are caught** (no count guard, no sync check, no checksum check, NaN counts skipped, bounds
  swapped, no UTF-8 check), baseline green twice.
- **Two defects found by those checks, both fixed:** (1) the fuzz test showed a flipped bit in a path was accepted as invalid UTF-8; Avro strings must be UTF-8, the decoder now rejects them. (2) The snappy trailer is a
  CRC-32 (IEEE), not CRC-32C as first written; comparing with real files caught it.
- **Integrity of the formats (from the 3,000 flips):** in an UNCOMPRESSED manifest, 1,121 of 1,500 byte flips decoded to a different but valid result with no error (plain Avro has no checksum), in snappy-compressed
  manifests 0 of 1,500 did. So a decoder cannot be the only guard: the planner has to cross-check each manifest's entry count against the manifest list, and the object store's own checksum has to be used.

## What this does not show

One synthetic table of one shape: 8 columns of statistics; tables with hundreds of columns cost proportionally more memory per entry unless the column filter is used. Not run on manifests written by the Java library or by
DuckDB. Partition types other than int, long, date and timestamp are not decoded yet (the product decoder needs all of them: decimal, string, bytes, fixed, uuid). Manifests are resolved by field NAME in the standard v2
layout; v1 and v3 manifests, and resolution by field id after a rename, are not handled. Manifest LISTS are not decoded by it. Timings are from a loaded development machine; a settled GCP run would be needed for
final numbers. iceberg-rust 0.11 was used as published; a later version or its async/arrow paths might differ.
