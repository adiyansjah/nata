#!/usr/bin/env python3
"""Does the catalog honour the REST `Idempotency-Key` header on a table commit? (ADR 0005, rule 8: our own operation token against the standard mechanism.)

usage: idempotency_probe.py <catalog base url> [--wait-ttl SECONDS]
Environment (the same as the Rust client and interop_check.py): IC_REST_CLIENT_ID, IC_REST_CLIENT_SECRET, IC_REST_SCOPE, IC_REST_WAREHOUSE.

Each test commits ONE snapshot to a fresh empty table (a commit whose second application must fail: the branch `main` must not exist yet) and prints one line
`IDEM <test> ...`. It reports what the server DID; only a harness failure (cannot connect, cannot create the table, no token) is an error (exit 1).
With --wait-ttl N it also waits N seconds and replays a key (use the server's ttl + 1).
Tests: no-key (the baseline: a duplicate conflicts), same-key (the replay), same-key-different-body, other-key, bad-key (not a UUID), after-ttl."""
import json, os, sys, time, urllib.error, urllib.parse, urllib.request, uuid


def http(method, url, body=None, headers=None):
    h = {"Content-Type": "application/json"}
    h.update(headers or {})
    data = None if body is None else (body if isinstance(body, bytes) else json.dumps(body).encode())
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, r.read().decode(), dict(r.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(), dict(e.headers)


def main():
    base = sys.argv[1].rstrip("/")
    wait_ttl = int(sys.argv[sys.argv.index("--wait-ttl") + 1]) if "--wait-ttl" in sys.argv else 0
    auth = {}
    cid, sec = os.environ.get("IC_REST_CLIENT_ID"), os.environ.get("IC_REST_CLIENT_SECRET")
    if cid and sec:
        form = urllib.parse.urlencode({"grant_type": "client_credentials", "client_id": cid, "client_secret": sec, "scope": os.environ.get("IC_REST_SCOPE", "PRINCIPAL_ROLE:ALL")}).encode()
        st, text, _ = http("POST", f"{base}/v1/oauth/tokens", form, {"Content-Type": "application/x-www-form-urlencoded"})
        if st != 200:
            print(f"HARNESS FAILURE: token request returned {st}"); return 1
        auth = {"Authorization": "Bearer " + json.loads(text)["access_token"]}
    wh = os.environ.get("IC_REST_WAREHOUSE")
    st, text, _ = http("GET", f"{base}/v1/config" + (f"?warehouse={urllib.parse.quote(wh)}" if wh else ""), headers=auth)
    if st != 200:
        print(f"HARNESS FAILURE: GET /v1/config returned {st}"); return 1
    cfg = json.loads(text)
    adv = {k: v for sect in ("defaults", "overrides") for k, v in cfg.get(sect, {}).items() if "idempot" in k.lower()}
    prefix = cfg.get("overrides", {}).get("prefix") or cfg.get("defaults", {}).get("prefix") or ""
    print(f"IDEM config-advertises-idempotency={bool(adv)} {adv}")
    root = f"{base}/v1/{prefix}" if prefix else f"{base}/v1"
    ns = "idem" + uuid.uuid4().hex[:6]
    st, text, _ = http("POST", f"{root}/namespaces", {"namespace": [ns], "properties": {}}, auth)
    if st not in (200, 201, 409):
        print(f"HARNESS FAILURE: create namespace {st} {text[:200]}"); return 1

    def new_table():
        name = "t" + uuid.uuid4().hex[:8]
        body = {"name": name, "schema": {"type": "struct", "schema-id": 0, "fields": [{"id": 1, "name": "id", "required": True, "type": "long"}]}, "properties": {"format-version": "2"}}
        st, text, _ = http("POST", f"{root}/namespaces/{ns}/tables", body, auth)
        if st != 200:
            print(f"HARNESS FAILURE: create table {st} {text[:300]}"); sys.exit(1)
        v = json.loads(text)
        return name, v["metadata"]["table-uuid"], v["metadata"]["location"]

    def commit_body(table_uuid, location, snap_id):
        snap = {"snapshot-id": snap_id, "sequence-number": 1, "timestamp-ms": int(time.time() * 1000), "manifest-list": f"{location}/metadata/snap-{snap_id}.avro",
                "summary": {"operation": "append"}, "schema-id": 0}
        return {"requirements": [{"type": "assert-table-uuid", "uuid": table_uuid}, {"type": "assert-ref-snapshot-id", "ref": "main", "snapshot-id": None}],
                "updates": [{"action": "add-snapshot", "snapshot": snap}, {"action": "set-snapshot-ref", "ref-name": "main", "type": "branch", "snapshot-id": snap_id}]}

    def post(name, body, key=None):
        h = dict(auth)
        if key is not None:
            h["Idempotency-Key"] = key
        st, text, hdr = http("POST", f"{root}/namespaces/{ns}/tables/{name}", body, h)
        return st, text, hdr

    def snapshots(name):
        st, text, _ = http("GET", f"{root}/namespaces/{ns}/tables/{name}", None, auth)
        return len(json.loads(text)["metadata"].get("snapshots", [])) if st == 200 else -1

    def body_id(text):
        try:
            return json.loads(text)["metadata"]["snapshots"][0]["snapshot-id"]
        except Exception:
            return None

    # 1. baseline: no key, the same commit twice
    n, u, loc = new_table(); b = commit_body(u, loc, 1001)
    s1, t1, _ = post(n, b); s2, t2, _ = post(n, b)
    print(f"IDEM no-key first={s1} second={s2} snapshots={snapshots(n)}")
    # 2. same key twice
    k = str(uuid.uuid4()); n, u, loc = new_table(); b = commit_body(u, loc, 1002)
    s1, t1, _ = post(n, b, k); s2, t2, h2 = post(n, b, k)
    same = s1 == s2 == 200 and body_id(t1) == body_id(t2)
    print(f"IDEM same-key first={s1} second={s2} same-response={same} snapshots={snapshots(n)} replay-header={ {a: v for a, v in h2.items() if 'idempot' in a.lower()} }")
    # 3. same key, a DIFFERENT body
    k = str(uuid.uuid4()); n, u, loc = new_table()
    s1, _, _ = post(n, commit_body(u, loc, 1003), k); s2, t2, _ = post(n, commit_body(u, loc, 1004), k)
    print(f"IDEM same-key-different-body first={s1} second={s2} snapshots={snapshots(n)} second-body={t2[:120]!r}")
    # 4. another key for the same, already applied, commit
    n, u, loc = new_table(); b = commit_body(u, loc, 1005)
    s1, _, _ = post(n, b, str(uuid.uuid4())); s2, _, _ = post(n, b, str(uuid.uuid4()))
    print(f"IDEM other-key first={s1} second={s2} snapshots={snapshots(n)}")
    # 5. a key that is not a UUID
    n, u, loc = new_table()
    s1, t1, _ = post(n, commit_body(u, loc, 1006), "not-a-uuid")
    print(f"IDEM bad-key status={s1} body={t1[:100]!r} snapshots={snapshots(n)}")
    # 6. a replay after the server's time to live
    if wait_ttl:
        k = str(uuid.uuid4()); n, u, loc = new_table(); b = commit_body(u, loc, 1007)
        s1, _, _ = post(n, b, k); time.sleep(wait_ttl); s2, _, _ = post(n, b, k)
        print(f"IDEM after-ttl waited={wait_ttl}s first={s1} second={s2} snapshots={snapshots(n)}")
    print("IDEM PROBE DONE")
    return 0


sys.exit(main())
