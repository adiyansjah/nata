//! The wire format of one exchange stream (one TCP connection carries one stream, sender -> receiver for data, both ways for control).
//!
//! `magic u8 | kind u8 | seq u64 LE | len u32 LE | payload (len bytes)`. TCP already protects against bit errors, so there is no CRC:
//! what has to be defended against is a peer that is buggy or hostile (bad magic, unknown kind, absurd length, wrong length for a
//! control frame), a stream cut in the middle of a frame, and reads that return one byte at a time.

use std::io::{self, Read, Write};

pub const MAGIC: u8 = 0xE7;
pub const HEADER: usize = 14;
/// The largest payload a receiver accepts. Checked BEFORE allocating, so a hostile length cannot make the receiver allocate.
pub const MAX_PAYLOAD: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// sender -> receiver, first frame: who is sending, for which query.
    Hello { src: u32, query: u64 },
    /// sender -> receiver: batch number `seq` (0, 1, 2, ... without gaps).
    Data { seq: u64, payload: Vec<u8> },
    /// sender -> receiver: the stream is complete and consisted of `count` Data frames.
    Eos { count: u64 },
    /// receiver -> sender: the consumer took `bytes` more bytes off the queue; the sender may send that much more.
    Credit { bytes: u64 },
    /// receiver -> sender: stop, the consumer does not want more.
    Cancel,
    Ping { n: u64 },
    Pong { n: u64 },
}

#[derive(Debug)]
pub enum WireError {
    Io(io::Error),
    BadMagic(u8),
    BadKind(u8),
    TooLarge(u64),
    /// The stream ended in the middle of a frame.
    Truncated,
    /// A control frame with a payload it must not have (or a Hello of the wrong size).
    BadLength { kind: u8, len: u32 },
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Io(e) => write!(f, "io error: {e}"),
            WireError::BadMagic(b) => write!(f, "bad magic byte 0x{b:02x}"),
            WireError::BadKind(k) => write!(f, "unknown frame kind {k}"),
            WireError::TooLarge(n) => write!(f, "frame payload of {n} bytes is above the limit of {MAX_PAYLOAD}"),
            WireError::Truncated => write!(f, "connection closed in the middle of a frame"),
            WireError::BadLength { kind, len } => write!(f, "frame kind {kind} has an invalid length {len}"),
        }
    }
}

impl From<io::Error> for WireError {
    fn from(e: io::Error) -> Self {
        WireError::Io(e)
    }
}

const K_HELLO: u8 = 1;
const K_DATA: u8 = 2;
const K_EOS: u8 = 3;
const K_CREDIT: u8 = 4;
const K_CANCEL: u8 = 5;
const K_PING: u8 = 6;
const K_PONG: u8 = 7;

impl Frame {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let (kind, seq, payload): (u8, u64, &[u8]) = match self {
            Frame::Hello { src, .. } => (K_HELLO, *src as u64, &[]),
            Frame::Data { seq, payload } => (K_DATA, *seq, payload),
            Frame::Eos { count } => (K_EOS, *count, &[]),
            Frame::Credit { bytes } => (K_CREDIT, *bytes, &[]),
            Frame::Cancel => (K_CANCEL, 0, &[]),
            Frame::Ping { n } => (K_PING, *n, &[]),
            Frame::Pong { n } => (K_PONG, *n, &[]),
        };
        let query_bytes;
        let payload = if let Frame::Hello { query, .. } = self {
            query_bytes = query.to_le_bytes();
            &query_bytes[..]
        } else {
            payload
        };
        out.push(MAGIC);
        out.push(kind);
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
    }
}

pub fn write_frame<W: Write>(w: &mut W, f: &Frame) -> io::Result<()> {
    let mut buf = Vec::with_capacity(HEADER + if let Frame::Data { payload, .. } = f { payload.len() } else { 8 });
    f.encode(&mut buf);
    w.write_all(&buf)
}

/// Reads until `buf` is full or the stream ends; returns how many bytes were read.
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<usize, WireError> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(WireError::Io(e)),
        }
    }
    Ok(got)
}

/// The next frame; `Ok(None)` if the stream ended cleanly BETWEEN frames. A stream that ends inside a frame is `Truncated`.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Option<Frame>, WireError> {
    let mut h = [0u8; HEADER];
    match read_full(r, &mut h)? {
        0 => return Ok(None),
        n if n < HEADER => return Err(WireError::Truncated),
        _ => {}
    }
    if h[0] != MAGIC {
        return Err(WireError::BadMagic(h[0]));
    }
    let kind = h[1];
    let seq = u64::from_le_bytes(h[2..10].try_into().unwrap());
    let len = u32::from_le_bytes(h[10..14].try_into().unwrap());
    if !(K_HELLO..=K_PONG).contains(&kind) {
        return Err(WireError::BadKind(kind));
    }
    #[cfg(not(feature = "bug-no-size-limit"))]
    if len as usize > MAX_PAYLOAD {
        return Err(WireError::TooLarge(len as u64));
    }
    let expect_len = match kind {
        K_HELLO => Some(8),
        K_DATA => None,
        _ => Some(0),
    };
    if let Some(e) = expect_len {
        if len != e {
            return Err(WireError::BadLength { kind, len });
        }
    }
    let mut payload = vec![0u8; len as usize];
    if read_full(r, &mut payload)? < payload.len() {
        return Err(WireError::Truncated);
    }
    Ok(Some(match kind {
        K_HELLO => Frame::Hello { src: seq as u32, query: u64::from_le_bytes(payload[..8].try_into().unwrap()) },
        K_DATA => Frame::Data { seq, payload },
        K_EOS => Frame::Eos { count: seq },
        K_CREDIT => Frame::Credit { bytes: seq },
        K_CANCEL => Frame::Cancel,
        K_PING => Frame::Ping { n: seq },
        _ => Frame::Pong { n: seq },
    }))
}
