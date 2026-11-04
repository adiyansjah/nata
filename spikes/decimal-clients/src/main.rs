//! Minimal Flight SQL server (arrow-flight 60.1, flight-sql feature) over a fixed table. The SQL text only picks a table:
//!   SELECT * FROM t                (id Int32, d64 Decimal64(15,2), d128 Decimal128(15,2), d64_wide Decimal64(18,0))
//!   SELECT d64|d128|d64_wide FROM t
//!   SELECT * FROM t_widened        (the same, with every Decimal64 cast to Decimal128 and PRECISION/SCALE field metadata)
//!   SELECT * FROM empty
//! DoPut ingest (adbc_ingest) prints the schema and the decimal values it decoded to stdout ("INGEST ...").
//! usage: decimal-clients-flight-server PORT      (prints "READY port" when listening)
use std::collections::HashMap;
use std::pin::Pin;
use std::str::FromStr;
use bytes as _;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Decimal128Array, Decimal64Array, Int32Array, RecordBatch};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::util::display::array_value_to_string;
use arrow_flight::decode::FlightRecordBatchStream;
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::error::FlightError;
use arrow_flight::flight_service_server::FlightServiceServer;
use arrow_flight::sql::server::{FlightSqlService, PeekableFlightDataStream};
use arrow_flight::sql::{
    ActionCreatePreparedStatementRequest, ActionCreatePreparedStatementResult, ActionClosePreparedStatementRequest,
    CommandPreparedStatementQuery, CommandStatementIngest, CommandStatementQuery, ProstMessageExt, SqlInfo,
    TicketStatementQuery,
};
use arrow_flight::{Action, FlightDescriptor, FlightEndpoint, FlightInfo, Ticket};
use futures::{Stream, StreamExt, TryStreamExt, stream};
use prost::Message;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

type DataStream = Pin<Box<dyn Stream<Item = Result<arrow_flight::FlightData, Status>> + Send + 'static>>;

fn dec(v: &str) -> i64 {
    // "-9999999999999.99" -> unscaled i64 with scale 2
    let neg = v.starts_with('-');
    let v = v.trim_start_matches('-');
    let (i, f) = v.split_once('.').unwrap_or((v, ""));
    let f = format!("{:0<2}", f);
    let n: i64 = format!("{i}{f}").parse().unwrap();
    if neg { -n } else { n }
}

fn table() -> RecordBatch {
    let d: Vec<Option<i64>> = [None, Some("0.00"), Some("0.01"), Some("-0.01"), Some("1.00"), Some("10000.00"), Some("9999999999999.99"), Some("-9999999999999.99"), Some("123456789012.34")]
        .iter().map(|o| o.map(dec)).collect();
    let wide: Vec<Option<i64>> = vec![Some(999_999_999_999_999_999), None, Some(-999_999_999_999_999_999), Some(1), Some(0), Some(-1), Some(10_000), Some(100_000_000), Some(12_345_678_901_234_567)];
    let id = Int32Array::from((0..9).collect::<Vec<i32>>());
    let d64 = Decimal64Array::from(d.clone()).with_precision_and_scale(15, 2).unwrap();
    let d128 = Decimal128Array::from(d.iter().map(|x| x.map(i128::from)).collect::<Vec<_>>()).with_precision_and_scale(15, 2).unwrap();
    let w = Decimal64Array::from(wide).with_precision_and_scale(18, 0).unwrap();
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("d64", d64.data_type().clone(), true),
        Field::new("d128", d128.data_type().clone(), true),
        Field::new("d64_wide", w.data_type().clone(), true),
    ]));
    let cols: Vec<ArrayRef> = vec![Arc::new(id), Arc::new(d64), Arc::new(d128), Arc::new(w)];
    RecordBatch::try_new(schema, cols).unwrap()
}

/// The server edge: Decimal64 -> Decimal128 (same precision and scale) plus Flight SQL column metadata.
fn widen(b: &RecordBatch) -> RecordBatch {
    let mut fields = vec![];
    let mut cols = vec![];
    for (f, c) in b.schema().fields().iter().zip(b.columns()) {
        let (ty, col) = match f.data_type() {
            DataType::Decimal64(p, s) => (DataType::Decimal128(*p, *s), cast(c, &DataType::Decimal128(*p, *s)).unwrap()),
            t => (t.clone(), c.clone()),
        };
        let mut md = HashMap::new();
        if let DataType::Decimal128(p, s) = &ty {
            md.insert("ARROW:FLIGHT:SQL:PRECISION".to_string(), p.to_string());
            md.insert("ARROW:FLIGHT:SQL:SCALE".to_string(), s.to_string());
            md.insert("ARROW:FLIGHT:SQL:TYPE_NAME".to_string(), "DECIMAL".to_string());
        }
        fields.push(Field::new(f.name(), ty, f.is_nullable()).with_metadata(md));
        cols.push(col);
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).unwrap()
}

fn lookup(sql: &str) -> Result<RecordBatch, Status> {
    let t = table();
    let (name, widened) = match sql.strip_prefix("SELECT ").and_then(|r| r.split_once(" FROM ")) {
        Some((cols, "t")) => (cols, false),
        Some((cols, "t_widened")) => (cols, true),
        Some((_, "empty")) => return Ok(t.slice(0, 0)),
        _ => return Err(Status::not_found(format!("unknown query {sql:?}"))),
    };
    let b = if name == "*" { t } else {
        let i = t.schema().index_of(name).map_err(|e| Status::not_found(e.to_string()))?;
        t.project(&[i]).unwrap()
    };
    Ok(if widened { widen(&b) } else { b })
}

fn schema_bytes(s: &Schema) -> bytes::Bytes {
    let opts = arrow::ipc::writer::IpcWriteOptions::default();
    let r: arrow_flight::SchemaResult = arrow_flight::SchemaAsIpc::new(s, &opts).try_into().unwrap();
    r.schema
}

fn info_for(sql: &str, desc: FlightDescriptor) -> Result<FlightInfo, Status> {
    let b = lookup(sql)?;
    let ticket = Ticket::new(TicketStatementQuery { statement_handle: sql.as_bytes().to_vec().into() }.as_any().encode_to_vec());
    FlightInfo::new()
        .try_with_schema(b.schema().as_ref())
        .map_err(|e| Status::internal(e.to_string()))
        .map(|i| i.with_endpoint(FlightEndpoint::new().with_ticket(ticket)).with_descriptor(desc).with_total_records(b.num_rows() as i64))
}

fn encode(b: RecordBatch) -> DataStream {
    let schema: SchemaRef = b.schema();
    let s = stream::iter(vec![Ok(b)]);
    Box::pin(FlightDataEncoderBuilder::new().with_schema(schema).build(s).map_err(|e: FlightError| Status::internal(e.to_string())))
}

#[derive(Clone)]
struct Svc;

#[tonic::async_trait]
impl FlightSqlService for Svc {
    type FlightService = Svc;

    async fn get_flight_info_statement(&self, q: CommandStatementQuery, r: Request<FlightDescriptor>) -> Result<Response<FlightInfo>, Status> {
        eprintln!("GetFlightInfo CommandStatementQuery {:?}", q.query);
        Ok(Response::new(info_for(&q.query, r.into_inner())?))
    }
    async fn get_flight_info_prepared_statement(&self, q: CommandPreparedStatementQuery, r: Request<FlightDescriptor>) -> Result<Response<FlightInfo>, Status> {
        let sql = String::from_utf8(q.prepared_statement_handle.to_vec()).unwrap();
        eprintln!("GetFlightInfo CommandPreparedStatementQuery {sql:?}");
        Ok(Response::new(info_for(&sql, r.into_inner())?))
    }
    async fn do_get_statement(&self, t: TicketStatementQuery, _r: Request<Ticket>) -> Result<Response<DataStream>, Status> {
        let sql = String::from_utf8(t.statement_handle.to_vec()).unwrap();
        eprintln!("DoGet TicketStatementQuery {sql:?}");
        Ok(Response::new(encode(lookup(&sql)?)))
    }
    async fn do_get_prepared_statement(&self, q: CommandPreparedStatementQuery, _r: Request<Ticket>) -> Result<Response<DataStream>, Status> {
        let sql = String::from_utf8(q.prepared_statement_handle.to_vec()).unwrap();
        Ok(Response::new(encode(lookup(&sql)?)))
    }
    async fn do_action_create_prepared_statement(&self, q: ActionCreatePreparedStatementRequest, _r: Request<Action>) -> Result<ActionCreatePreparedStatementResult, Status> {
        eprintln!("DoAction CreatePreparedStatement {:?}", q.query);
        let b = lookup(&q.query)?;
        Ok(ActionCreatePreparedStatementResult {
            prepared_statement_handle: q.query.clone().into_bytes().into(),
            dataset_schema: schema_bytes(b.schema().as_ref()),
            parameter_schema: schema_bytes(&Schema::empty()),
        })
    }
    async fn do_action_close_prepared_statement(&self, _q: ActionClosePreparedStatementRequest, _r: Request<Action>) -> Result<(), Status> {
        Ok(())
    }
    async fn do_put_statement_ingest(&self, _c: CommandStatementIngest, r: Request<PeekableFlightDataStream>) -> Result<i64, Status> {
        let s = r.into_inner().map_err(|e| FlightError::Tonic(Box::new(e)));
        let batches: Vec<RecordBatch> = FlightRecordBatchStream::new_from_flight_data(s)
            .try_collect().await.map_err(|e| Status::invalid_argument(format!("decode failed: {e}")))?;
        let mut n = 0;
        for b in &batches {
            n += b.num_rows();
            let schema: Vec<String> = b.schema().fields().iter().map(|f| format!("{}: {}", f.name(), f.data_type())).collect();
            println!("INGEST schema=[{}] rows={}", schema.join(", "), b.num_rows());
            for (f, c) in b.schema().fields().iter().zip(b.columns()) {
                if f.name() != "id" {
                    let v: Vec<String> = (0..c.len()).map(|i| if c.is_null(i) { "NULL".into() } else { array_value_to_string(c, i).unwrap() }).collect();
                    println!("INGEST {} = {}", f.name(), v.join(" "));
                }
            }
        }
        Ok(n as i64)
    }
    async fn register_sql_info(&self, _id: i32, _r: &SqlInfo) {}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::args().nth(1).unwrap_or_else(|| "0".into());
    let l = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", u16::from_str(&port)?)).await?;
    println!("READY {}", l.local_addr()?.port());
    let _ = std::io::Write::flush(&mut std::io::stdout());
    Server::builder()
        .add_service(FlightServiceServer::new(Svc))
        .serve_with_incoming(tokio_stream_wrap(l))
        .await?;
    Ok(())
}

fn tokio_stream_wrap(l: tokio::net::TcpListener) -> impl Stream<Item = Result<tokio::net::TcpStream, std::io::Error>> {
    stream::unfold(l, |l| async move { let r = l.accept().await.map(|(s, _)| s); Some((r, l)) }).boxed()
}
