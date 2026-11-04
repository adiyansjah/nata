# Decimal64 at the client edge: Flight, Flight SQL, ADBC, JDBC, Postgres wire

Date: 2026-11-04. Question (ADR 0003, decision 3): does Arrow `Decimal64` survive the protocols a Nata server would use, and what must the server edge widen? Follows item 17 of `bench/results/20261104-decimal-verification.md` (pyarrow, pandas, polars, DuckDB: 29 of 29 probes accepted `Decimal64` in files). Code: `spikes/decimal-clients/` (throwaway; README has the reproduce commands).

## Summary

| path | client (version) | server | Decimal64 | what the client shows |
|---|---|---|---|---|
| Arrow Flight `do_get`, `do_put`, `get_schema`, `list_flights` | pyarrow.flight 26.0.1 (C++) | pyarrow.flight server | **accepted**, type kept | `decimal64(15, 2)`, `decimal64(18, 0)`; values identical |
| Flight SQL, ADBC | adbc_driver_flightsql 1.13.0 (Go driver), prepared and unprepared, `fetch_arrow_table`, `fetch_df`, `fetchall`, `adbc_ingest` | pyarrow.flight + hand-written Flight SQL | **accepted**, type kept | Arrow `decimal64`; DB-API `description` says `Decimal64Type(decimal64(15, 2))`; Python rows are `Decimal`; pandas columns are `object` holding `Decimal` |
| Flight SQL, ADBC, by hand | same driver and pyarrow.flight | **Rust `arrow-flight` 60.1.0** Flight SQL server | **accepted**, type kept | 13 of 13; the Rust decoder also accepted `adbc_ingest` of `Decimal64` |
| Flight SQL JDBC | Apache Arrow Flight SQL JDBC driver 19.0.0 (latest on Maven Central), JDK 25.0.2 | both servers | **REJECTED** | `Library only supports 128-bit and 256-bit decimal values` |
| same, after the server casts to `Decimal128` | same | both servers | accepted as `Decimal128` | `java.math.BigDecimal`, exact; precision and scale correct when the server sets the Flight SQL field metadata |
| Postgres wire, `numeric` text and binary | psycopg 3.3.7 (libpq 18.6), asyncpg 0.32.0, pg8000 1.31.6 | toy server, values taken from the Arrow `Decimal64`/`Decimal128` buffers | n/a: the wire type is `numeric`, whatever the Arrow width | 4 of 4, exact `Decimal` |

**Only the Java Arrow library rejects `Decimal64`, and it rejects the whole result set, not just the column.** Everything else tried accepts it. No client converted a `Decimal64` to float or string. The server edge must widen to `Decimal128` for Flight SQL (JDBC); the Postgres edge does not depend on the Arrow width.

Probe counts (controls excluded): Flight + ADBC against the pyarrow server 29 accepted, 0 rejected; Rust server 13 accepted, 0 rejected; JDBC 11 probes per server: 6 accepted, 5 rejected, identical on both servers; Postgres clients 4 of 4. Controls: 6 (below).

## Method and versions

- Machine: Intel Core i3-1220P, x86-64, Debian 13 (Linux 6.12). **Everything ran on x86-64 only; ARM64 was not run** (no cloud spend).
- Table (9 rows): `id` Int32; `d64` Decimal64(15,2) = NULL, 0.00, 0.01, -0.01, 1.00, 10000.00, 9999999999999.99, -9999999999999.99, 123456789012.34; `d128` Decimal128(15,2) the same values; `d64_wide` Decimal64(18,0) = 999999999999999999, NULL, -999999999999999999, 1, 0, -1, 10000, 100000000, 12345678901234567.
- Versions: pyarrow 26.0.1, pandas 3.1.0, adbc_driver_manager and adbc_driver_flightsql 1.13.0, Rust arrow 60.1.0 and arrow-flight 60.1.0 (feature `flight-sql`, tonic 0.14.9), Flight SQL JDBC driver 19.0.0 (sha1 checked against Maven Central), JPype1 1.7.1, JRE 25.0.2 from the pip wheel `jdk4py` 25.0.2.1. **`java` is not installed on this machine** (`java: command not found`); I did not use sudo or system packages. The JRE inside the pip wheel has `java.sql` and `libjvm`, no `javac`, so the driver is driven from Python with JPype (`DriverManager.getConnection`, `executeQuery`, `ResultSetMetaData`, `getObject`, `getBigDecimal`, `getString`, `getDouble`).
- Postgres: real PostgreSQL 16.2 from the `pgserver` wheel (no `psql`/`postgres` on the machine), used through libpq/psycopg.
- Servers: (1) pyarrow.flight server in-process (plain Flight) plus a hand-written Flight SQL layer (protobuf by hand; the SQL text only picks a table); (2) a separate Rust server on `arrow-flight` 60.1.0, the library Nata would use, started as a binary; JDBC runs against both. Neither is a SQL engine.
- Every probe compares the schema string and the values read back with the input (`values_equal_expected`). Controls that must fail: a table with one value changed (reported `False`), an unknown ticket and an unknown query (exact error recorded), and, for Postgres, a mutated encoder (reported `False`, client silently showed `Decimal('1')` for 0.01).

## Probe matrix with exact errors

### Arrow Flight (pyarrow server and client)

| probe | result |
|---|---|
| `get_flight_info(path)`, `get_schema(path)`, `list_flights` | accepted: `id: int32, d64: decimal64(15, 2), d128: decimal128(15, 2), d64_wide: decimal64(18, 0)` |
| `do_get` with `RecordBatchStream` (whole table) | accepted, types kept, values identical |
| `do_get` with `GeneratorStream` (batches of 4, 4, 1 rows via `read_chunk`) | accepted, values identical |
| `do_get` d64 only; d64_wide only | accepted |
| `read_pandas` | accepted: dtype `object` holding `decimal.Decimal` for all three decimal columns (no float) |
| `do_put` all columns / d64 only (server `read_all`) | accepted: server saw `decimal64(15, 2)`, `decimal128(15, 2)`, `decimal64(18, 0)`, values identical |

### Flight SQL via ADBC (Go driver 1.13.0), pyarrow server and Rust server

| probe | pyarrow server | Rust server |
|---|---|---|
| `cursor.execute` + `fetch_arrow_table` (DB-API; the driver prepares first): all columns, d64, d128, d64_wide | accepted, types kept, values identical | accepted (all, d64, d64_wide) |
| low-level `execute_query` without prepare (wire: `CommandStatementQuery`) | accepted | accepted |
| pyarrow.flight by hand: `GetFlightInfo(CommandStatementQuery)` + `DoGet(TicketStatementQuery)` | accepted | accepted (incl. `read_pandas`: `object`) |
| `fetch_record_batch` (stream), empty result keeps schema, `adbc_execute_schema`, `cursor.description` | accepted: `Decimal64Type(decimal64(15, 2))` | accepted (empty result: schema kept) |
| `fetch_df` (pandas), `fetchall` | accepted: `object` / Python `Decimal`, exact | not run |
| `adbc_ingest` of d64, d64_wide, d128, all columns (client to server) | accepted: server saw the same Arrow types, values identical | accepted: the Rust decoder printed `Decimal64(15, 2)` and `Decimal64(18, 0)` with all 9 values exact |

The driver requests `GetSqlInfo` on the unprepared path. The pyarrow server answered with an error (visible in its command log) and the driver carried on; the Rust server does not implement it either (the crate's default is `unimplemented`, reasoned, not logged) and the same probe passed.

### Flight SQL JDBC driver 19.0.0 (JDK 25.0.2), identical on both servers

| probe | result |
|---|---|
| connect, `getDriverName/Version` | ok: `Arrow Flight SQL JDBC Driver Core`, `19.0.0` |
| control: `SELECT d128` (Decimal128(15,2)), no field metadata | accepted; `java.math.BigDecimal` 0.00 ... 9999999999999.99, `getBigDecimal` scale 2; `getPrecision`/`getScale` report **0/0** because the server gave none |
| `SELECT d64` (Decimal64(15,2)) | **rejected**: `java.sql.SQLException: Error while executing SQL "SELECT d64 FROM t": Library only supports 128-bit and 256-bit decimal values`, caused by `java.lang.IllegalArgumentException: Library only supports 128-bit and 256-bit decimal values` |
| `SELECT d64_wide` (Decimal64(18,0)) | rejected, same text |
| `SELECT *` (one Decimal64 column among others) | rejected, same text: the whole result set fails, the Decimal128 and Int32 columns are not returned |
| `SELECT * FROM empty` (schema only, zero rows) | rejected, same text: it fails on the schema, not on data |
| `PreparedStatement` for `SELECT d64` | rejected: `java.sql.SQLException: while preparing SQL: SELECT d64 FROM t`, same cause |
| widened by the server: `SELECT d64` as Decimal128(15,2) with `ARROW:FLIGHT:SQL:PRECISION/SCALE/TYPE_NAME` field metadata | accepted: `DECIMAL`, precision 15, scale 2, `getBigDecimal` exact with scale 2 (`0.01`, `10000.00`, `-9999999999999.99`), NULL kept; `getDouble` also works |
| widened: `SELECT d64_wide` as Decimal128(18,0) | accepted: precision 18, scale 0; `getBigDecimal` 999999999999999999 exact; `getDouble` gives `1e+18` (the client's own conversion loses the last digits) |
| widened: `SELECT *`; widened as `PreparedStatement` | accepted |

Where it fails (root-cause frames): `ArrowType.getTypeForField(ArrowType.java:1719)` <- `Field.convertField` <- `Schema.convertSchema` <- `MessageSerializer.deserializeSchema`, in the driver's shaded Arrow Java 19.0.0. Arrow Java's `arrow-vector` latest on Maven Central is also 19.0.0 (checked 2026-11-04). It is the schema deserialiser, so any Java program on this `arrow-vector` that receives a schema with `Decimal64` (a Java Flight client, Spark connectors) is expected to fail the same way. **That is reasoned from the stack trace; no other Java client was run.**

JDK 25 note (not related to the rejection, the Decimal128 control passes with the same flags): the driver's shaded Netty failed to initialise (`ExceptionInInitializerError`, root cause `UnsupportedOperationException` in `EmptyByteBuf.memoryAddress`) until the JVM was given `--add-opens=java.base/java.nio=ALL-UNNAMED --sun-misc-unsafe-memory-access=allow`; `-Dio.netty.tryReflectionSetAccessible=true` (plain and with the shaded prefix) did not help. A JDBC user on JDK 24+ needs those flags regardless of Nata.

### Controls (the checks can fail)

| control | result |
|---|---|
| Flight `do_get` of a table with `d64[2]` changed from 0.01 to 0.02 | recorded `values_equal_expected: False` |
| Flight `do_get` unknown ticket | `ArrowInvalid: unsupported wire type 6. Detail: Python exception: ... ValueError: unsupported wire type 6 ... INVALID_ARGUMENT` (server-side parse of my test ticket) |
| ADBC unknown query | `ProgrammingError: NOT_FOUND: [FlightSQL] 'SELECT 1'. Detail: Python exception: ... KeyError: 'SELECT 1' ... (NotFound; Prepare)` |
| psycopg unknown column | `SyntaxError: column "nope" does not exist` |
| psycopg binary with a mutated encoder (dscale 0 for scale-2 values) | `values_equal_expected: False`; the client returned `Decimal('1')` for 0.01 with no error |
| JDBC Decimal128 control next to the Decimal64 rejection | accepted (shows the rejection is the type, not the setup) |

## Postgres wire protocol: sources

PostgreSQL documentation (version 16 pages, fetched 2026-11-04):

- Formats: <https://www.postgresql.org/docs/16/protocol-overview.html>, "Formats and Format Codes": "Text has format code zero, binary has format code one"; "The text representation of values is whatever strings are produced and accepted by the input/output conversion functions for the particular data type"; "For other data types consult the documentation or source code to learn about the binary representation. Keep in mind that binary representations for complex data types might change across server versions; the text format is usually the more portable choice." So the binary `numeric` layout is **not** in the documentation; it is in the source.
- RowDescription and DataRow: <https://www.postgresql.org/docs/16/protocol-message-formats.html>: per field "The object ID of the field's data type", "The type modifier (see pg_attribute.atttypmod). The meaning of the modifier is type-specific", "The format code being used for the field. Currently will be zero (text) or one (binary)"; DataRow: "The length of the column value, in bytes ... -1 indicates a NULL column value".
- Type: <https://www.postgresql.org/docs/16/datatype-numeric.html>: `numeric` "up to 131072 digits before the decimal point; up to 16383 digits after the decimal point"; "If the scale of a value to be stored is greater than the declared scale of the column, the system will round the value to the specified number of fractional digits." (so a column `numeric(15,2)` shows exactly 2 decimals in text, which matches an Arrow scale of 2).
- Binary layout and typmod: `src/backend/utils/adt/numeric.c`, branch REL_16_STABLE, <https://raw.githubusercontent.com/postgres/postgres/REL_16_STABLE/src/backend/utils/adt/numeric.c> (fetched 2026-11-04; the tested server is 16.2, the file is the branch head): `NBASE 10000`, `DEC_DIGITS 4`; `numeric_send` writes `int16 ndigits, int16 weight, int16 sign, int16 dscale` then `ndigits` int16 digits; signs `NUMERIC_POS 0x0000`, `NUMERIC_NEG 0x4000`, `NUMERIC_NAN 0xC000`, `NUMERIC_PINF 0xD000`, `NUMERIC_NINF 0xF000`; `numeric_recv` rejects a bad sign, a `dscale` over the mask and a digit outside 0..9999, accepts any int16 weight, and truncates digits hidden by `dscale`; `make_numeric_typmod` is `((precision << 16) | (scale & 0x7ff)) + VARHDRSZ`; the value is `sum digits[i] * NBASE^(weight - i)`.

### What the server must do for an Arrow `Decimal64(p,s)` or `Decimal128(p,s)`

Reasoned from the sources above, then checked below. The wire type is `numeric` (OID 1700) for both widths, so the Arrow width does not matter at this edge: the server reads the unscaled integer (an `i64` or `i128`) and the column scale.

1. RowDescription: type OID 1700, typlen -1, typmod `((p << 16) | s) + 4`, format code from Bind.
2. Text result: the unscaled integer written with exactly `s` decimals (`10000.00`, `0.01`, `-0.01`; zero at scale 2 is `0.00`); no exponent.
3. Binary result: sign, `dscale = s`, and the digits in groups of four decimal digits aligned at the decimal point (integer part from the right, fraction from the left), no leading or trailing zero groups, `weight` = power of 10000 of the first group (negative below 1), zero is `ndigits = 0, weight = 0`.
4. NULL: length -1 in DataRow.
5. Nothing needs more than 38 digits (ADR 0003 limit), so at most 11 base-10000 groups in total (for example scale 1 with 37 integer digits).

### The encoder (Python reference, `spikes/decimal-clients/pg_numeric.py`)

```python
def encode_numeric(unscaled: int, scale: int) -> bytes:
    """Binary `numeric` for value = unscaled / 10**scale, scale >= 0, dscale = scale."""
    assert scale >= 0
    sign = NEG if unscaled < 0 else POS
    n = abs(unscaled)
    if n == 0:
        return struct.pack("!hhHh", 0, 0, POS, scale)
    s = str(n).rjust(scale + 1, "0")            # at least one integer digit
    ip, fp = s[: len(s) - scale], s[len(s) - scale:]
    ip = ip.rjust(-(-len(ip) // 4) * 4, "0")    # integer part: groups of 4 aligned at the decimal point (from the right)
    fp = fp.ljust(-(-len(fp) // 4) * 4, "0")    # fractional part: groups of 4 aligned at the decimal point (from the left)
    groups = [int(ip[i:i + 4]) for i in range(0, len(ip), 4)] + [int(fp[i:i + 4]) for i in range(0, len(fp), 4)]
    weight = len(ip) // 4 - 1
    while groups and groups[0] == 0:            # PostgreSQL never stores leading zero digits
        groups.pop(0)
        weight -= 1
    while groups and groups[-1] == 0:           # ... nor trailing zero digits (dscale carries them)
        groups.pop()
    return struct.pack("!hhHh", len(groups), weight, sign, scale) + struct.pack(f"!{len(groups)}h", *groups)
```

### Checks against real PostgreSQL 16.2 (`verify_numeric.py`, output in full below)

```
server: PostgreSQL 16.2 on x86_64-pc-linux-gnu, compiled by gcc (GCC) 10.2.1 20210130 (Red Hat 10.2.1-11), 64-bit
boundary cases: 54/54 identical to PostgreSQL's numeric_send
fuzz 20000 random values (38 digits, scale 0..38, runs of 0/9): 20000/20000 identical to PostgreSQL's numeric_send
numeric_recv round trip (our bytes -> PostgreSQL -> text): 5054/5054 equal to our text
layout decoder vs PostgreSQL bytes (value and dscale): 2054/2054
RowDescription typmod for numeric(15,2): PostgreSQL 983046, ours 983046 OK
RowDescription typmod for numeric(18,0): PostgreSQL 1179652, ours 1179652 OK
RowDescription typmod for numeric(18,18): PostgreSQL 1179670, ours 1179670 OK
RowDescription typmod for numeric(38,0): PostgreSQL 2490372, ours 2490372 OK
RowDescription typmod for numeric(38,38): PostgreSQL 2490410, ours 2490410 OK
RowDescription typmod for numeric(1,0): PostgreSQL 65540, ours 65540 OK
numeric(15,2) column, 9 rows incl. NULL, binary result: 9/9 identical
mutant 'weight not decremented for values < 1': caught (1007 of 2054 cases differ from PostgreSQL)
mutant 'extra trailing zero digit kept': caught (2032 of 2054 cases differ from PostgreSQL)
mutant 'fraction groups aligned right': caught (1447 of 2054 cases differ from PostgreSQL)
mutant 'dscale derived from digits (drops trailing zeros)': caught (1512 of 2054 cases differ from PostgreSQL)
mutant 'negative sign 0x8000': caught (1020 of 2054 cases differ from PostgreSQL)
mutant 'zero with weight -1': caught (22 of 2054 cases differ from PostgreSQL)
RESULT: all checks passed, all mutants caught
```

- Boundary cases (54): 0, 1 and -1 unit at scales 0, 2, 15, 18, 38; 0.01, -0.01, 1.00, 10000.00, 10000, 9999, 10001, 10^8, 10^12, 0.0001, 0.00001, 0.00010001, 5.0000 (a trailing zero group), 18-digit and 38-digit maxima and minima, scale = precision at 18 and 38, i64 min/max, and the `Decimal(15,2)` / `Decimal64(18,0)` values of the test table.
- 20,000 random values (1..38 digits, scale 0..38, runs of 0 and 9 to hit group boundaries): all byte-identical to what PostgreSQL's `numeric_send` returns for the same value.
- The other direction: PostgreSQL's `numeric_recv` accepted our bytes as a binary parameter and printed the same text for 5,054 values (this also checks our text form, including `dscale`).
- A `numeric(15,2)` column read back in binary matches for NULL, 0.00, 0.01, -0.01, 1.00, 10000.00 and the 13-digit extremes.
- The typmod matched PostgreSQL's for numeric(15,2), (18,0), (18,18), (38,0), (38,38), (1,0).
- **The check can fail:** six mutants of the encoder (weight not decremented below 1, an extra trailing zero group, fraction groups aligned right, `dscale` derived from the digits, wrong negative sign, zero with weight -1) were each caught (22 to 2,032 of 2,054 cases differ).
- A bug found on the way: my first independent decoder used Python's default 28-digit `Decimal` context and failed on 38-digit values; that was a test bug, not an encoder bug, and the decoder now uses integer arithmetic (2,054 of 2,054 PostgreSQL outputs decoded to the right value and `dscale`).

### End to end through real Postgres clients (`probe_pgwire.py`)

`pgwire_server.py` is a toy v3 server (startup, simple and extended query, no auth) that takes the unscaled integers from the Arrow data buffers of the `Decimal64`/`Decimal128` arrays and sends them with the encoder above.

| client | result format | result |
|---|---|---|
| psycopg 3.3.7 (libpq 18.6) | text | accepted: `cursor.description` gives type OID 1700, precision 15/15/18 and scale 2/2/0 from the typmod; Python `Decimal`; values identical |
| psycopg 3.3.7 | binary | same |
| asyncpg 0.32.0 (always binary) | binary | accepted: type `numeric`, values identical |
| pg8000 1.31.6 (pure Python) | text | accepted, values identical |

## Not verified, and why

- Arrow Java clients other than the Flight SQL JDBC driver (arrow-flight-core, Spark, Dremio-style tools): not run. The rejection is in `Schema.convertSchema`, so it is expected, not measured.
- Other JDBC driver versions: only 19.0.0, the newest on Maven Central on 2026-11-04. Older drivers were not tried.
- Flight SQL ODBC driver, Tableau/DBeaver/other BI tools, C#, Go Arrow Flight client directly (the ADBC driver is Go, so Go Arrow was exercised indirectly), Rust Flight client. TLS and authentication (handshake) were not used. Flight SQL metadata calls (`GetTables`, `GetXdbcTypeInfo`, `GetSqlInfo` replies) were not implemented by either server.
- Postgres: pgjdbc, pgx, node-postgres, ODBC, `psql`, ADBC's Postgres driver, `COPY ... BINARY`, and parameters sent by the client (the decode direction was checked only against PostgreSQL's own output, not through a client sending parameters). `NaN` and infinity are out of scope for decimals. The Python encoder is a reference: a Rust version and its speed were not written or measured. The toy server is not PostgreSQL: clients that run catalog queries at connect (asyncpg for unknown OIDs, many GUI tools) were not exercised.
- x86-64 only; ARM64 not run. The encodings are fixed big-endian, so no difference is expected, which is reasoned.
- The widening cost (`i64` to `i128` copy per batch, then bigger IPC messages) was not measured.

## Proposed changes to ADR 0003 (for a human to accept; the ADR is not edited here)

1. Decision 3, last sentence: replace "Results are widened at the client edge where a protocol needs it" with the measured rule: **Flight SQL results are widened to `Decimal128` with the same precision and scale (the schema in `FlightInfo` and in the `DoGet` stream must both be widened), because the Arrow Java library, hence the Flight SQL JDBC driver 19.0.0, rejects `Decimal64` with "Library only supports 128-bit and 256-bit decimal values", for the whole result set. Postgres wire sends `numeric` for both widths.**
2. Evidence, "Reasoned" list: remove "(Postgres wire `NUMERIC` and Arrow Flight SQL client support for `Decimal64` was not checked)"; add a "Measured" bullet pointing at this file: Flight, Flight SQL (ADBC, pyarrow, Rust `arrow-flight` 60.1.0 server) accept `Decimal64`; JDBC rejects it; widened `Decimal128` works with exact `BigDecimal`.
3. Confidence line for `Decimal64` at the edge: split by protocol. Arrow-native clients (pyarrow, pandas, polars, DuckDB, ADBC Go driver, Rust `arrow-flight`): **medium to high**: every probe passed, but on x86-64 only, one 9-row table, the listed versions, and no Rust Flight client was run. Java/JDBC: **known rejection**, mitigated by the widening rule. Postgres wire: **high** for the encoding (byte-identical to PostgreSQL 16.2 on 20,054 values, six mutants caught), **medium** for client coverage (psycopg, asyncpg, pg8000 only).
4. Not covered: replace "Flight SQL, ADBC, JDBC" with the list in "Not verified" above (other Java clients, ODBC, pgjdbc/pgx, ARM64, widening cost).
5. Flight SQL servers should set `ARROW:FLIGHT:SQL:PRECISION`, `SCALE` and `TYPE_NAME` on decimal fields: without them JDBC reported precision/scale 0/0 for a correct `Decimal128` column (measured).
6. Consequences: add "the Flight SQL edge needs a cast kernel `Decimal64 -> Decimal128` for result batches and a schema rewrite; the Postgres edge needs the `numeric` encoder (text and binary), tested against a real PostgreSQL with the mutants above" and an open check "measure the widening cost per batch".
7. Revisit when: Arrow Java releases a version with `Decimal64` support (re-run `probe_jdbc.py` against the new driver; a pass would allow dropping the widening for JDBC), or when a client is found that rejects `Decimal128(p,s)` with `p <= 18`.
8. Open check to add to the ADR: keep `probe_jdbc.py` and `verify_numeric.py` (or Rust ports) as the regression tests for the edge once the server exists; today they run against toy servers.

## Reproduce

See `spikes/decimal-clients/README.md`. In short: `.venv/bin/python probe_flight.py`, `probe_rust.py`, `probe_jdbc.py` (and `--rust`), `probe_pgwire.py`, and `.venv-pg/bin/python verify_numeric.py`.
