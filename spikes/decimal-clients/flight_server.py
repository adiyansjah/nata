"""A minimal Arrow Flight server (pyarrow.flight) that serves the decimal test table.

Plain Flight: do_get(ticket=b"table"/b"d64"/...), do_put(path descriptor), get_schema.
Flight SQL (hand-rolled protobuf, just enough for ADBC and the JDBC driver to run a query):
  CommandStatementQuery, CommandPreparedStatementQuery, CreatePreparedStatement action,
  CommandStatementIngest (do_put). This is NOT a real SQL engine: the query text only picks a table.

Run stand-alone:  python flight_server.py PORT   (prints "READY" when listening)
"""
import sys
import threading

import pyarrow as pa
import pyarrow.flight as fl

from common import make_table

TYPE_PREFIX = "type.googleapis.com/arrow.flight.protocol.sql."


# ---- tiny protobuf helpers (varint + length-delimited only)
def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def f_len(field, data):
    return varint(field << 3 | 2) + varint(len(data)) + data


def f_varint(field, n):
    return varint(field << 3 | 0) + varint(n)


def parse(buf):
    """-> dict field -> list of (wire, value)"""
    i, out = 0, {}
    while i < len(buf):
        key = shift = 0
        while True:
            b = buf[i]; i += 1
            key |= (b & 0x7F) << shift; shift += 7
            if not b & 0x80:
                break
        field, wire = key >> 3, key & 7
        if wire == 0:
            v = shift = 0
            while True:
                b = buf[i]; i += 1
                v |= (b & 0x7F) << shift; shift += 7
                if not b & 0x80:
                    break
        elif wire == 2:
            ln = shift = 0
            while True:
                b = buf[i]; i += 1
                ln |= (b & 0x7F) << shift; shift += 7
                if not b & 0x80:
                    break
            v = bytes(buf[i:i + ln]); i += ln
        else:
            raise ValueError(f"unsupported wire type {wire}")
        out.setdefault(field, []).append((wire, v))
    return out


def any_pack(name, value):
    return f_len(1, (TYPE_PREFIX + name).encode()) + f_len(2, value)


def any_unpack(buf):
    m = parse(buf)
    url = m[1][0][1].decode()
    return url.rsplit(".", 1)[-1], (m.get(2) or [(2, b"")])[0][1]


def widen(t):
    """What the server edge must do for clients that lack Decimal64: cast every decimal64 column to decimal128, same precision and scale.
    Adds the Flight SQL column metadata keys (PRECISION, SCALE, TYPE_NAME) that JDBC reads for ResultSetMetaData."""
    cols, fields = [], []
    for f in t.schema:
        c = t[f.name]
        if pa.types.is_decimal64(f.type):
            c = c.cast(pa.decimal128(f.type.precision, f.type.scale))
        ty = c.type
        md = {}
        if pa.types.is_decimal(ty):
            md = {"ARROW:FLIGHT:SQL:PRECISION": str(ty.precision), "ARROW:FLIGHT:SQL:SCALE": str(ty.scale), "ARROW:FLIGHT:SQL:TYPE_NAME": "DECIMAL"}
        fields.append(pa.field(f.name, ty, metadata=md))
        cols.append(c)
    return pa.Table.from_arrays(cols, schema=pa.schema(fields))


def tables():
    t = make_table()
    return {
        "SELECT * FROM t_widened": widen(t),
        "SELECT d64 FROM t_widened": widen(t).select(["d64"]),
        "SELECT d64_wide FROM t_widened": widen(t).select(["d64_wide"]),
        "SELECT * FROM t": t,
        "SELECT d64 FROM t": t.select(["d64"]),
        "SELECT d128 FROM t": t.select(["d128"]),
        "SELECT d64_wide FROM t": t.select(["d64_wide"]),
        "SELECT * FROM empty": t.slice(0, 0),
    }


class Server(fl.FlightServerBase):
    def __init__(self, location="grpc://127.0.0.1:0"):
        super().__init__(location)
        self.t = tables()
        self.seen_commands = []  # Flight SQL message names seen in GetFlightInfo / DoAction
        self.put_log = []   # (descriptor, schema string, rows, to_pylist of d64)
        self.table_for_plain = {b"table": self.t["SELECT * FROM t"]}
        # negative control: same table with d64[2] changed from 0.01 to 0.02
        d = self.t["SELECT * FROM t"]
        bad = d.column("d64").to_pylist(); bad[2] = type(bad[2])("0.02")
        self.table_for_plain[b"mutated"] = d.set_column(1, "d64", pa.array(bad, d.schema.field("d64").type))
        for k in ("d64", "d128", "d64_wide"):
            self.table_for_plain[k.encode()] = self.t[f"SELECT {k} FROM t"]

    # ---------------- plain Flight
    def list_flights(self, context, criteria):
        for name, tb in self.table_for_plain.items():
            d = fl.FlightDescriptor.for_path(name.decode())
            yield fl.FlightInfo(tb.schema, d, [fl.FlightEndpoint(fl.Ticket(name), [])], tb.num_rows, -1)

    def _plain_info(self, descriptor):
        name = descriptor.path[0]
        tb = self.table_for_plain[name if isinstance(name, bytes) else name.encode()]
        return fl.FlightInfo(tb.schema, descriptor, [fl.FlightEndpoint(fl.Ticket(name if isinstance(name, bytes) else name.encode()), [])], tb.num_rows, -1)

    def get_flight_info(self, context, descriptor):
        if descriptor.descriptor_type == fl.DescriptorType.PATH:
            return self._plain_info(descriptor)
        kind, val = any_unpack(descriptor.command)
        self.seen_commands.append("GetFlightInfo:" + kind)
        if kind == "CommandStatementQuery":
            sql = parse(val)[1][0][1].decode()
            if sql not in self.t:
                raise fl.FlightServerError(f"unknown query {sql!r}")
            handle = sql.encode()
        elif kind == "CommandPreparedStatementQuery":
            handle = parse(val)[1][0][1]
        else:
            raise fl.FlightServerError(f"unsupported Flight SQL command {kind}")
        tb = self.t[handle.decode()]
        ticket = fl.Ticket(any_pack("TicketStatementQuery", f_len(1, handle)))
        return fl.FlightInfo(tb.schema, descriptor, [fl.FlightEndpoint(ticket, [])], tb.num_rows, -1)

    def get_schema(self, context, descriptor):
        return fl.SchemaResult(self.get_flight_info(context, descriptor).schema)

    def do_get(self, context, ticket):
        raw = ticket.ticket
        if raw in self.table_for_plain:
            return fl.RecordBatchStream(self.table_for_plain[raw])
        if raw == b"generator":  # same data through GeneratorStream (batches written one by one)
            tb = self.table_for_plain[b"table"]
            return fl.GeneratorStream(tb.schema, iter(tb.to_batches(max_chunksize=4)))
        kind, val = any_unpack(raw)
        assert kind == "TicketStatementQuery", kind
        handle = parse(val)[1][0][1].decode()
        return fl.RecordBatchStream(self.t[handle])

    def do_put(self, context, descriptor, reader, writer):
        tb = reader.read_all()
        self.put_log.append((descriptor, str(tb.schema).replace("\n", "; "), tb.num_rows, tb.to_pydict()))
        if descriptor.descriptor_type == fl.DescriptorType.CMD:
            kind, _ = any_unpack(descriptor.command)
            if kind == "CommandStatementIngest":
                writer.write(pa.py_buffer(f_varint(1, tb.num_rows)))  # DoPutUpdateResult.record_count

    def do_action(self, context, action):
        if action.type == "CreatePreparedStatement":
            kind, val = any_unpack(action.body.to_pybytes())
            self.seen_commands.append("DoAction:CreatePreparedStatement")
            sql = parse(val)[1][0][1]
            tb = self.t[sql.decode()]
            res = f_len(1, sql) + f_len(2, tb.schema.serialize().to_pybytes()) + f_len(3, pa.schema([]).serialize().to_pybytes())
            yield fl.Result(pa.py_buffer(any_pack("ActionCreatePreparedStatementResult", res)))
        elif action.type == "ClosePreparedStatement":
            return
        else:
            raise fl.FlightServerError(f"unknown action {action.type}")

    def list_actions(self, context):
        return [("CreatePreparedStatement", "Flight SQL"), ("ClosePreparedStatement", "Flight SQL")]


def start(port=0):
    s = Server(f"grpc://127.0.0.1:{port}")
    threading.Thread(target=s.serve, daemon=True).start()
    return s


if __name__ == "__main__":
    s = Server(f"grpc://127.0.0.1:{sys.argv[1]}")
    print("READY", s.port, flush=True)
    s.serve()
