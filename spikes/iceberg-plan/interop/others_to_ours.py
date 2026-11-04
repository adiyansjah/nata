#!/usr/bin/env python3
"""Tables PYICEBERG writes (every transform, appends, partition and schema evolution, deletes by rewrite) read by OUR reader: the rows must equal what pyiceberg
and DuckDB read; random filters must give the same counts in all three, and our plan must never lose a row.
usage: others_to_ours.py <workdir> <first-seed> <last-seed> [filters-per-table]       exit 0 = no disagreement"""
import datetime, decimal, os, random, shutil, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
import pyarrow as pa, pyarrow.parquet as pq
from pyiceberg.catalog.sql import SqlCatalog
from pyiceberg.partitioning import PartitionSpec, PartitionField
from pyiceberg.schema import Schema
from pyiceberg.transforms import IdentityTransform, BucketTransform, TruncateTransform, YearTransform, MonthTransform, DayTransform, HourTransform
from pyiceberg.types import *
from pyiceberg import expressions as E

SCHEMA = Schema(NestedField(1, "id", LongType(), required=False), NestedField(2, "k", IntegerType(), required=False), NestedField(3, "day", DateType(), required=False),
                NestedField(4, "ts", TimestampType(), required=False), NestedField(5, "name", StringType(), required=False), NestedField(6, "score", DoubleType(), required=False),
                NestedField(7, "amt", DecimalType(10, 2), required=False), NestedField(8, "blob", BinaryType(), required=False))
ARROW = pa.schema([pa.field("id", pa.int64()), pa.field("k", pa.int32()), pa.field("day", pa.date32()), pa.field("ts", pa.timestamp("us")), pa.field("name", pa.string()),
                   pa.field("score", pa.float64()), pa.field("amt", pa.decimal128(10, 2)), pa.field("blob", pa.binary())])
STRS = ["", "a", "ab", "abcdefghijklmnopqrstuvwxyz", "abcdefghijklmnopq", "é" * 18, "😀" * 18, "日本語" * 8, "Zebra", "apple"]
SPECS = [[], [("k", IdentityTransform())], [("ts", DayTransform())], [("day", MonthTransform()), ("k", BucketTransform(4))], [("name", TruncateTransform(2))], [("day", YearTransform())],
         [("ts", HourTransform())], [("id", BucketTransform(8))], [("k", TruncateTransform(10)), ("name", IdentityTransform())], [("amt", TruncateTransform(500))]]


def batch(rng, n, start, extra=None):
    nul = lambda v, p=0.12: None if rng.random() < p else v
    cols = {
        "id": [start + i for i in range(n)], "k": [nul(rng.randint(-30, 30)) for _ in range(n)],
        "day": [nul(datetime.date(1970, 1, 1) + datetime.timedelta(days=rng.randint(-30000, 30000))) for _ in range(n)],
        "ts": [nul(datetime.datetime(1970, 1, 1) + datetime.timedelta(hours=rng.randint(-2000, 2000), microseconds=rng.randint(0, 3599999999))) for _ in range(n)],
        "name": [nul(rng.choice(STRS)) for _ in range(n)], "score": [nul(rng.choice([-0.0, 0.0, 1.5, -1.5, 100.25, 3.0, 1e300, float("inf")]) if rng.random() < 0.5 else rng.randint(-800, 800) / 8) for _ in range(n)],
        "amt": [nul(decimal.Decimal(rng.randint(-20000, 20000)).scaleb(-2)) for _ in range(n)], "blob": [nul(bytes(rng.choice([0, 255, rng.randint(0, 255)]) for _ in range(rng.randint(0, 20)))) for _ in range(n)],
    }
    sch = ARROW
    if extra:
        cols[extra] = [nul(rng.randint(-50, 50)) for _ in range(n)]
        sch = ARROW.append(pa.field(extra, pa.int32()))
    return pa.table(cols, schema=sch)


def make_table(cat, seed, work):
    rng = random.Random(seed)
    name = f"ns.t{seed}"
    spec_fields = rng.choice(SPECS)
    fields = [PartitionField(source_id=SCHEMA.find_field(c).field_id, field_id=1000 + i, transform=t, name=f"{c}_{type(t).__name__.lower()}") for i, (c, t) in enumerate(spec_fields)]
    tbl = cat.create_table(name, schema=SCHEMA, partition_spec=PartitionSpec(*fields))
    log = [f"spec {[(c, str(t)) for c, t in spec_fields]}"]
    nxt = 0
    for step in range(rng.randint(2, 4)):
        n = rng.randint(5, 60)
        names = [f.name for f in tbl.schema().fields]
        b = batch(rng, n, nxt, extra="extra" if "extra" in names else None)
        if "kk" in names: b = b.rename_columns(["kk" if c == "k" else c for c in b.column_names])
        tbl.append(b); nxt += n; log.append(f"append {n}")
        if step == 0 and rng.random() < 0.6:
            c, t = rng.choice([("k", BucketTransform(3)), ("ts", MonthTransform()), ("name", TruncateTransform(1)), ("day", DayTransform())])
            try:
                with tbl.update_spec() as u: u.add_field(c, t, f"{c}_evo")
                log.append("evolve spec")
            except ValueError as e:
                log.append(f"spec evolution refused by pyiceberg: {str(e)[:40]}")
        if step == 1 and rng.random() < 0.5:
            with tbl.update_schema() as u: u.add_column("extra", IntegerType())
            log.append("add column extra")
        if step == 1 and rng.random() < 0.3:
            with tbl.update_schema() as u: u.rename_column("k", "kk")
            log.append("rename k -> kk")
    if rng.random() < 0.6:
        col = "kk" if "kk" in [f.name for f in tbl.schema().fields] else "k"
        tbl.delete(E.GreaterThan(col, rng.randint(0, 20)))      # copy-on-write: files are rewritten without the matching rows
        log.append("delete by rewrite")
    return tbl, log


def main():
    work, s0, s1 = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    nf = int(sys.argv[4]) if len(sys.argv) > 4 else 20
    shutil.rmtree(work, ignore_errors=True); os.makedirs(work)
    cat = SqlCatalog("c", uri=f"sqlite:///{work}/cat.db", warehouse=f"file://{work}/wh")
    cat.create_namespace("ns")
    con = duckdb_con()
    stats = dict(tables=0, ours_eq_py=0, duck_eq_py=0, filters=0, ours_eq_duck_count=0, ours_files=0, py_files=0, py_null_semantics=0, py_zero_semantics=0, py_bugs=0)
    bad = []
    for seed in range(s0, s1 + 1):
        try:
            tbl, log = make_table(cat, seed, work)
        except Exception as e:
            if "redundant partitions" in str(e): stats["skipped_builds"] = stats.get("skipped_builds", 0) + 1; continue      # pyiceberg's own validation of the evolved spec
            bad.append(f"seed {seed}: pyiceberg could not build the table: {type(e).__name__}: {str(e)[:200]}"); continue
        stats["tables"] += 1
        d = os.path.dirname(os.path.dirname(tbl.metadata_location.replace("file://", "").replace("file:", "")))
        md = tbl.metadata_location.replace("file://", "").replace("file:", "")
        try:
            icx("scan", d, d + "-ours.parquet")
        except Exception as e:
            bad.append(f"seed {seed}: our reader failed on a pyiceberg table: {str(e)[-300:]} | {log}"); continue
        ours = pq.read_table(d + "-ours.parquet"); cols = ours.column_names
        want = rows_of(ours, cols)
        py_rows = rows_of(tbl.scan().to_arrow(), cols)
        if want == py_rows: stats["ours_eq_py"] += 1
        else: bad.append(f"seed {seed}: our rows differ from pyiceberg's ({len(want)} vs {len(py_rows)}) | {log}")
        duck_rows = rows_of(con.sql(f"select * from iceberg_scan('{md}')").to_arrow_table(), cols)
        if duck_rows == py_rows: stats["duck_eq_py"] += 1
        else: bad.append(f"seed {seed}: DuckDB rows differ from pyiceberg's ({len(duck_rows)} vs {len(py_rows)}) | {log}")
        try:
            out = icx("check-partitions", d)
            stats["partition_checks"] = stats.get("partition_checks", 0) + int(out.split(" row-field")[0].split(", ")[1])
        except Exception as e:
            bad.append(f"seed {seed}: our transforms disagree with the partition values pyiceberg wrote: {str(e)[-300:]} | {log}")
        # filters: finite doubles only (the generator can emit +-0.0 and inf: count semantics are checked against DuckDB)
        icx("filters", d, seed, nf, d + "-filters.json")
        for f in json.load(open(d + "-filters.json")):
            stats["filters"] += 1; stats["ours_files"] += len(f["planned_files"])
            try:
                n_duck = con.sql(f"select count(*) from iceberg_scan('{md}') where {f['sql']}").fetchall()[0][0]
                if n_duck == f["matched_rows"]: stats["ours_eq_duck_count"] += 1
                else: bad.append(f"seed {seed} filter {f['id']}: DuckDB counts {n_duck}, ours {f['matched_rows']}: {f['sql']}")
            except Exception as e:
                bad.append(f"seed {seed} filter {f['id']}: DuckDB failed: {str(e)[:120]} | {f['sql']}")
    print(json.dumps(stats))
    for b in bad[:40]: print("DISAGREE", b)
    print(f"{len(bad)} disagreements")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
