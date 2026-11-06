# Postgres-wire baseline, slice 4b step 1 — 2026-11-07

**Decision: implement the supported codec ourselves.** The unchanged reuse candidate fails R1(iii).
This is a local design spike, not slice closeout or the plan-item-35 certification job. No GCP, push or production server was used.

The card's rule, unchanged:

> **R1, protocol implementation:** reuse the `pgwire` crate unless the bake-off shows it cannot (i) run under our scheduler without a second
> thread pool beyond ADR 0002's tokio I/O runtime, (ii) stream results with backpressure, (iii) be fuzzed without panics, or (iv) pass
> `cargo-deny` and the unsafe inventory. Otherwise write the message codec for the supported messages ourselves (a few hundred lines).

| R1 criterion | pgwire 0.42.1 | Checked hand codec | Verdict |
|---|---|---|---|
| (i) no extra protocol pool | 3 idle / 6 one pending query / 27 eight pending queries; 3 after disconnect | same counts | both meet the local integration probe; no hidden pgwire pool |
| (ii) streaming/backpressure | 13,324 / 13,300 KiB peak server RSS, two fresh processes; production stops at 159,354 of 1M rows | 13,352 / 13,192 KiB; production stops at 160,772 | both meet; collect-all mutants reach 178,660 / 177,504 KiB |
| (iii) fuzz without panic | **2,015 panics / 10,000 regular cases**; 0 / 10,000 startup cases | **0 / 10,000** regular and **0 / 10,000** startup; guard-removal mutant panics 471 times and exits 101 | pgwire fails, hand meets corpus |
| (iv) cargo-deny + unsafe inventory | added full-workspace graph passes all four categories, exit 0; pgwire has 0 textual unsafe blocks, dependencies inventoried below | own code forbids unsafe; tokio dependencies remain necessary | both acceptable under the existing dependency policy |

A single disqualifier suffices under R1. Thread/RSS differences are not speed claims. A wrapper that catches panics would still fail
“without panics”; a full pre-validator or fork is a different implementation, not measured reuse of 0.42.1.
[ADR 0008](../../docs/decisions/0008-client-protocol.md) records the decision and production obligations.

## Method and scope

Local x86-64 Debian 13, Intel Core i3-1220P, Rust/Cargo 1.99.1. Shared development machine; warm filesystem cache. The compile/test phase
raised load substantially (a sampled one-minute load was 16.83); after it, a sample was 0.63. **SANITY ONLY for performance**: no settled
latency/throughput benchmark, no ARM64 measurement, no 20% streaming-budget verdict. R1's deterministic panic, thread census and resource
controls do not depend on a wall-clock speed comparison. The fixed wait durations are test stimulus, not benchmark results.

`spikes/pgwire-bakeoff` is an independent Cargo workspace under the existing `exclude = ["spikes"]` boundary. Both arms use the **same**
`nata_sql::plan_sql_with`/`nata_exec::execute` path called by slice 4's `nata sql`, two scan workers, window four, footers-ahead eight.
Planning runs off tokio with the existing big-stack parser helper. A bounded 32-row channel bridges the synchronous query coordinator to
async output. Local decode/evaluation stay on Nata threads. No SQL subprocess, CSV parser, mocked result or generated-row substitution
stands in for query execution. Protocol loop/state and query bridge are shared experimental scaffolding; the competing pieces are the
checked hand frontend/backend codec versus pgwire's actual frontend decoder and `QueryResponse` Stream/Framed backend sink. It is not a
bake-off of pgwire's default SQL/session handlers. Production extended recovery and admission are still to be implemented.

The hand decoder covers Startup, SSL/GSS refusal, CancelRequest, Query, Parse, Bind (bounded length/count/text-format checks), Describe,
Execute, Sync, Close, Flush and Terminate. The live spike implements no-parameter unnamed execution; it parses but does not substitute Bind
parameters. It does not claim binary, cursor suspension, error-state recovery or complete session chatter. The hand backend encodes auth,
parameters, key, ready, row description/data, completion and errors; pgwire supplies its stream/row encoding. Both use the same limited
startup/session harness so real-client differences can be attributed narrowly.

### Correctness before resource measurements

DuckDB 2.0.0 generated 1,000,000 BIGINT values 0 through 999,999, Parquet row groups of 16,384. `nata sql --csv --threads 2 --window 4`
and DuckDB CSV compare byte-for-byte (`cmp`, exit 0), SHA-256:
`17e9c233901541234eda2f0a66f488ff046c6373fc529393db620e761b2731dd`.
The raw client verifies **each** of all 1,000,000 wire values in order for both arms, then an unnamed extended exchange with tags
`ParseComplete, BindComplete, RowDescription, DataRow, CommandComplete, ReadyForQuery`. `LIMIT 1` returns `0` before each resource probe.

## (i) Threading

Audited `pgwire-0.42.1/src/tokio/server.rs`, `api/query.rs` and all source matches for `spawn`, `Runtime`, `thread::`, `block_on`:
spawn sites in its source are tests; the server API awaits the caller's socket and sink. It constructs no runtime or worker pool.
Linking `futures-executor` does not start its optional thread pool. Cargo features exclude TLS/crypto executors.

`/proc/<server-pid>/task/*/comm` and `status` show main + two `tokio-rt-worker` threads idle. A pending query adds `nata-query`,
`nata-scan-0`, `nata-scan-1`: six total, both arms. Eight blocked queries add 8 × 3: 27 total, both; disconnecting them returns to three.
The parser/schema helper can transiently add threads during planning; the reported pending-query plateau is after planning, not a bound
on every transient. The extra query threads are the existing synchronous execution/coordinator boundary, **not** a claim that step 1 has
built ADR 0002's globally budgeted server query admission.

Known-bad control: `pool-mutant` constructs a second two-worker tokio runtime with thread names `rogue-pool`. Idle census becomes five
and the expected-three check fails. No change to the production graph. [Raw load record](20261107-pgwire-baseline/load.txt).

## (ii) Bounded output

Fresh processes, alternating hand/pg/hand/pg, then one collect-all mutant for each. Client receive buffer 4 KiB; it reads at most 4 KiB
per 50 ms for 80 iterations. Server RSS/HWM sampled every iteration. Peak below is Linux VmHWM, not merely the last sample; RSS is
server-process memory, excluding the client, kernel TCP buffers, page cache and build processes. This fixed-size integer result does
not establish a bound for arbitrary-width cells or the SF10 R16 workload.

| Arm / run | Idle RSS KiB | Peak VmHWM KiB | First / last sampled RSS KiB | Client bytes consumed | Producer progress at 1/2/3/4 seconds |
|---|---:|---:|---|---:|---|
| hand 1 | 5,584 | 13,352 | 12,840 / 13,352 | 153,755 | 160,772 / 160,772 / 160,772 / 160,772 |
| pgwire 1 | 5,832 | 13,324 | 13,068 / 13,020 | 126,989 | 159,354 / 159,354 / 159,354 / 159,354 |
| hand 2 | 5,748 | 13,192 | 13,004 / 13,148 | 153,755 | 160,772 / 160,772 / 160,772 / 160,772 |
| pgwire 2 | 5,856 | 13,300 | 13,220 / 13,300 | 126,989 | 159,354 / 159,354 / 159,354 / 159,354 |
| hand collect-all | 5,768 | 177,504 | 35,248 / 177,504 | 145,902 | 1,000,000 at every sample |
| pgwire collect-all | 5,944 | 178,660 | 34,996 / 178,660 | 143,501 | 1,000,000 at every sample |

Over 839,000 of the million result rows remain unproduced while the slow reader is pending. The counter stops rather than reaching a
million, distinguishing backpressure from a merely small encoding. The bridge holds 32 rows; Nata retains bounded row-group/batch
buffers, and TCP absorbs some output. The mutant collects `Vec<Vec<Option<String>>>` in the producer before sending: it exceeds a
diagnostic 32 MiB ceiling and reaches every row. The ceiling is a probe canary, not a replacement for the card's scan/R16
formula. No allowance was increased. [Raw memory samples](20261107-pgwire-baseline/memory.json).

Key path (the retained source is authoritative):

```rust
let (tx, rx) = tokio::sync::mpsc::channel(32);
// Off the I/O runtime, in the existing execute sink:
tx.blocking_send(Ok(row))?;
// Reuse arm: Stream::unfold(rx, ... recv().await ...), then:
pgwire::api::query::send_query_response(&mut framed, response, send_description).await?;
// Hand arm: receive one row, encode it, socket.write_all(&packet).await?;
```

pgwire `send_query_response` awaits `feed` for each row. Tokio-util Framed's sink readiness flushes its bounded write buffer before
accepting more, so `feed` is not whole-response buffering. The measured collect-all control isolates the opposite implementation.

## (iii) Seeded decoder corpus and mutants

Seed printed: **322661912839** (`0x4b20261107`), xorshift64 `(13,7,17)`, reset for each candidate. For each of 10,000 regular cases select
one of `Q P B D E C S H X`, 0–127 pseudorandom payload bytes, and one of lengths 4, 2, signed-max, payload+4, payload+5. Length-four cases
truncate to the header. Startup's separate 10,000 cases remove the tag and alternate startup/cancel magic when a protocol word exists.
This is a deterministic mutation corpus, **not** exhaustive coverage or coverage-guided fuzzing; it includes valid empty-message/cancel
controls, hence accepted counts. Each decoder invocation is inside `catch_unwind` for counting; the enclosing command has a 20-second
timeout, exits 0 normally, and asserts zero hand panics. The pgwire decoder itself is not repaired or prevalidated by the hand decoder.

| Corpus | Decoder | Panic | Rejected/incomplete | Accepted |
|---|---|---:|---:|---:|
| regular 10,000 | hand | 0 | 9,334 | 666 |
| regular 10,000 | pgwire | 2,015 | 4,000 | 3,985 |
| startup 10,000 | hand | 0 | 9,990 | 10 |
| startup 10,000 | pgwire | 0 | 9,028 | 972 |

Neither run hangs. Rejected/incomplete shares one counter: truncated buffers can legitimately return `None` until EOF; the live framing
layer must turn EOF-in-frame into a closed session. Acceptance counts alone are not correctness claims.

Minimal known-bad input (complete claimed frame, missing Parse fields):

```rust
let bytes = b"P\0\0\0\x04";
let mut ctx = pgwire::messages::DecodeContext::default();
ctx.awaiting_frontend_ssl = false;
ctx.awaiting_frontend_startup = false;
pgwire::messages::PgWireFrontendMessage::decode(&mut bytes::BytesMut::from(&bytes[..]), &ctx);
```

This panics in `bytes`' checked `get_u16`: pgwire `extendedquery::Parse::decode_body` calls it after cstring parsing on an empty body.
The decoder never checks the lower frame length/body remainder before that read. Also inspect `codec::decode_packet` and other fixed-width
body reads; patching only this reproducer is not a complete fix. No memory unsafety is claimed; the crate has zero textual unsafe blocks.
The hand decoder rejects it. **Known-bad hand mutation:** remove `Cursor::take`'s `n > remaining` guard, retaining the rest of the decoder;
it compiles, produces 471 panics in regular corpus, and the zero-panic assertion exits 101. Both the detector and its mutant are replayable.
[Corpus counters](20261107-pgwire-baseline/fuzz.txt), [mutant output](20261107-pgwire-baseline/mutant.txt).

Live length headers 3, -1 and 1,114,113 with **no body sent** close immediately in both arms, within a two-second socket deadline; RSS
remains 5,752 KiB (hand) / 5,732 KiB (pg) before/after. The common frame gate reads length before `Vec::with_capacity`/`resize`.
This tests our outer cap, not pgwire's own large-packet default (about 1 GiB). The production startup cap and connection limits are
reasoned in ADR 0008 and not implemented by this spike. [Live limits/cancel record](20261107-pgwire-baseline/limits.txt).

## (iv) Dependency policy and unsafe inventory

Candidate: `pgwire = { version = "=0.42.1", default-features = false, features = ["server-api"] }`.
No TLS, no `aws-lc-rs`. The independent spike has its own locked graph. Separately, temporarily add that **normal dependency to
`crates/nata/Cargo.toml` in the full real workspace**, preserving its existing lockfile entries; run `cargo metadata` and `cargo deny check`.
All categories pass, exit 0: advisories, bans, licenses, sources. Restore the manifest and lockfile byte-for-byte afterwards.
Also run cargo-deny against the retained spike with the unchanged root `deny.toml`: all categories pass, exit 0.
Duplicate versions are warnings under the existing policy, not silenced or changed to pass.

Known-bad policy control: add `deny = [{ name = "pgwire" }]` under `[bans]` in a temporary copy of deny.toml; checking the added metadata
exits **2**, `error[banned]: crate 'pgwire = 0.42.1' is explicitly banned`. This is a real bans failure, not a build/config failure.
The inventory is accepted under the workspace's current distinction: own Rust forbids unsafe, third-party code is inventoried, not
required to have zero unsafe. No new exception/allowance was added to deny.toml.

Inventory method: for each registry (name, version) in added-workspace metadata absent from baseline metadata, count regex
`\bunsafe\s*\{` over **every `.rs` file in that crate's source directory**, including tests, examples, all targets and comments.
This is a reproducible textual inventory of unsafe-block syntax, not macro-expanded active unsafe, unsafe functions/impls, native C/asm,
or a soundness audit. The inventory includes version refreshes, not just newly named crates; existing baseline versions are not recounted.
Tokio/mio/socket2 are explicit here because adding the server feature resolves a newer version and activates the I/O path.
Pgwire and the hand spike each have **zero** unsafe blocks; the spike has `#![forbid(unsafe_code)]` in addition to the real workspace lint.

| Crate | Added/resolved version | Unsafe blocks (textual) |
|---|---|---:|
| array-init | 2.1.0 | 4 |
| async-trait | 0.1.92 | 2 |
| base64 | 0.22.1 | 0 |
| block-buffer | 0.12.1 | 18 |
| borsh | 1.8.1 | 5 |
| byteorder | 1.5.0 | 39 |
| cfg_aliases | 0.2.2 | 0 |
| chacha20 | 0.10.2 | 23 |
| cmov | 0.5.4 | 21 |
| const-oid | 0.10.2 | 1 |
| cpufeatures | 0.3.1 | 8 |
| crypto-common | 0.2.2 | 0 |
| ctutils | 0.4.8 | 0 |
| derive-new | 0.7.0 | 0 |
| digest | 0.11.3 | 0 |
| fallible-iterator | 0.2.0 | 0 |
| futures | 0.3.34 | 0 |
| futures-channel | 0.3.34 | 11 |
| futures-executor | 0.3.34 | 4 |
| futures-io | 0.3.34 | 0 |
| futures-macro | 0.3.34 | 2 |
| futures-sink | 0.3.34 | 2 |
| hex | 0.4.3 | 0 |
| hmac | 0.13.0 | 0 |
| hybrid-array | 0.4.15 | 30 |
| itoa | 1.0.18 | 12 |
| lazy-regex | 3.6.2 | 0 |
| lazy-regex-proc_macros | 3.6.2 | 0 |
| md-5 | 0.11.0 | 1 |
| md5 | 0.8.1 | 0 |
| mio | 1.2.4 | 117 |
| pg_interval | 0.6.0 | 0 |
| pgwire | 0.42.1 | 0 |
| postgres-protocol | 0.6.12 | 0 |
| postgres-types | 0.2.14 | 0 |
| rand | 0.10.3 | 17 |
| rand_core | 0.10.1 | 0 |
| regex | 1.13.1 | 0 |
| regex-lite | 0.1.9 | 0 |
| serde | 1.0.229 | 2 |
| serde_core | 1.0.229 | 2 |
| serde_derive | 1.0.229 | 0 |
| serde_json | 1.0.154 | 17 |
| sha2 | 0.11.1 | 39 |
| smol_str | 0.3.7 | 33 |
| socket2 | 0.6.5 | 215 |
| stringprep | 0.1.5 | 0 |
| thiserror | 2.0.21 | 0 |
| thiserror-impl | 2.0.21 | 0 |
| tinyvec | 1.13.3 | 0 |
| tokio | 1.54.1 | 831 |
| tokio-macros | 2.7.2 | 0 |
| tokio-util | 0.7.20 | 27 |
| typenum | 1.20.1 | 0 |
| unicode-bidi | 0.3.18 | 1 |
| unicode-normalization | 0.1.26 | 4 |
| unicode-properties | 0.1.5 | 0 |
| zmij | 1.0.23 | 59 |

58 added/version-refreshed registry packages, 1,547 textual unsafe blocks in total, in this full-workspace metadata delta. The independent spike's fresh resolution also
refreshes unrelated existing dependencies; its additional ten inventory entries are: brotli-decompressor 6.3.0: 152; cc 1.11.1: 31;
flate2 1.1.11: 27; generator 0.8.12: 104; windows-core 0.100.0: 141; windows-implement 0.100.0: 9;
windows-interface 0.100.0: 3; windows-link 0.100.0: 0; windows-result 0.100.0: 14; windows-strings 0.100.0: 49.
These are not attributed to production pgwire requirements. The selected hand implementation still needs tokio networking but does not
need pgwire's postgres-types/crypto/type-extension graph. R13 remains the existing CLI-only signal dependency, not a server signal hook.

## Real clients, informational only

Initially no psql, java or javac. User-local only: `apt-get download` followed by `dpkg-deb -x` below `$HOME/tmp-4b/clients/root` installed
psql 17.11, libpq 17.11, OpenJDK JRE/JDK 21.0.13 and pgjdbc 42.7.7; no system package changes. Java's extracted absolute configuration
symlinks were redirected into that extraction. Psycopg binary 3.3.8 was installed in a user-local uv venv for the type probe.

| Client / operation | Hand | pgwire |
|---|---|---|
| psql 17.11 simple, `LIMIT 1` | `0`, exit 0 | same |
| psql unnamed extended, `\bind` then `\g` | `0`, exit 0 | same |
| psql cancel (SIGINT while query streaming) | ErrorResponse, `spike query failed`, exit 1 | same |
| pgjdbc 42.7.7 `preferQueryMode=simple` | correct row; `Statement.cancel()` → 57014 | same |
| pgjdbc `preferQueryMode=extended`, prepareThreshold=0, binaryTransfer=false | correct row; cancel → 57014 | same |
| pgjdbc / psycopg ordinary type fixture | all ten output columns read back | same |

The raw cancel probe additionally checks `57014`, ReadyForQuery, a successful next query in the cancelled session, and a successful
query in a second session. It does not establish race freedom or cancellation latency under CPU contention or a permanently blocked sink.
The protocol harness handles JDBC's `SET` chatter, not the full driver's bootstrap surface. `psql` cancellation's exit 1 is its error result,
not a failed detector. The positive tests assert row values; Java cancel smoke prints its SQLSTATE (any future `MISS` is a failed probe).
[Client output](20261107-pgwire-baseline/clients.txt) and ADR 0008 contain OIDs and readback types.

The fixture values are true, 127, -32768, 2147483647, 9223372036854775807, 1.25 (float32), 0.0 (float64), 123.450 DECIMAL(10,3), `a,b`,
2024-02-29. **A real failing-first observation:** initially used Nata CSV boolean text `true` under OID 16. Psycopg read True, but JDBC
`getObject` returned false. Correcting the wire form to `t` makes both read true. The corrected fixture is retained; no false value is
accepted as a pass. Extreme Date32 domains, NULL/empty-string mutants, float specials/signed-zero and all decimal boundaries are not
certified by this ten-column fixture; permanent checks/item 35 must expand it.

### Corrections during the experiment

The first transport harness incorrectly treated pgwire's virtual “no SSL negotiation” message as an actual SSL request and waited for
another startup packet. Set the decode context from the actual SSL/GSS magic before invoking the decoder; the complete probes above
were rerun. The first pgwire CancelRequest adapter matched only `SecretKey::I32`; pgwire's initial protocol context can decode a four-byte
key as `Bytes`. Normalize four-byte values (as the crate's `as_i32`/`eq_value` helpers do); both Java modes, psql and isolation were rerun.
These were spike adapter bugs, not extra R1 disqualifiers. A first cargo-deny invocation put `--config` after `check` and was rejected;
all reported pass/fail evidence comes from valid invocations with global options before the subcommand.

## Reproduction and retained code

**Kept excluded, not deleted:** `spikes/pgwire-bakeoff/{Cargo.toml,Cargo.lock,src/codec.rs,src/main.rs,*.py,*.java,README.md}`.
Reason: the parent needs the exact unchanged-crate crash, hand guard-removal mutant and live resource probes before implementing step 2.
It is a local experiment, not a `nata serve` stub or workspace member. Delete it at step 4 when permanent decoder/resource checks replace
it. No binaries, downloaded clients, Parquet data or environment metadata are committed. The README gives complete local commands;
important commands and code boundaries are also preserved here.

```sh
mkdir -p "$HOME/tmp-4b"
export TMPDIR="$HOME/tmp-4b"
# DuckDB calls always have AI_AGENT unset.
env -u AI_AGENT duckdb -no-agent -c "COPY (SELECT i::BIGINT AS v FROM range(1000000) t(i)) TO '/home/dev/tmp-4b/million.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 16384);"
cargo build --locked --manifest-path spikes/pgwire-bakeoff/Cargo.toml
timeout 20s spikes/pgwire-bakeoff/target/debug/pgwire-bakeoff fuzz
python3 spikes/pgwire-bakeoff/mutate.py
python3 spikes/pgwire-bakeoff/driver.py
python3 spikes/pgwire-bakeoff/load.py
python3 spikes/pgwire-bakeoff/limits.py
python3 spikes/pgwire-bakeoff/verify.py
cargo deny --manifest-path spikes/pgwire-bakeoff/Cargo.toml --config deny.toml check
```

The decoder's critical invariant is `Cursor::take(n)` refuses `n > remaining` before splitting; cstrings require a terminator and valid
UTF-8, counts are constrained by remaining payload, and the body must be completely consumed. The live reader checks header bounds
before allocating a body. For direct replay the minimal five-byte Parse example above needs only pgwire/bytes, not Parquet or Nata.

Step 1 does not close carried-in R13/R15/R16, change the card's part 5, provide a production codec, or satisfy the two-CPU benchmarks,
64-session scalability, sanitizers/leak certification or the full item-35 client matrix. Checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`TMPDIR=$HOME/tmp-4b cargo test --workspace`, `cargo deny check`; plus standalone spike build/fmt/Clippy (`--all-targets -- -D warnings`)/deny, decoder/mutant and
loopback resource/client probes, staged and final-tree leak scans. Full ordinary workspace checks and leak scans
are recorded at this commit; cargo-deny passes both baseline and added graphs. No performance or resource target was relaxed.
