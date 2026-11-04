"""A toy PostgreSQL wire-protocol (v3) server that serves the decimal test table from Arrow arrays.
It exists to answer: given an Arrow Decimal64/Decimal128 column, what must the server send so real Postgres clients read `numeric`?
Supports: SSLRequest refusal, startup without auth, simple Query, extended protocol (Parse/Bind/Describe/Execute/Sync/Close).
The "SQL" only picks columns: SELECT <col>[, <col>...] FROM t     (anything else: ErrorResponse).
"""
import asyncio
import struct

import pyarrow as pa

import pg_numeric as N
from common import make_table

TABLE = make_table()
OID_INT4, OID_NUMERIC = 23, 1700


def unscaled_values(arr: pa.Array):
    """Raw unscaled integers (None for NULL) straight from the Arrow data buffer: the server-edge view of Decimal64/Decimal128."""
    width = 8 if pa.types.is_decimal64(arr.type) else 16
    buf = arr.buffers()[1]
    out = []
    for i in range(len(arr)):
        if not arr[i].is_valid:
            out.append(None); continue
        raw = bytes(buf)[(arr.offset + i) * width:(arr.offset + i + 1) * width]
        out.append(int.from_bytes(raw, "little", signed=True))
    return out


def msg(tag: bytes, body: bytes = b"") -> bytes:
    return tag + struct.pack("!I", len(body) + 4) + body


def cstr(s: str) -> bytes:
    return s.encode() + b"\0"


def parse_sql(sql: str):
    s = sql.strip().rstrip(";")
    if not s.upper().startswith("SELECT ") or not s.upper().endswith(" FROM T"):
        raise ValueError(f'unsupported query "{sql}" (toy server: SELECT <cols> FROM t)')
    cols = [c.strip() for c in s[7:-7].split(",")]
    if cols == ["*"]:
        cols = TABLE.column_names
    for c in cols:
        if c not in TABLE.column_names:
            raise ValueError(f'column "{c}" does not exist')
    return cols


def row_description(cols, fmts):
    """RowDescription: per field name, table oid, attnum, type oid, typlen, typmod, format code (docs: protocol-message-formats)."""
    body = struct.pack("!h", len(cols))
    for i, c in enumerate(cols):
        t = TABLE.schema.field(c).type
        fmt = fmts[i] if len(fmts) > 1 else (fmts[0] if fmts else 0)
        if pa.types.is_decimal(t):
            body += cstr(c) + struct.pack("!IhIhih", 0, 0, OID_NUMERIC, -1, N.typmod(t.precision, t.scale), fmt)
        else:
            body += cstr(c) + struct.pack("!IhIhih", 0, 0, OID_INT4, 4, -1, fmt)
    return msg(b"T", body)


def data_rows(cols, fmts):
    data = []
    for c in cols:
        t = TABLE.schema.field(c).type
        if pa.types.is_decimal(t):
            data.append(("dec", unscaled_values(TABLE[c].combine_chunks()), t.scale))
        else:
            data.append(("int", TABLE[c].to_pylist(), 0))
    out = []
    for r in range(TABLE.num_rows):
        body = struct.pack("!h", len(cols))
        for i, (kind, vals, scale) in enumerate(data):
            fmt = fmts[i] if len(fmts) > 1 else (fmts[0] if fmts else 0)
            v = vals[r]
            if v is None:
                body += struct.pack("!i", -1); continue
            if kind == "dec":
                b = N.encode_numeric(v, scale) if fmt == 1 else N.text_numeric(v, scale).encode()
            else:
                b = struct.pack("!i", v) if fmt == 1 else str(v).encode()
            body += struct.pack("!i", len(b)) + b
        out.append(msg(b"D", body))
    return b"".join(out), TABLE.num_rows


async def serve_client(reader, writer):
    async def rd(n):
        return await reader.readexactly(n)
    try:
        while True:  # startup phase
            ln = struct.unpack("!I", await rd(4))[0]
            body = await rd(ln - 4)
            code = struct.unpack("!I", body[:4])[0]
            if code in (80877103, 80877104):  # SSLRequest / GSSENCRequest
                writer.write(b"N"); await writer.drain(); continue
            break
        out = msg(b"R", struct.pack("!I", 0))
        for k, v in (("server_version", "16.0 (toy)"), ("client_encoding", "UTF8"), ("DateStyle", "ISO, MDY"), ("integer_datetimes", "on"),
                     ("standard_conforming_strings", "on"), ("TimeZone", "UTC"), ("server_encoding", "UTF8")):
            out += msg(b"S", cstr(k) + cstr(v))
        out += msg(b"K", struct.pack("!II", 1, 2)) + msg(b"Z", b"I")
        writer.write(out); await writer.drain()
        stmts, portals, failed = {}, {}, False
        while True:
            tag = await rd(1)
            ln = struct.unpack("!I", await rd(4))[0]
            body = await rd(ln - 4)
            if tag == b"X":
                break
            if tag == b"Q":
                sql = body[:-1].decode()
                try:
                    cols = parse_sql(sql)
                    rows, n = data_rows(cols, [0])
                    writer.write(row_description(cols, [0]) + rows + msg(b"C", cstr(f"SELECT {n}")))
                except Exception as e:  # noqa: BLE001
                    writer.write(msg(b"E", b"SERROR\0C42601\0M" + str(e).encode() + b"\0\0"))
                writer.write(msg(b"Z", b"I")); await writer.drain(); continue
            if failed and tag != b"S":
                continue
            try:
                if tag == b"P":
                    name, rest = body.split(b"\0", 1)
                    sql, _ = rest.split(b"\0", 1)
                    stmts[name] = parse_sql(sql.decode()); writer.write(msg(b"1"))
                elif tag == b"B":
                    portal, rest = body.split(b"\0", 1); stmt, rest = rest.split(b"\0", 1)
                    nf = struct.unpack("!h", rest[:2])[0]; rest = rest[2 + 2 * nf:]
                    npar = struct.unpack("!h", rest[:2])[0]; assert npar == 0, "toy server takes no parameters"; rest = rest[2:]
                    nr = struct.unpack("!h", rest[:2])[0]; rfmts = list(struct.unpack(f"!{nr}h", rest[2:2 + 2 * nr]))
                    portals[portal] = (stmts[stmt], rfmts); writer.write(msg(b"2"))
                elif tag == b"D":
                    kind, name = body[:1], body[1:-1]
                    if kind == b"S":
                        writer.write(msg(b"t", struct.pack("!h", 0)) + row_description(stmts[name], [0]))
                    else:
                        cols, rfmts = portals[name]; writer.write(row_description(cols, rfmts))
                elif tag == b"E":
                    portal = body.split(b"\0", 1)[0]
                    cols, rfmts = portals[portal]; rows, n = data_rows(cols, rfmts)
                    writer.write(rows + msg(b"C", cstr(f"SELECT {n}")))
                elif tag == b"C":
                    writer.write(msg(b"3"))
                elif tag == b"S":
                    writer.write(msg(b"Z", b"I")); failed = False; await writer.drain()
                elif tag == b"H":
                    await writer.drain()
            except Exception as e:  # noqa: BLE001
                failed = True
                writer.write(msg(b"E", b"SERROR\0C42601\0M" + str(e).encode() + b"\0\0")); await writer.drain()
    except (asyncio.IncompleteReadError, ConnectionError):
        pass
    finally:
        writer.close()


async def start(port=0):
    return await asyncio.start_server(serve_client, "127.0.0.1", port)
