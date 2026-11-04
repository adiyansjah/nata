//! Framing of the toy exchange: `magic u8 | kind u8 | len u32 LE | payload | crc32 LE` (the CRC covers everything
//! before it). A frame that does not decode (flipped bit, truncation, trailing bytes) is dropped by the receiver,
//! which is then just a lost message: the protocol's retransmission repairs it.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// coordinator -> worker: read `file` and stream the result of `task`, attempt `attempt`.
    Assign { task: u32, attempt: u32, file: u32 },
    /// coordinator -> worker: all frames before `next` of this attempt arrived. `stale`: drop this task.
    Ack { task: u32, attempt: u32, next: u32, stale: bool },
    Cancel,
    Shutdown,
    /// coordinator -> worker: reply to a heartbeat (refreshes the worker's lease).
    HbAck,
    /// worker -> coordinator.
    Heartbeat { born: u64 },
    /// worker -> coordinator: frame `seq` of a task's output; `last` marks the end of the stream.
    Data { task: u32, attempt: u32, seq: u32, last: bool, rows: Vec<i64> },
    /// worker -> coordinator: the worker incarnation `born` dropped all its tasks.
    CancelAck { born: u64 },
    /// worker -> coordinator: this incarnation is exiting.
    Bye { born: u64 },
    /// client -> coordinator.
    ClientCancel,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    Short,
    Magic,
    Length,
    Crc,
    Kind(u8),
    Payload,
}

const MAGIC: u8 = 0xA7;

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn u8(&mut self) -> Result<u8, FrameError> {
        let (h, t) = self.0.split_first().ok_or(FrameError::Payload)?;
        self.0 = t;
        Ok(*h)
    }

    fn u32(&mut self) -> Result<u32, FrameError> {
        if self.0.len() < 4 {
            return Err(FrameError::Payload);
        }
        let (h, t) = self.0.split_at(4);
        self.0 = t;
        Ok(u32::from_le_bytes(h.try_into().expect("4 bytes")))
    }

    fn u64(&mut self) -> Result<u64, FrameError> {
        if self.0.len() < 8 {
            return Err(FrameError::Payload);
        }
        let (h, t) = self.0.split_at(8);
        self.0 = t;
        Ok(u64::from_le_bytes(h.try_into().expect("8 bytes")))
    }
}

impl Msg {
    fn kind(&self) -> u8 {
        match self {
            Msg::Assign { .. } => 1,
            Msg::Ack { .. } => 2,
            Msg::Cancel => 3,
            Msg::Shutdown => 4,
            Msg::HbAck => 5,
            Msg::Heartbeat { .. } => 6,
            Msg::Data { .. } => 7,
            Msg::CancelAck { .. } => 8,
            Msg::Bye { .. } => 9,
            Msg::ClientCancel => 10,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut p = Vec::new();
        match self {
            Msg::Assign { task, attempt, file } => {
                put32(&mut p, *task);
                put32(&mut p, *attempt);
                put32(&mut p, *file);
            }
            Msg::Ack { task, attempt, next, stale } => {
                put32(&mut p, *task);
                put32(&mut p, *attempt);
                put32(&mut p, *next);
                p.push(*stale as u8);
            }
            Msg::Heartbeat { born } | Msg::CancelAck { born } | Msg::Bye { born } => p.extend_from_slice(&born.to_le_bytes()),
            Msg::Data { task, attempt, seq, last, rows } => {
                put32(&mut p, *task);
                put32(&mut p, *attempt);
                put32(&mut p, *seq);
                p.push(*last as u8);
                put32(&mut p, rows.len() as u32);
                for r in rows {
                    p.extend_from_slice(&r.to_le_bytes());
                }
            }
            Msg::Cancel | Msg::Shutdown | Msg::HbAck | Msg::ClientCancel => {}
        }
        let mut f = Vec::with_capacity(p.len() + 10);
        f.push(MAGIC);
        f.push(self.kind());
        put32(&mut f, p.len() as u32);
        f.extend_from_slice(&p);
        let crc = crc32(&f);
        put32(&mut f, crc);
        f
    }

    pub fn decode(bytes: &[u8]) -> Result<Msg, FrameError> {
        if bytes.len() < 10 {
            return Err(FrameError::Short);
        }
        if bytes[0] != MAGIC {
            return Err(FrameError::Magic);
        }
        let len = u32::from_le_bytes(bytes[2..6].try_into().expect("4 bytes")) as usize;
        if bytes.len() != 10 + len {
            return Err(FrameError::Length);
        }
        let body = &bytes[..6 + len];
        let want = u32::from_le_bytes(bytes[6 + len..].try_into().expect("4 bytes"));
        if crc32(body) != want {
            return Err(FrameError::Crc);
        }
        let mut r = Rd(&bytes[6..6 + len]);
        let msg = match bytes[1] {
            1 => Msg::Assign { task: r.u32()?, attempt: r.u32()?, file: r.u32()? },
            2 => Msg::Ack { task: r.u32()?, attempt: r.u32()?, next: r.u32()?, stale: r.u8()? != 0 },
            3 => Msg::Cancel,
            4 => Msg::Shutdown,
            5 => Msg::HbAck,
            6 => Msg::Heartbeat { born: r.u64()? },
            7 => {
                let task = r.u32()?;
                let attempt = r.u32()?;
                let seq = r.u32()?;
                let last = r.u8()? != 0;
                let n = r.u32()? as usize;
                if n > r.0.len() / 8 {
                    return Err(FrameError::Payload);
                }
                let mut rows = Vec::with_capacity(n);
                for _ in 0..n {
                    rows.push(r.u64()? as i64);
                }
                Msg::Data { task, attempt, seq, last, rows }
            }
            8 => Msg::CancelAck { born: r.u64()? },
            9 => Msg::Bye { born: r.u64()? },
            10 => Msg::ClientCancel,
            k => return Err(FrameError::Kind(k)),
        };
        if !r.0.is_empty() {
            return Err(FrameError::Payload);
        }
        Ok(msg)
    }
}

pub fn encode_i64s(v: &[i64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 8);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn decode_i64s(b: &[u8]) -> Vec<i64> {
    b.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().expect("8 bytes"))).collect()
}
