//! One bounded partition frame, IPC or a throwaway explicit-record CONTROL.
//! No custom codec is proposed for the product. Fixed little-endian envelope:
//! magic/version+format, partition/depth, rows, payload length, CRC(header+payload).
use arrow::{
    array::*,
    datatypes::*,
    ipc::{reader::StreamReader, writer::StreamWriter},
    record_batch::RecordBatch,
};
use std::{
    fs::{File, OpenOptions},
    io::{Cursor, Read, Write},
    path::Path,
    sync::Arc,
};

pub type R<T> = Result<T, Box<dyn std::error::Error>>;
pub const MAX_ROWS: usize = 1024;
pub const MAX_PAYLOAD: usize = 1 << 20;
pub const HEADER: usize = 32;
const MAX_META: usize = 65536;
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub text: Option<String>,
    pub decimal: Option<i128>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub key: Key,
    pub hash: u64,
    pub count: u64,
    pub nonnull: u64,
    pub sum: Option<i128>,
    pub avg_sum: Option<i128>,
    pub avg_count: u64,
    pub min: Option<i128>,
    pub max: Option<i128>,
}
#[derive(Clone, Copy, Debug)]
pub enum Format {
    Ipc = 1,
    RecordControl = 2,
}
fn bad<T>() -> R<T> {
    Err("bounded spill rejection".into())
}
fn schema() -> Schema {
    Schema::new(vec![
        Field::new_dictionary("key", DataType::Int32, DataType::Utf8, true),
        Field::new("decimal_key", DataType::Decimal128(38, 4), true),
        Field::new("hash", DataType::UInt64, false),
        Field::new("count", DataType::UInt64, false),
        Field::new("nonnull", DataType::UInt64, false),
        Field::new("sum", DataType::Decimal128(38, 4), true),
        Field::new("avg_sum", DataType::Decimal128(38, 4), true),
        Field::new("avg_count", DataType::UInt64, false),
        Field::new("min", DataType::Decimal128(38, 4), true),
        Field::new("max", DataType::Decimal128(38, 4), true),
    ])
}
fn batch(states: &[State]) -> R<RecordBatch> {
    let mut dict = StringDictionaryBuilder::<Int32Type>::new();
    for s in states {
        dict.append_option(s.key.text.as_deref());
    }
    let dec = |v: Vec<Option<i128>>| -> R<ArrayRef> {
        Ok(Arc::new(
            Decimal128Array::from(v).with_precision_and_scale(38, 4)?,
        ))
    };
    Ok(RecordBatch::try_new(
        Arc::new(schema()),
        vec![
            Arc::new(dict.finish()),
            dec(states.iter().map(|s| s.key.decimal).collect())?,
            Arc::new(UInt64Array::from_iter_values(states.iter().map(|s| s.hash))),
            Arc::new(UInt64Array::from_iter_values(
                states.iter().map(|s| s.count),
            )),
            Arc::new(UInt64Array::from_iter_values(
                states.iter().map(|s| s.nonnull),
            )),
            dec(states.iter().map(|s| s.sum).collect())?,
            dec(states.iter().map(|s| s.avg_sum).collect())?,
            Arc::new(UInt64Array::from_iter_values(
                states.iter().map(|s| s.avg_count),
            )),
            dec(states.iter().map(|s| s.min).collect())?,
            dec(states.iter().map(|s| s.max).collect())?,
        ],
    )?)
}
fn ipc_encode(states: &[State]) -> R<Vec<u8>> {
    let mut w = StreamWriter::try_new(Vec::new(), &schema())?;
    w.write(&batch(states)?)?;
    w.finish()?;
    Ok(w.into_inner()?)
}
fn take<'a>(bytes: &mut &'a [u8], n: usize) -> R<&'a [u8]> {
    if n > bytes.len() {
        return bad();
    }
    let (a, b) = bytes.split_at(n);
    *bytes = b;
    Ok(a)
}
fn u64_at(b: &[u8]) -> u64 {
    u64::from_le_bytes(b.try_into().unwrap())
}
fn u32_at(b: &[u8]) -> u32 {
    u32::from_le_bytes(b.try_into().unwrap())
}

/// Bounded Flatbuffer verification followed by fixed-schema shape checks.
/// This runs BEFORE StreamReader, including its schema/dictionary decoding.
fn preflight(payload: &[u8], rows: usize) -> R<()> {
    use arrow::ipc::{MessageHeader, root_as_message};
    let trusted = ipc_encode(&[])?;
    let trusted_schema_len = u32_at(&trusted[4..8]) as usize;
    let trusted_schema = &trusted[8..8 + trusted_schema_len];
    let mut p = payload;
    let (mut messages, mut dictionaries, mut batches) = (0, 0, 0);
    let mut dictionary_len = 0;
    loop {
        if take(&mut p, 4)? != [255; 4] {
            return bad();
        }
        let n = u32_at(take(&mut p, 4)?) as usize;
        if n == 0 {
            if !p.is_empty() || batches != 1 {
                return bad();
            }
            break;
        }
        if n > MAX_META {
            return bad();
        }
        let metadata = take(&mut p, n)?;
        let m = root_as_message(metadata).map_err(|_| "invalid IPC metadata")?;
        let body_len = usize::try_from(m.bodyLength())?;
        if body_len > MAX_PAYLOAD {
            return bad();
        }
        let body = take(&mut p, body_len)?;
        match m.header_type() {
            MessageHeader::Schema if messages == 0 => {
                // Reject nested or attacker-selected schemas without building them.
                if metadata != trusted_schema || body_len != 0 {
                    return bad();
                }
            }
            MessageHeader::DictionaryBatch if messages > 0 && dictionaries == 0 && batches == 0 => {
                let d = m.header_as_dictionary_batch().ok_or("dictionary header")?;
                if d.id() != 0 || d.isDelta() {
                    return bad();
                }
                dictionary_len = check_batch(d.data().ok_or("dictionary data")?, None, body, 1, 3)?;
                dictionaries += 1;
            }
            MessageHeader::RecordBatch if messages > 0 && batches == 0 => {
                check_batch(
                    m.header_as_record_batch().ok_or("record header")?,
                    Some(rows),
                    body,
                    10,
                    20,
                )?;
                let rb = m.header_as_record_batch().ok_or("record header")?;
                let bs = rb.buffers().ok_or("buffers")?;
                let valid = buffer_bytes(body, bs.get(0))?;
                let keys = buffer_bytes(body, bs.get(1))?;
                let ns = rb.nodes().ok_or("nodes")?;
                for i in 0..rows {
                    let present =
                        ns.get(0).null_count() == 0 || (valid[i / 8] & (1 << (i % 8))) != 0;
                    if present {
                        let k = i32::from_le_bytes(keys[i * 4..i * 4 + 4].try_into()?);
                        if k < 0 || k as usize >= dictionary_len {
                            return bad();
                        }
                    }
                }
                batches += 1;
            }
            _ => return bad(),
        }
        messages += 1;
        if messages > 3 {
            return bad();
        }
    }
    Ok(())
}
fn check_batch(
    b: arrow::ipc::RecordBatch<'_>,
    rows: Option<usize>,
    body: &[u8],
    nodes: usize,
    buffers: usize,
) -> R<usize> {
    let len = usize::try_from(b.length())?;
    if len > MAX_ROWS
        || rows.is_some_and(|n| n != len)
        || b.compression().is_some()
        || b.variadicBufferCounts().is_some()
    {
        return bad();
    }
    let ns = b.nodes().ok_or("nodes")?;
    let bs = b.buffers().ok_or("buffers")?;
    if ns.len() != nodes || bs.len() != buffers {
        return bad();
    }
    for n in ns {
        if n.length() != b.length() || n.null_count() < 0 || n.null_count() > b.length() {
            return bad();
        }
    }
    // Monotonic, non-overlapping, wholly-backed buffers: Arrow may slice directly.
    let mut end = 0;
    for b in bs {
        let start = usize::try_from(b.offset())?;
        let size = usize::try_from(b.length())?;
        let next = start.checked_add(size).ok_or("buffer overflow")?;
        if start < end || next > body.len() {
            return bad();
        }
        end = next;
    }
    for i in 0..nodes {
        let nulls = bs.get(if nodes == 1 { 0 } else { i * 2 });
        if ns.get(i).null_count() > 0 && nulls.length() < len.div_ceil(8) as i64 {
            return bad();
        }
        let data = bs.get(if nodes == 1 { 1 } else { i * 2 + 1 });
        let width = if nodes == 1 || i == 0 {
            4
        } else if [1, 5, 6, 8, 9].contains(&i) {
            16
        } else {
            8
        };
        let needed = if nodes == 1 {
            (len + 1) * 4
        } else {
            len * width
        };
        if data.length() < needed as i64 {
            return bad();
        }
    }
    if nodes == 1 {
        let offsets = buffer_bytes(body, bs.get(1))?;
        let strings = buffer_bytes(body, bs.get(2))?;
        let mut previous = 0;
        for i in 0..=len {
            let v = i32::from_le_bytes(offsets[i * 4..i * 4 + 4].try_into()?);
            if v < previous || v < 0 || v as usize > strings.len() {
                return bad();
            }
            previous = v;
        }
    }
    Ok(len)
}
fn buffer_bytes<'a>(body: &'a [u8], b: &arrow::ipc::Buffer) -> R<&'a [u8]> {
    let start = usize::try_from(b.offset())?;
    let end = start
        .checked_add(usize::try_from(b.length())?)
        .ok_or("buffer length")?;
    body.get(start..end).ok_or_else(|| "backed buffer".into())
}
fn ipc_decode(payload: &[u8], rows: usize) -> R<Vec<State>> {
    preflight(payload, rows)?;
    let mut r = StreamReader::try_new(Cursor::new(payload), None)?;
    let b = r.next().ok_or("missing batch")??;
    if b.schema().as_ref() != &schema() || b.num_rows() != rows || r.next().is_some() {
        return bad();
    }
    let d = b
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .ok_or("dictionary")?;
    let strings = d
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or("strings")?;
    let dec = |col: usize, row: usize| -> R<Option<i128>> {
        let a = b
            .column(col)
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .ok_or("decimal")?;
        Ok((!a.is_null(row)).then(|| a.value(row)))
    };
    let uint = |col: usize, row: usize| -> R<u64> {
        let a = b
            .column(col)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or("uint")?;
        if a.is_null(row) {
            return bad();
        }
        Ok(a.value(row))
    };
    let mut out = Vec::with_capacity(rows);
    for i in 0..rows {
        let text = if d.is_null(i) {
            None
        } else {
            let k = usize::try_from(d.keys().value(i))?;
            if k >= strings.len() || strings.is_null(k) {
                return bad();
            }
            Some(strings.value(k).to_owned())
        };
        out.push(State {
            key: Key {
                text,
                decimal: dec(1, i)?,
            },
            hash: uint(2, i)?,
            count: uint(3, i)?,
            nonnull: uint(4, i)?,
            sum: dec(5, i)?,
            avg_sum: dec(6, i)?,
            avg_count: uint(7, i)?,
            min: dec(8, i)?,
            max: dec(9, i)?,
        });
    }
    Ok(out)
}
fn record_encode(states: &[State]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in states {
        match &s.key.text {
            None => out.extend(u32::MAX.to_le_bytes()),
            Some(t) => {
                out.extend((t.len() as u32).to_le_bytes());
                out.extend(t.as_bytes());
            }
        }
        for v in [s.key.decimal, s.sum, s.avg_sum, s.min, s.max] {
            out.push(v.is_some() as u8);
            out.extend(v.unwrap_or(0).to_le_bytes());
        }
        for v in [s.hash, s.count, s.nonnull, s.avg_count] {
            out.extend(v.to_le_bytes());
        }
    }
    out
}
fn record_decode(mut p: &[u8], rows: usize) -> R<Vec<State>> {
    let mut out = Vec::with_capacity(rows);
    for _ in 0..rows {
        let n = u32_at(take(&mut p, 4)?);
        let text = if n == u32::MAX {
            None
        } else {
            if n > 4096 {
                return bad();
            }
            Some(std::str::from_utf8(take(&mut p, n as usize)?)?.to_owned())
        };
        let mut v = [None; 5];
        for v in &mut v {
            let tag = take(&mut p, 1)?[0];
            let value = i128::from_le_bytes(take(&mut p, 16)?.try_into()?);
            *v = match tag {
                0 => None,
                1 => Some(value),
                _ => return bad(),
            };
        }
        let mut u = [0; 4];
        for u in &mut u {
            *u = u64_at(take(&mut p, 8)?);
        }
        out.push(State {
            key: Key {
                text,
                decimal: v[0],
            },
            hash: u[0],
            count: u[1],
            nonnull: u[2],
            sum: v[1],
            avg_sum: v[2],
            avg_count: u[3],
            min: v[3],
            max: v[4],
        });
    }
    if !p.is_empty() {
        return bad();
    }
    Ok(out)
}
pub fn seal(frame: &mut [u8]) {
    let mut crc = crc32fast::Hasher::new();
    crc.update(&frame[..28]);
    crc.update(&frame[HEADER..]);
    frame[28..32].copy_from_slice(&crc.finalize().to_le_bytes());
}
pub fn encode(states: &[State], format: Format) -> R<Vec<u8>> {
    if states.len() > MAX_ROWS
        || states
            .iter()
            .any(|s| s.key.text.as_ref().is_some_and(|t| t.len() > 4096))
    {
        return bad();
    }
    let payload = match format {
        Format::Ipc => ipc_encode(states)?,
        Format::RecordControl => record_encode(states),
    };
    if payload.len() > MAX_PAYLOAD {
        return bad();
    }
    let mut frame = vec![0; HEADER];
    frame[..8].copy_from_slice(b"NATAAG01");
    frame[8] = format as u8;
    // Partition=0 and depth=0 in this one-partition experiment.
    frame[16..20].copy_from_slice(&(states.len() as u32).to_le_bytes());
    frame[20..28].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    frame.extend(payload);
    seal(&mut frame);
    Ok(frame)
}
pub fn write(path: &Path, states: &[State], format: Format) -> R<usize> {
    let frame = encode(states, format)?;
    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(&frame)?;
    Ok(frame.len())
}
pub fn read(path: &Path, grant: usize) -> R<Vec<State>> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    let mut header = [0; HEADER];
    f.read_exact(&mut header)?;
    let rows = u32_at(&header[16..20]) as usize;
    let n = u64_at(&header[20..28]);
    // Working set allowance covers owned payload, Arrow's body copy, alignment,
    // arrays/keys/output, and fixed schema/preflight scratch (not process RSS).
    if &header[..8] != b"NATAAG01"
        || header[9..16] != [0; 7]
        || rows > MAX_ROWS
        || n > MAX_PAYLOAD as u64
        || n.checked_add(HEADER as u64) != Some(size)
        || grant < 8 * MAX_PAYLOAD
    {
        return bad();
    }
    let mut payload = vec![0; n as usize];
    f.read_exact(&mut payload)?;
    let mut crc = crc32fast::Hasher::new();
    crc.update(&header[..28]);
    crc.update(&payload);
    if crc.finalize() != u32_at(&header[28..32]) {
        return bad();
    }
    match header[8] {
        1 => ipc_decode(&payload, rows),
        2 => record_decode(&payload, rows),
        _ => bad(),
    }
}
/// Deterministic generated partials, not final SQL aggregates. Dictionary order
/// differs between seeds; identity is decoded value, never dictionary code.
pub fn generated(seed: u64, n: usize) -> Vec<State> {
    let mut x = seed.max(1);
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let wide = 10i128.pow(37) + x as i128;
            let value = if i % 2 == 0 { wide } else { -wide };
            let v = (i % 7 != 0).then_some(value);
            State {
                key: Key {
                    text: match i % 11 {
                        0 => None,
                        1 => Some(String::new()),
                        _ => Some(format!("k{}λ", x % 19)),
                    },
                    decimal: (i % 5 != 0).then_some(value),
                },
                hash: 0, // all keys deliberately collide
                count: 3,
                nonnull: if v.is_some() { 2 } else { 0 },
                sum: v,
                avg_sum: (i % 3 != 0).then_some(value - 17),
                avg_count: if i % 3 != 0 { 1 } else { 0 },
                min: (i % 5 != 0).then_some(value - 1),
                max: (i % 5 != 0).then_some(value + 1),
            }
        })
        .collect()
}
