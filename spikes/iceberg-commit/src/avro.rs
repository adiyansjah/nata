//! A minimal Avro object container file (null codec) writer and reader, enough for the Iceberg manifest, manifest-list,
//! data and position-delete schemas. Hand-written so that the header carries exactly the schema JSON with Iceberg's
//! `field-id` attributes. Independent validation of the files is the pyiceberg step of gcp-check.sh.
use crate::Res;

#[derive(Clone, Debug)]
pub enum Sch {
    Int,
    Long,
    Str,
    /// name, fields: (name, field-id, schema)
    Rec(String, Vec<(String, Option<i64>, Sch)>),
    /// the union ["null", T]
    Opt(Box<Sch>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Null,
    Int(i32),
    Long(i64),
    Str(String),
    Rec(Vec<Val>),
}

impl Val {
    pub fn long(&self) -> i64 {
        match self { Val::Long(v) => *v, Val::Int(v) => *v as i64, other => panic!("not a long: {other:?}") }
    }
    pub fn int(&self) -> i32 {
        match self { Val::Int(v) => *v, other => panic!("not an int: {other:?}") }
    }
    pub fn str(&self) -> &str {
        match self { Val::Str(v) => v, other => panic!("not a string: {other:?}") }
    }
    pub fn opt_long(&self) -> Option<i64> {
        match self { Val::Null => None, v => Some(v.long()) }
    }
    pub fn rec(&self) -> &[Val] {
        match self { Val::Rec(v) => v, other => panic!("not a record: {other:?}") }
    }
}

pub fn opt_long(v: Option<i64>) -> Val {
    v.map(Val::Long).unwrap_or(Val::Null)
}

impl Sch {
    pub fn to_json(&self) -> String {
        match self {
            Sch::Int => "\"int\"".into(),
            Sch::Long => "\"long\"".into(),
            Sch::Str => "\"string\"".into(),
            Sch::Opt(t) => format!("[\"null\",{}]", t.to_json()),
            Sch::Rec(name, fields) => {
                let fs: Vec<String> = fields
                    .iter()
                    .map(|(n, id, s)| {
                        let mut f = format!("{{\"name\":\"{n}\",\"type\":{}", s.to_json());
                        if let Sch::Opt(_) = s {
                            f.push_str(",\"default\":null");
                        }
                        if let Some(id) = id {
                            f.push_str(&format!(",\"field-id\":{id}"));
                        }
                        f.push('}');
                        f
                    })
                    .collect();
                format!("{{\"type\":\"record\",\"name\":\"{name}\",\"fields\":[{}]}}", fs.join(","))
            }
        }
    }
}

fn put_long(out: &mut Vec<u8>, v: i64) {
    let mut z = ((v << 1) ^ (v >> 63)) as u64;
    loop {
        let b = (z & 0x7F) as u8;
        z >>= 7;
        if z == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_long(out, b.len() as i64);
    out.extend_from_slice(b);
}

pub fn encode(out: &mut Vec<u8>, s: &Sch, v: &Val) {
    match (s, v) {
        (Sch::Int, Val::Int(x)) => put_long(out, *x as i64),
        (Sch::Long, Val::Long(x)) => put_long(out, *x),
        (Sch::Str, Val::Str(x)) => put_bytes(out, x.as_bytes()),
        (Sch::Opt(_), Val::Null) => put_long(out, 0),
        (Sch::Opt(t), other) => {
            put_long(out, 1);
            encode(out, t, other)
        }
        (Sch::Rec(_, fields), Val::Rec(vals)) => {
            assert_eq!(fields.len(), vals.len(), "record arity");
            for ((_, _, fs), fv) in fields.iter().zip(vals) {
                encode(out, fs, fv);
            }
        }
        (s, v) => panic!("avro encode: schema {s:?} does not match value {v:?}"),
    }
}

struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn long(&mut self) -> Res<i64> {
        let mut shift = 0;
        let mut z: u64 = 0;
        loop {
            let byte = *self.b.get(self.i).ok_or("avro: unexpected end of data")?;
            self.i += 1;
            z |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 63 {
                return Err("avro: varint too long".into());
            }
        }
        Ok(((z >> 1) as i64) ^ -((z & 1) as i64))
    }
    fn bytes(&mut self) -> Res<&'a [u8]> {
        let n = self.long()?;
        if n < 0 || self.i + n as usize > self.b.len() {
            return Err("avro: bad length".into());
        }
        let s = &self.b[self.i..self.i + n as usize];
        self.i += n as usize;
        Ok(s)
    }
}

fn decode(c: &mut Cur, s: &Sch) -> Res<Val> {
    Ok(match s {
        Sch::Int => Val::Int(c.long()? as i32),
        Sch::Long => Val::Long(c.long()?),
        Sch::Str => Val::Str(String::from_utf8(c.bytes()?.to_vec()).map_err(|e| format!("avro: utf8: {e}"))?),
        Sch::Opt(t) => match c.long()? {
            0 => Val::Null,
            1 => decode(c, t)?,
            n => return Err(format!("avro: union branch {n}")),
        },
        Sch::Rec(_, fields) => {
            let mut vals = Vec::with_capacity(fields.len());
            for (_, _, fs) in fields {
                vals.push(decode(c, fs)?);
            }
            Val::Rec(vals)
        }
    })
}

/// Write an object container file: header (schema + metadata) and one block holding every record.
pub fn write_file(schema: &Sch, meta: &[(&str, String)], records: &[Val], sync_seed: u64) -> Vec<u8> {
    let mut out = b"Obj\x01".to_vec();
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        ("avro.schema".into(), schema.to_json().into_bytes()),
        ("avro.codec".into(), b"null".to_vec()),
    ];
    for (k, v) in meta {
        entries.push(((*k).to_string(), v.clone().into_bytes()));
    }
    put_long(&mut out, entries.len() as i64);
    for (k, v) in &entries {
        put_bytes(&mut out, k.as_bytes());
        put_bytes(&mut out, v);
    }
    put_long(&mut out, 0);
    let mut sync = [0u8; 16];
    let mut p = crate::Prng::new(sync_seed);
    for chunk in sync.chunks_mut(8) {
        chunk.copy_from_slice(&p.next().to_le_bytes());
    }
    out.extend_from_slice(&sync);
    if !records.is_empty() {
        let mut block = Vec::new();
        for r in records {
            encode(&mut block, schema, r);
        }
        put_long(&mut out, records.len() as i64);
        put_long(&mut out, block.len() as i64);
        out.extend_from_slice(&block);
        out.extend_from_slice(&sync);
    }
    out
}

pub struct AvroFile {
    pub meta: Vec<(String, String)>,
    pub records: Vec<Val>,
}

impl AvroFile {
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Read an object container file written with the null codec, decoding records with the expected schema.
pub fn read_file(bytes: &[u8], schema: &Sch) -> Res<AvroFile> {
    if bytes.len() < 4 || &bytes[..4] != b"Obj\x01" {
        return Err("avro: bad magic".into());
    }
    let mut c = Cur { b: bytes, i: 4 };
    let mut meta = Vec::new();
    loop {
        let n = c.long()?;
        if n == 0 {
            break;
        }
        let n = if n < 0 {
            c.long()?; // block size in bytes, unused
            -n
        } else {
            n
        };
        for _ in 0..n {
            let k = String::from_utf8_lossy(c.bytes()?).to_string();
            let v = String::from_utf8_lossy(c.bytes()?).to_string();
            meta.push((k, v));
        }
    }
    if let Some((_, codec)) = meta.iter().find(|(k, _)| k == "avro.codec") {
        if codec != "null" {
            return Err(format!("avro: unsupported codec {codec}"));
        }
    }
    let sync = bytes.get(c.i..c.i + 16).ok_or("avro: missing sync marker")?.to_vec();
    c.i += 16;
    let mut records = Vec::new();
    while c.i < bytes.len() {
        let count = c.long()?;
        let size = c.long()? as usize;
        let end = c.i + size;
        if end + 16 > bytes.len() {
            return Err("avro: truncated block".into());
        }
        let mut bc = Cur { b: &bytes[..end], i: c.i };
        for _ in 0..count {
            records.push(decode(&mut bc, schema)?);
        }
        if bc.i != end {
            return Err("avro: block size does not match its records".into());
        }
        if bytes[end..end + 16] != sync[..] {
            return Err("avro: bad sync marker".into());
        }
        c.i = end + 16;
    }
    Ok(AvroFile { meta, records })
}
