//! A hand-written decoder for Iceberg v2 manifests (Avro object container files) that writes straight into compact arenas.
//!
//! No generic value tree: the writer schema in the file header is compiled once into a small program (what to keep, what to skip), and each record
//! is decoded in place. Everything a planner needs is kept; everything else (column sizes, key metadata, split offsets, ...) is skipped without being
//! materialised. Optionally only the statistics of some columns are kept (`want`), which is what a planner with a filter on two columns needs.
//! Containers: codecs null, deflate, snappy (with its 4-byte CRC) and zstandard. Anything it does not understand is an error, never a guess.

use std::collections::HashMap;

use serde_json::Value as J;

type R<T> = Result<T, String>;

// ------------------------------------------------------------------------------------------------------------------------ schema

#[derive(Debug, Clone)]
pub enum S {
    Null,
    Bool,
    Int,
    Long,
    Float,
    Double,
    Bytes,
    Str,
    Fixed(usize),
    Enum,
    Array(Box<S>),
    Map(Box<S>),
    Union(Vec<S>),
    Rec(Vec<(String, S)>),
}

pub fn parse_schema(v: &J, names: &mut HashMap<String, S>) -> R<S> {
    match v {
        J::String(s) => Ok(match s.as_str() {
            "null" => S::Null,
            "boolean" => S::Bool,
            "int" => S::Int,
            "long" => S::Long,
            "float" => S::Float,
            "double" => S::Double,
            "bytes" => S::Bytes,
            "string" => S::Str,
            other => names.get(other).cloned().ok_or_else(|| format!("unknown type name {other}"))?,
        }),
        J::Array(a) => Ok(S::Union(a.iter().map(|x| parse_schema(x, names)).collect::<R<Vec<_>>>()?)),
        J::Object(o) => {
            let t = o.get("type").ok_or("schema object without type")?;
            let ts = match t {
                J::String(s) => s.as_str(),
                other => return parse_schema(other, names),
            };
            let named = |s: S, names: &mut HashMap<String, S>| {
                if let Some(J::String(n)) = o.get("name") {
                    names.insert(n.clone(), s.clone());
                }
                s
            };
            Ok(match ts {
                "record" => {
                    let mut fields = vec![];
                    for f in o.get("fields").and_then(J::as_array).ok_or("record without fields")? {
                        let name = f.get("name").and_then(J::as_str).ok_or("field without name")?.to_string();
                        fields.push((name, parse_schema(f.get("type").ok_or("field without type")?, names)?));
                    }
                    named(S::Rec(fields), names)
                }
                "array" => S::Array(Box::new(parse_schema(o.get("items").ok_or("array without items")?, names)?)),
                "map" => S::Map(Box::new(parse_schema(o.get("values").ok_or("map without values")?, names)?)),
                "fixed" => {
                    let n = o.get("size").and_then(J::as_u64).ok_or("fixed without size")? as usize;
                    named(S::Fixed(n), names)
                }
                "enum" => named(S::Enum, names),
                _ => return parse_schema(&J::String(ts.to_string()), names),
            })
        }
        _ => Err("bad schema".into()),
    }
}

// ------------------------------------------------------------------------------------------------------------------------ cursor

pub struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Cur { b, p: 0 }
    }
    fn eof(&self) -> bool {
        self.p >= self.b.len()
    }
    fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
    /// A block of `n` items cannot be larger than the data left: every item of a manifest takes at least one byte. Rejecting it first stops a
    /// corrupt or hostile count from looping for ever on zero-size items (an array of null) or from reserving absurd memory.
    #[inline]
    fn check_count(&self, n: i64) -> R<usize> {
        #[cfg(feature = "bug-no-count-guard")]
        return Ok(n.max(0) as usize);
        #[allow(unreachable_code)]
        if n < 0 || n as u64 > self.remaining() as u64 {
            return Err(format!("item count {n} is larger than the {} bytes left", self.remaining()));
        }
        #[allow(unreachable_code)]
        Ok(n as usize)
    }
    #[inline]
    fn long(&mut self) -> R<i64> {
        let mut shift = 0u32;
        let mut v = 0u64;
        loop {
            let byte = *self.b.get(self.p).ok_or("unexpected end of data")?;
            self.p += 1;
            v |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(((v >> 1) as i64) ^ -((v & 1) as i64));
            }
            shift += 7;
            if shift > 63 {
                return Err("varint too long".into());
            }
        }
    }
    #[inline]
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.p.checked_add(n).ok_or("length overflow")?;
        let s = self.b.get(self.p..end).ok_or("unexpected end of data")?;
        self.p = end;
        Ok(s)
    }
    #[inline]
    fn len(&mut self) -> R<usize> {
        let n = self.long()?;
        usize::try_from(n).map_err(|_| format!("negative length {n}"))
    }
}

/// Skips one value of schema `s`.
fn skip(s: &S, c: &mut Cur) -> R<()> {
    match s {
        S::Null => {}
        S::Bool => {
            c.take(1)?;
        }
        S::Int | S::Long | S::Enum => {
            c.long()?;
        }
        S::Float => {
            c.take(4)?;
        }
        S::Double => {
            c.take(8)?;
        }
        S::Bytes | S::Str => {
            let n = c.len()?;
            c.take(n)?;
        }
        S::Fixed(n) => {
            c.take(*n)?;
        }
        S::Array(it) => loop {
            let n = c.long()?;
            if n == 0 {
                break;
            }
            if n < 0 {
                let bytes = c.len()?; // a block with a byte size can be skipped whole
                c.take(bytes)?;
                continue;
            }
            for _ in 0..c.check_count(n)? {
                skip(it, c)?;
            }
        },
        S::Map(v) => loop {
            let n = c.long()?;
            if n == 0 {
                break;
            }
            if n < 0 {
                let bytes = c.len()?;
                c.take(bytes)?;
                continue;
            }
            for _ in 0..c.check_count(n)? {
                let k = c.len()?;
                c.take(k)?;
                skip(v, c)?;
            }
        },
        S::Union(bs) => {
            let i = c.len()?;
            skip(bs.get(i).ok_or("union index out of range")?, c)?;
        }
        S::Rec(fs) => {
            for (_, f) in fs {
                skip(f, c)?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------------------------------ compact output

#[derive(Debug, Clone, Copy, Default)]
pub struct Range {
    pub off: u32,
    pub len: u32,
}

/// One manifest entry. Variable-size parts are ranges into the arenas of `Compact`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CEntry {
    pub status: i8,
    pub content: i8,
    pub has_snapshot: bool,
    pub has_seq: bool,
    pub has_fseq: bool,
    pub snapshot_id: i64,
    pub seq: i64,
    pub fseq: i64,
    pub record_count: i64,
    pub size: i64,
    pub path: Range,
    pub referenced: Range,
    pub part: Range,       // into `part`
    pub part_nulls: u32,   // bit i set = partition field i is null (or of a type this decoder does not keep)
    pub null_counts: Range, // into `counts`
    pub value_counts: Range,
    pub nan_counts: Range,
    pub lower: Range, // into `bounds`
    pub upper: Range,
    pub eq_ids: Range, // into `ids`
}

#[derive(Default)]
pub struct Compact {
    pub entries: Vec<CEntry>,
    pub strings: Vec<u8>,
    pub part: Vec<i64>,
    pub counts: Vec<(i32, i64)>,
    pub bounds: Vec<(i32, u32, u32)>, // (column id, offset into `bytes`, length)
    pub bytes: Vec<u8>,
    pub ids: Vec<i32>,
    /// Partition fields whose type is not int/long/date/timestamp (kept as null here and counted).
    pub unsupported_part_fields: u64,
    pub spec_id: i32,
    pub content: i32,
}

impl Compact {
    pub fn bytes_used(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<CEntry>()
            + self.strings.capacity()
            + self.part.capacity() * 8
            + self.counts.capacity() * 16
            + self.bounds.capacity() * 12
            + self.bytes.capacity()
            + self.ids.capacity() * 4
    }
}

// ------------------------------------------------------------------------------------------------------------------------ the program

#[derive(Clone, Copy, Debug, PartialEq)]
enum H {
    Skip,
    Status,
    Snap,
    Seq,
    FSeq,
    DataFile,
    Content,
    Path,
    Part,
    RecCount,
    Size,
    Values,
    Nulls,
    Nans,
    Lower,
    Upper,
    EqIds,
    Referenced,
}

fn handlers(fs: &[(String, S)], data_file: bool) -> Vec<H> {
    fs.iter()
        .map(|(n, _)| match (data_file, n.as_str()) {
            (false, "status") => H::Status,
            (false, "snapshot_id") => H::Snap,
            (false, "sequence_number") => H::Seq,
            (false, "file_sequence_number") => H::FSeq,
            (false, "data_file") => H::DataFile,
            (true, "content") => H::Content,
            (true, "file_path") => H::Path,
            (true, "partition") => H::Part,
            (true, "record_count") => H::RecCount,
            (true, "file_size_in_bytes") => H::Size,
            (true, "value_counts") => H::Values,
            (true, "null_value_counts") => H::Nulls,
            #[cfg(not(feature = "bug-skip-nan"))]
            (true, "nan_value_counts") => H::Nans,
            (true, "lower_bounds") => H::Lower,
            (true, "upper_bounds") => H::Upper,
            (true, "equality_ids") => H::EqIds,
            (true, "referenced_data_file") => H::Referenced,
            _ => H::Skip,
        })
        .collect()
}

/// For a possibly-optional field: reads the union tag; `None` if the branch is null.
#[inline]
fn opt<'s>(s: &'s S, c: &mut Cur) -> R<Option<&'s S>> {
    match s {
        S::Union(bs) => {
            let i = c.len()?;
            let b = bs.get(i).ok_or("union index out of range")?;
            Ok(if matches!(b, S::Null) { None } else { Some(b) })
        }
        other => Ok(Some(other)),
    }
}

fn read_int(s: &S, c: &mut Cur) -> R<i64> {
    match s {
        S::Int | S::Long => c.long(),
        other => Err(format!("expected int or long, found {other:?}")),
    }
}

fn str_into(arena: &mut Vec<u8>, s: &S, c: &mut Cur) -> R<Range> {
    match s {
        S::Str | S::Bytes => {
            let n = c.len()?;
            let b = c.take(n)?;
            // An Avro string is UTF-8 and consumers will treat the arena bytes as text; found by the byte-flip test: a flipped bit in a path was accepted.
            #[cfg(not(feature = "bug-no-utf8-check"))]
            if matches!(s, S::Str) && std::str::from_utf8(b).is_err() {
                return Err("a string is not valid UTF-8".into());
            }
            let off = arena.len();
            arena.extend_from_slice(b);
            Ok(Range { off: u32::try_from(off).map_err(|_| "string arena above 4 GiB")?, len: n as u32 })
        }
        other => Err(format!("expected string, found {other:?}")),
    }
}

/// An array of {key: int, value: long}; keeps the entries whose key is wanted.
fn counts_into(out: &mut Vec<(i32, i64)>, s: &S, c: &mut Cur, want: Option<&[i32]>) -> R<Range> {
    let start = out.len();
    let Some(S::Array(item)) = opt(s, c)? else { return Ok(Range::default()) };
    let S::Rec(fs) = &**item else { return Err("counts array item is not a record".into()) };
    if !matches!(fs.as_slice(), [(_, S::Int), (_, S::Long)]) {
        return Err("counts array item is not {int, long}".into());
    }
    loop {
        let mut n = c.long()?;
        if n == 0 {
            break;
        }
        if n < 0 {
            n = -n;
            c.long()?; // byte size, not needed
        }
        for _ in 0..c.check_count(n)? {
            let k = c.long()? as i32;
            let v = c.long()?;
            if want.is_none_or(|w| w.contains(&k)) {
                out.push((k, v));
            }
        }
    }
    Ok(Range { off: start as u32, len: (out.len() - start) as u32 })
}

/// An array of {key: int, value: bytes}.
fn bounds_into(out: &mut Vec<(i32, u32, u32)>, bytes: &mut Vec<u8>, s: &S, c: &mut Cur, want: Option<&[i32]>) -> R<Range> {
    let start = out.len();
    let Some(S::Array(item)) = opt(s, c)? else { return Ok(Range::default()) };
    let S::Rec(fs) = &**item else { return Err("bounds array item is not a record".into()) };
    if !matches!(fs.as_slice(), [(_, S::Int), (_, S::Bytes)]) {
        return Err("bounds array item is not {int, bytes}".into());
    }
    loop {
        let mut n = c.long()?;
        if n == 0 {
            break;
        }
        if n < 0 {
            n = -n;
            c.long()?;
        }
        for _ in 0..c.check_count(n)? {
            let k = c.long()? as i32;
            let len = c.len()?;
            let b = c.take(len)?;
            if want.is_none_or(|w| w.contains(&k)) {
                let off = bytes.len();
                bytes.extend_from_slice(b);
                out.push((k, off as u32, len as u32));
            }
        }
    }
    Ok(Range { off: start as u32, len: (out.len() - start) as u32 })
}

struct Program {
    top: Vec<(H, S)>,
    data: Vec<(H, S)>,
}

fn compile(schema: &S) -> R<Program> {
    let S::Rec(top) = schema else { return Err("manifest schema is not a record".into()) };
    let hs = handlers(top, false);
    let mut data = vec![];
    for ((_, s), h) in top.iter().zip(&hs) {
        if *h == H::DataFile {
            let S::Rec(df) = s else { return Err("data_file is not a record".into()) };
            let dh = handlers(df, true);
            data = df.iter().zip(dh).map(|((_, s), h)| (h, s.clone())).collect();
        }
    }
    if data.is_empty() {
        return Err("manifest schema has no data_file record".into());
    }
    Ok(Program { top: top.iter().zip(hs).map(|((_, s), h)| (h, s.clone())).collect(), data })
}

fn decode_entry(p: &Program, c: &mut Cur, out: &mut Compact, want: Option<&[i32]>) -> R<()> {
    let mut e = CEntry::default();
    for (h, s) in &p.top {
        match h {
            H::Status => e.status = read_int(s, c)? as i8,
            H::Snap => {
                if let Some(b) = opt(s, c)? {
                    e.snapshot_id = read_int(b, c)?;
                    e.has_snapshot = true;
                }
            }
            H::Seq => {
                if let Some(b) = opt(s, c)? {
                    e.seq = read_int(b, c)?;
                    e.has_seq = true;
                }
            }
            H::FSeq => {
                if let Some(b) = opt(s, c)? {
                    e.fseq = read_int(b, c)?;
                    e.has_fseq = true;
                }
            }
            H::DataFile => decode_data_file(p, c, out, &mut e, want)?,
            _ => skip(s, c)?,
        }
    }
    out.entries.push(e);
    Ok(())
}

fn decode_data_file(p: &Program, c: &mut Cur, out: &mut Compact, e: &mut CEntry, want: Option<&[i32]>) -> R<()> {
    for (h, s) in &p.data {
        match h {
            H::Content => e.content = read_int(s, c)? as i8,
            H::Path => e.path = str_into(&mut out.strings, s, c)?,
            H::Referenced => {
                if let Some(b) = opt(s, c)? {
                    e.referenced = str_into(&mut out.strings, b, c)?;
                }
            }
            H::RecCount => e.record_count = read_int(s, c)?,
            H::Size => e.size = read_int(s, c)?,
            H::Part => {
                let S::Rec(fs) = s else { return Err("partition is not a record".into()) };
                let start = out.part.len();
                for (i, (_, ft)) in fs.iter().enumerate() {
                    let v = match opt(ft, c)? {
                        None => None,
                        Some(S::Int | S::Long) => Some(read_int(fs_inner(ft), c)?),
                        Some(other) => {
                            skip(other, c)?;
                            out.unsupported_part_fields += 1;
                            None
                        }
                    };
                    if v.is_none() && i < 32 {
                        e.part_nulls |= 1 << i;
                    }
                    out.part.push(v.unwrap_or(0));
                }
                e.part = Range { off: start as u32, len: fs.len() as u32 };
            }
            H::Values => e.value_counts = counts_into(&mut out.counts, s, c, want)?,
            H::Nulls => e.null_counts = counts_into(&mut out.counts, s, c, want)?,
            H::Nans => e.nan_counts = counts_into(&mut out.counts, s, c, want)?,
            #[cfg(not(feature = "bug-swap-bounds"))]
            H::Lower => e.lower = bounds_into(&mut out.bounds, &mut out.bytes, s, c, want)?,
            #[cfg(not(feature = "bug-swap-bounds"))]
            H::Upper => e.upper = bounds_into(&mut out.bounds, &mut out.bytes, s, c, want)?,
            #[cfg(feature = "bug-swap-bounds")]
            H::Lower => e.upper = bounds_into(&mut out.bounds, &mut out.bytes, s, c, want)?,
            #[cfg(feature = "bug-swap-bounds")]
            H::Upper => e.lower = bounds_into(&mut out.bounds, &mut out.bytes, s, c, want)?,
            H::EqIds => {
                let start = out.ids.len();
                if let Some(S::Array(it)) = opt(s, c)? {
                    loop {
                        let mut n = c.long()?;
                        if n == 0 {
                            break;
                        }
                        if n < 0 {
                            n = -n;
                            c.long()?;
                        }
                        for _ in 0..c.check_count(n)? {
                            out.ids.push(read_int(&it, c)? as i32);
                        }
                    }
                }
                e.eq_ids = Range { off: start as u32, len: (out.ids.len() - start) as u32 };
            }
            _ => skip(s, c)?,
        }
    }
    Ok(())
}

/// The non-null branch of an optional field's schema (after `opt` consumed the tag).
fn fs_inner(s: &S) -> &S {
    match s {
        S::Union(bs) => bs.iter().find(|b| !matches!(b, S::Null)).unwrap_or(s),
        other => other,
    }
}

// ------------------------------------------------------------------------------------------------------------------------ container

fn decompress<'a>(codec: &str, data: &'a [u8]) -> R<std::borrow::Cow<'a, [u8]>> {
    use std::io::Read;
    Ok(match codec {
        "null" | "" => std::borrow::Cow::Borrowed(data),
        "deflate" => {
            let mut v = Vec::new();
            flate2::read::DeflateDecoder::new(data).read_to_end(&mut v).map_err(|e| format!("deflate: {e}"))?;
            std::borrow::Cow::Owned(v)
        }
        "snappy" => {
            if data.len() < 4 {
                return Err("snappy block shorter than its checksum".into());
            }
            let (body, crc) = data.split_at(data.len() - 4);
            let v = snap::raw::Decoder::new().decompress_vec(body).map_err(|e| format!("snappy: {e}"))?;
            let want = u32::from_be_bytes(crc.try_into().unwrap());
            #[cfg(not(feature = "bug-no-crc"))]
            if crc32(&v) != want {
                return Err("snappy block checksum mismatch".into());
            }
            std::borrow::Cow::Owned(v)
        }
        "zstandard" => std::borrow::Cow::Owned(zstd::stream::decode_all(data).map_err(|e| format!("zstd: {e}"))?),
        other => return Err(format!("unsupported avro codec {other}")),
    })
}

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CRC-32 (IEEE, as in zlib and java.util.zip.CRC32): the Avro specification ends every snappy block with the big-endian CRC-32 of the
/// UNCOMPRESSED data. (Not CRC-32C: the first version of this function used that and the check against real files caught it.)
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc = CRC_TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

type Header = (HashMap<String, Vec<u8>>, [u8; 16]);

/// Reads the container header: magic, metadata map, sync marker.
fn read_header(c: &mut Cur) -> R<Header> {
    if c.take(4)? != b"Obj\x01" {
        return Err("not an Avro object container file".into());
    }
    let mut meta: HashMap<String, Vec<u8>> = HashMap::new();
    loop {
        let mut n = c.long()?;
        if n == 0 {
            break;
        }
        if n < 0 {
            n = -n;
            c.long()?;
        }
        for _ in 0..c.check_count(n)? {
            let kl = c.len()?;
            let k = String::from_utf8(c.take(kl)?.to_vec()).map_err(|_| "metadata key is not UTF-8")?;
            let vl = c.len()?;
            meta.insert(k, c.take(vl)?.to_vec());
        }
    }
    let sync: [u8; 16] = c.take(16)?.try_into().unwrap();
    Ok((meta, sync))
}

/// Where the first data block of a container starts (the length of its header). For tests that corrupt the data region only.
pub fn data_offset(bytes: &[u8]) -> R<usize> {
    let mut c = Cur::new(bytes);
    read_header(&mut c)?;
    Ok(c.p)
}

/// Decodes a whole manifest file. `want`: keep the statistics of these column ids only (None = all).
pub fn decode_manifest(bytes: &[u8], want: Option<&[i32]>) -> R<Compact> {
    let mut c = Cur::new(bytes);
    let (meta, sync) = read_header(&mut c)?;
    let schema_json: J = serde_json::from_slice(meta.get("avro.schema").ok_or("no avro.schema in the header")?).map_err(|e| format!("schema json: {e}"))?;
    let schema = parse_schema(&schema_json, &mut HashMap::new())?;
    let program = compile(&schema)?;
    let codec = meta.get("avro.codec").map(|v| String::from_utf8_lossy(v).to_string()).unwrap_or_default();
    let mut out = Compact::default();
    out.spec_id = meta.get("partition-spec-id").and_then(|b| String::from_utf8_lossy(b).parse().ok()).unwrap_or(0);
    out.content = if meta.get("content").map(|b| b.as_slice()) == Some(b"deletes") { 1 } else { 0 };
    while !c.eof() {
        let count = c.len()?;
        let size = c.len()?;
        let raw = c.take(size)?;
        #[cfg(not(feature = "bug-no-sync-check"))]
        if c.take(16)? != sync {
            return Err("sync marker mismatch: the file is corrupt".into());
        }
        #[cfg(feature = "bug-no-sync-check")]
        c.take(16)?;
        let data = decompress(&codec, raw)?;
        let mut bc = Cur::new(&data);
        bc.check_count(count as i64)?;
        if out.entries.is_empty() {
            out.entries.reserve(count.min(1 << 16)); // never trust a count for a big up-front reservation
        }
        for _ in 0..count {
            decode_entry(&program, &mut bc, &mut out, want)?;
        }
        if !bc.eof() {
            return Err("a block has bytes left after its last record".into());
        }
    }
    Ok(out)
}
