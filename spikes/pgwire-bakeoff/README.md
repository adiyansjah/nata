# Local throwaway protocol bake-off (not the server)

Kept **outside the production workspace** for the parent to replay the unchanged pgwire crash and the known-bad controls. Delete when
slice 4b step 4 has permanent decoder/resource checks. No `nata serve` implementation lives here. See the authoritative method,
counters and limitations in `bench/results/20261107-pgwire-baseline.md` and ADR 0008.

Commands assume the checkout and home-directory layout used for this local experiment; Python/Java fixture paths are explicit.
Dependencies are pinned by this crate's lockfile. Rust forbids unsafe here too.

```sh
cd /home/dev/src/nata
mkdir -p "$HOME/tmp-4b/clients"
export TMPDIR="$HOME/tmp-4b"
env -u AI_AGENT duckdb -no-agent -c "COPY (SELECT i::BIGINT AS v FROM range(1000000) t(i)) TO '/home/dev/tmp-4b/million.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 16384);"
env -u AI_AGENT duckdb -no-agent -c "COPY (SELECT true AS b, 127::TINYINT AS ti, (-32768)::SMALLINT AS si, 2147483647::INTEGER AS i, 9223372036854775807::BIGINT AS bi, 1.25::FLOAT AS f, 0.0::DOUBLE AS d, 123.450::DECIMAL(10,3) AS n, 'a,b' AS s, DATE '2024-02-29' AS dt) TO '/home/dev/tmp-4b/types.parquet' (FORMAT PARQUET);"
cargo build --locked --manifest-path spikes/pgwire-bakeoff/Cargo.toml
timeout 20s spikes/pgwire-bakeoff/target/debug/pgwire-bakeoff fuzz
python3 spikes/pgwire-bakeoff/mutate.py
python3 spikes/pgwire-bakeoff/driver.py
python3 spikes/pgwire-bakeoff/load.py
python3 spikes/pgwire-bakeoff/limits.py
cargo deny --manifest-path spikes/pgwire-bakeoff/Cargo.toml --config deny.toml check
```

`driver.py` alternates bounded arms twice, then the deliberate collect-all implementation once each; it writes `$HOME/tmp-4b/memory.json`.
It reads slowly for four seconds per case; there is no throughput claim. `load.py` samples eight pending queries and checks the thread
census after disconnect. Both use real `plan_sql_with` + `execute`. `limits.py` checks invalid/oversized-header rejection and cancel
isolation. `mutate.py` removes one exact checked-read guard, compiles, requires fuzzer exit 101, then restores and rebuilds even on error.
Run it without another build or probe changing these same files.

Thread-pool canary: start `target/debug/pgwire-bakeoff pg 15800 pool-mutant` from this directory, then connect and inspect `/proc/<pid>/task`.
The two `rogue-pool` workers make the expected-three idle-thread check fail (five total). Without the flag: three idle, six with one
pending query, 27 with eight pending. These are plateau counts, excluding transient parser/schema helper threads.

Optional clients, user-local on Debian 13 (do not install system-wide):

```sh
cd "$HOME/tmp-4b/clients"
apt-get download postgresql-client-17 libpq5 openjdk-21-jre-headless openjdk-21-jdk-headless libpostgresql-jdbc-java
for pkg in ./*.deb; do dpkg-deb -x "$pkg" root; done
```

The extracted JDK's `conf` symlinks point to system paths; redirect those under the extracted root:

```python
from pathlib import Path
root = Path.home() / 'tmp-4b/clients/root'
for p in root.rglob('*'):
    if p.is_symlink():
        target = p.readlink()
        if target.is_absolute() and str(target).startswith('/etc/java-21-openjdk'):
            local = root / str(target)[1:]
            if local.exists():
                p.unlink()
                p.symlink_to(local)
```

From the checkout:

```sh
uv venv "$HOME/tmp-4b/py"
uv pip install --python "$HOME/tmp-4b/py/bin/python" 'psycopg[binary]==3.3.8'
cp spikes/pgwire-bakeoff/{Clients,Types}.java "$HOME/tmp-4b/"
"$HOME/tmp-4b/clients/root/usr/lib/jvm/java-21-openjdk-amd64/bin/javac" "$HOME/tmp-4b/Clients.java" "$HOME/tmp-4b/Types.java"
cp spikes/pgwire-bakeoff/type-client.py "$HOME/tmp-4b/"
python3 spikes/pgwire-bakeoff/verify.py
```

`verify.py` verifies every million-row value plus the raw extended response, runs psql simple/extended and SIGINT cancel, JDBC simple/extended
and cancel, and JDBC/psycopg type readback. It uses extracted psql's `LD_LIBRARY_PATH`. The JDBC runner explicitly disables binary transfer
and server-named prepares. This harness is not a substitute for plan item 35.

For dependency reproduction: save the root `Cargo.lock` and `crates/nata/Cargo.toml` outside the repo, run `cargo metadata --format-version 1`
to a baseline file, temporarily add the exact pgwire dependency from this manifest under nata's normal `[dependencies]`, then run metadata
and `cargo deny check` again. Restore both files byte-for-byte in a `finally` block. Compare registry `(name,version)` pairs between the
metadata files, counting regex `\bunsafe\s*\{` over all `.rs` files below each new manifest directory. This inventories examples/tests and
all targets too, including inactive text; it is not a macro-expanded or native-code soundness audit. The main record contains all 58
workspace-delta counts and ten additional independent-spike resolution counts. A temporary bans config with pgwire denied must fail.

Security/coverage limits: loopback only, deterministic cancel keys, no authentication/TLS, no production connection/startup caps, no
parameter substitution or extended error recovery. Do not expose or reuse this harness as a service. The shared frame cap is checked before
body allocation. No errors print SQL cell values. The telemetry prints only counters, not rows.
