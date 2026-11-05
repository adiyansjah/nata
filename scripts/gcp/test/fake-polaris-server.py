#!/usr/bin/env python3
"""A stand-in for the Polaris server, for the tests of spikes/iceberg-commit/scripts/polaris.sh. It checks what polaris.sh gets WRONG if it is careless:
the bootstrap credentials must arrive as POLARIS_BOOTSTRAP_CREDENTIALS='realm,clientId,clientSecret' (three fields), the token request must carry exactly those
credentials, every management call must carry the bearer token, and the catalog must be created with FILE storage and an allowed location before its grants.
Every request is appended to $FAKE_LOG as 'METHOD PATH' (never the credentials). FAKE_FAIL=<word> makes any request whose path contains the word return 500.
Reads -Dquarkus.http.port and -Dquarkus.management.port from the config file named by -Dquarkus.config.locations in POLARIS_JAVA_OPTS."""
import json, os, re, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs

opts = os.environ.get("POLARIS_JAVA_OPTS", "")
cfg = re.search(r"-Dquarkus\.config\.locations=(\S+)", opts)
props = open(cfg.group(1)).read() if cfg else ""
port = int(re.search(r"quarkus\.http\.port=(\d+)", props).group(1))
mgmt = int(re.search(r"quarkus\.management\.port=(\d+)", props).group(1))
log = os.environ["FAKE_LOG"]
fields = os.environ.get("POLARIS_BOOTSTRAP_CREDENTIALS", "").split(",")
if len(fields) != 3 or fields[0] != "POLARIS":
    print("Invalid credentials format: expected realm,clientId,clientSecret", file=sys.stderr)
    sys.exit(1)
_, CID, SEC = fields
TOKEN = "tok-" + SEC[:6]
state = {"catalog": None}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _rec(self): open(log, "a").write(f"{self.command} {self.path}\n")
    def _reply(self, code, body=b""):
        self.send_response(code); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def _body(self):
        n = int(self.headers.get("Content-Length") or 0); return self.rfile.read(n).decode()
    def _go(self):
        self._rec()
        if os.environ.get("FAKE_FAIL") and os.environ["FAKE_FAIL"] in self.path: return self._reply(500, b"boom")
        if self.path == "/q/health/ready": return self._reply(200, b"ok")
        body = self._body()
        if self.path == "/api/catalog/v1/oauth/tokens":
            q = parse_qs(body)
            if q.get("grant_type") == ["client_credentials"] and q.get("client_id") == [CID] and q.get("client_secret") == [SEC] and q.get("scope") == ["PRINCIPAL_ROLE:ALL"]:
                return self._reply(200, json.dumps({"access_token": TOKEN, "token_type": "bearer", "expires_in": 3600}).encode())
            return self._reply(401, b"bad credentials")
        if self.headers.get("Authorization") != "Bearer " + TOKEN: return self._reply(401, b"no token")
        if self.command == "POST" and self.path == "/api/management/v1/catalogs":
            c = json.loads(body)["catalog"]; st = c["storageConfigInfo"]
            if st.get("storageType") != "FILE" or not st.get("allowedLocations") or not c["properties"]["default-base-location"].startswith("file://"):
                return self._reply(400, b"catalog needs FILE storage and an allowed location")
            state["catalog"] = c["name"]; return self._reply(201, b"{}")
        if self.command == "PUT" and re.fullmatch(r"/api/management/v1/catalogs/[^/]+/catalog-roles/catalog_admin/grants", self.path):
            if state["catalog"] is None: return self._reply(404, b"catalog first")
            return self._reply(201)
        if self.command == "PUT" and re.fullmatch(r"/api/management/v1/principal-roles/service_admin/catalog-roles/[^/]+", self.path):
            return self._reply(201)
        return self._reply(404, b"unknown")
    do_GET = do_POST = do_PUT = _go

for p in (mgmt,):
    threading.Thread(target=HTTPServer(("127.0.0.1", p), H).serve_forever, daemon=True).start()
HTTPServer(("127.0.0.1", port), H).serve_forever()
