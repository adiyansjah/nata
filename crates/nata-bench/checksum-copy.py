#!/usr/bin/env python3
"""Copy a Parquet file into a new one WITH page checksums (CRC-32 in every page header): the input of the R4 cost arm of scan-bench.sh. DuckDB and parquet-rs 60.1.0
cannot write checksums; pyarrow can. One input row group becomes one output row group (streamed, so SF10 does not need the memory of the table); snappy, like the
files DuckDB writes. The copy has the same rows in the same order: scan-bench.sh checks the row count against DuckDB's before it times anything.
usage: checksum-copy.py in.parquet out.parquet"""
import sys
import pyarrow.parquet as pq

src, dst = sys.argv[1], sys.argv[2]
f = pq.ParquetFile(src)
w = pq.ParquetWriter(dst, f.schema_arrow, compression="snappy", write_page_checksum=True)
for i in range(f.num_row_groups):
    w.write_table(f.read_row_group(i))
w.close()
g = pq.ParquetFile(dst)
assert g.metadata.num_rows == f.metadata.num_rows, "row count differs"
print(f"{dst}: {g.metadata.num_rows} rows, {g.num_row_groups} row groups, page checksums on")
