#!/usr/bin/env python3
"""Independent read of the metadata the spike wrote: pyiceberg lists, for every snapshot of the table, the data and delete
files it finds by reading OUR manifest lists and manifests (its own Avro reader, its own v2 schema handling). They must be
the same files that the spike's own reader found. A mismatch means the Avro we write is not what Iceberg readers expect.
usage: interop_check.py <catalog-url> <interop.json written by `ic-spike interop-table`>
exit 0 pass, 1 mismatch, 3 pyiceberg unavailable or could not run (reported as SKIPPED by gcp-check.sh)"""
import json, sys

def norm(p):
    for pre in ("file://", "file:"):
        if p.startswith(pre):
            return p[len(pre):]
    return p

def main():
    url, path = sys.argv[1], sys.argv[2]
    expected = json.load(open(path))
    try:
        from pyiceberg.catalog import load_catalog
    except Exception as e:  # not installed
        print("pyiceberg import failed:", e); return 3
    try:
        cat = load_catalog("rest", **{"uri": url})
        table = cat.load_table(("nata", expected["table"]))
    except Exception as e:
        print("pyiceberg could not load the table:", repr(e)); return 3
    bad = 0
    for s in expected["snapshots"]:
        try:
            tasks = list(table.scan(snapshot_id=s["id"]).plan_files())
        except Exception as e:
            print(f"snapshot {s['id']}: pyiceberg failed to plan the scan: {e!r}"); bad += 1; continue
        data = sorted(norm(t.file.file_path) for t in tasks)
        dels = sorted({norm(d.file_path) for t in tasks for d in t.delete_files})
        want_data = sorted(norm(p) for p in s["data"])
        want_dels = sorted(norm(p) for p in s["deletes"])
        ok = data == want_data and dels == want_dels
        print(f"snapshot {s['id']}: pyiceberg data={len(data)} deletes={len(dels)}; spike data={len(want_data)} deletes={len(want_dels)} -> {'OK' if ok else 'MISMATCH'}")
        bad += 0 if ok else 1
    print("INTEROP", "PASS" if bad == 0 else "FAIL")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
