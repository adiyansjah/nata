#!/usr/bin/env python3
"""Writes the small Parquet files WITH page checksums (CRC-32 in every page header) that the fuzz test and tests/checksums.rs damage. parquet-rs 60.1.0 cannot write
checksums ("TODO: Add support for crc checksum" in column/page.rs), so pyarrow does it (`write_page_checksum=True`). The files are committed (a few tens of KB each):
the tests must not need pyarrow, which GitHub's runners do not have. Run again only to change a shape:

    uvx --offline --with pyarrow python -I crates/nata-parquet/tests/fixtures/make_checksummed.py crates/nata-parquet/tests/fixtures

The shapes follow the ones of tests/fuzz_files.rs (a planted marker string and a planted number must reach the output, so that a leak into an error text is seen).
"""
import datetime, decimal, os, sys
import pyarrow as pa
import pyarrow.parquet as pq

MARKER = "ZQXJ-PLANTED-7731"
NUMBER = 987_654_321
D = decimal.Decimal


def lineitem_like(path):
    """4 row groups of 600 rows: ints, decimals, dictionary strings, a date, a planted marker text and a planted number; plain pages, no compression, so that
    the marker bytes are in the file and a test can damage a value of a page."""
    n, groups = 600, 4
    total = n * groups
    t = pa.table({
        "l_orderkey": pa.array(range(total), pa.int64()),
        "l_partkey": pa.array([i % 211 for i in range(total)], pa.int32()),
        "l_quantity": pa.array([D(1 + i % 50) for i in range(total)], pa.decimal128(15, 2)),
        "l_extendedprice": pa.array([D(900 + i * 37) / 100 for i in range(total)], pa.decimal128(15, 2)),
        "l_returnflag": pa.array([("A", "N", "R")[i % 3] for i in range(total)], pa.string()),
        "l_shipdate": pa.array([datetime.date(1992, 1, 1) + datetime.timedelta(days=i % 2500) for i in range(total)], pa.date32()),
        "l_comment": pa.array([f"{MARKER} c{i % 97}" for i in range(total)], pa.string()),
        "l_secret": pa.array([NUMBER] * total, pa.int64()),
    })
    pq.write_table(t, path, row_group_size=n, compression="none", write_page_checksum=True, data_page_size=4096, use_dictionary=["l_returnflag"])


def v2_zstd_small_pages(path):
    """Data pages of version 2 and zstd, many small pages (the CRC covers the compressed payload, repetition and definition levels included)."""
    n = 1500
    t = pa.table({
        "id": pa.array(range(n), pa.int64()),
        "x": pa.array([None if i % 17 == 0 else i * 0.25 for i in range(n)], pa.float64()),
        "s": pa.array([None if i % 13 == 0 else f"{MARKER} v{i % 31}" for i in range(n)], pa.string()),
        "secret": pa.array([NUMBER] * n, pa.int64()),
    })
    pq.write_table(t, path, row_group_size=500, compression="zstd", data_page_version="2.0", write_page_checksum=True, data_page_size=512)


def nulls_and_dictionaries(path):
    """Nullable columns, an all-NULL column, dictionary pages (they carry a CRC too), snappy."""
    n = 1000
    t = pa.table({
        "id": pa.array(range(n), pa.int32()),
        "a": pa.array([None if i % 5 == 0 else i % 11 for i in range(n)], pa.int32()),
        "n": pa.array([None] * n, pa.int32()),
        "s": pa.array([None if i % 9 == 0 else f"{MARKER} d{i % 6}" for i in range(n)], pa.string()),
        "secret": pa.array([NUMBER] * n, pa.int64()),
    })
    pq.write_table(t, path, row_group_size=400, compression="snappy", use_dictionary=True, write_page_checksum=True, data_page_size=1024)


if __name__ == "__main__":
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    for name, f in (("checksummed_lineitem_like", lineitem_like), ("checksummed_v2_zstd", v2_zstd_small_pages), ("checksummed_nulls_dict", nulls_and_dictionaries)):
        p = os.path.join(out, name + ".parquet")
        f(p)
        print(name, os.path.getsize(p), "bytes")
