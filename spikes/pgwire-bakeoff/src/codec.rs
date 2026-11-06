use bytes::BytesMut;
use pgwire::messages::{DecodeContext, PgWireFrontendMessage as F};
pub const MAX: usize = 1_048_576 + 65_536;
#[derive(Debug)]
pub enum Msg {
    Startup,
    Cancel(i32, i32),
    Ssl,
    Query(String),
    Parse(String),
    Bind,
    Describe(u8),
    Execute,
    Sync,
    Close,
    Flush,
    End,
}
struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if n > self.0.len() {
            return Err("truncated".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u16(&mut self) -> Result<usize, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as usize)
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.0.iter().position(|b| *b == 0).ok_or("cstring")?;
        let s = std::str::from_utf8(self.take(n)?)
            .map_err(|_| "utf8")?
            .to_owned();
        self.take(1)?;
        Ok(s)
    }
}
pub fn hand(b: &[u8], startup: bool) -> Result<Msg, String> {
    let off = usize::from(!startup);
    if b.len() < off + 4 {
        return Err("header".into());
    }
    let len = i32::from_be_bytes(b[off..off + 4].try_into().unwrap());
    if len < 4 || len as usize > MAX {
        return Err("length".into());
    }
    if b.len() != len as usize + off {
        return Err("frame".into());
    }
    let mut c = Cursor(&b[off + 4..]);
    let m = if startup {
        match c.i32()? {
            196608 => {
                while !c.0.is_empty() && c.0[0] != 0 {
                    c.str()?;
                    c.str()?;
                }
                c.take(1)?;
                Msg::Startup
            }
            80877103 | 80877104 => Msg::Ssl,
            80877102 => Msg::Cancel(c.i32()?, c.i32()?),
            _ => return Err("protocol".into()),
        }
    } else {
        match b[0] {
            b'Q' => Msg::Query(c.str()?),
            b'P' => {
                let name = c.str()?;
                if !name.is_empty() {
                    return Err("named".into());
                }
                let q = c.str()?;
                let n = c.u16()?;
                c.take(n.checked_mul(4).ok_or("count")?)?;
                Msg::Parse(q)
            }
            b'B' => {
                c.str()?;
                c.str()?;
                let n = c.u16()?;
                for _ in 0..n {
                    if c.u16()? != 0 {
                        return Err("binary".into());
                    }
                }
                let n = c.u16()?;
                for _ in 0..n {
                    let len = c.i32()?;
                    if len >= 0 {
                        c.take(len as usize)?;
                    } else if len != -1 {
                        return Err("param length".into());
                    }
                }
                let n = c.u16()?;
                for _ in 0..n {
                    if c.u16()? != 0 {
                        return Err("binary".into());
                    }
                }
                Msg::Bind
            }
            b'D' | b'C' => {
                let t = c.take(1)?[0];
                if t != b'S' && t != b'P' {
                    return Err("target".into());
                }
                c.str()?;
                if b[0] == b'D' {
                    Msg::Describe(t)
                } else {
                    Msg::Close
                }
            }
            b'E' => {
                c.str()?;
                if c.i32()? < 0 {
                    return Err("rows".into());
                }
                Msg::Execute
            }
            b'S' => Msg::Sync,
            b'H' => Msg::Flush,
            b'X' => Msg::End,
            _ => return Err("tag".into()),
        }
    };
    if !c.0.is_empty() {
        return Err("trailing".into());
    }
    Ok(m)
}
pub fn pg(b: &[u8], startup: bool) -> Result<Msg, String> {
    let mut ctx = DecodeContext::default();
    ctx.awaiting_frontend_ssl = startup
        && b.len() >= 8
        && matches!(
            i32::from_be_bytes(b[4..8].try_into().unwrap()),
            80877103 | 80877104
        );
    ctx.awaiting_frontend_startup = startup;
    let m = F::decode(&mut BytesMut::from(b), &ctx)
        .map_err(|e| e.to_string())?
        .ok_or("incomplete")?;
    Ok(match m {
        F::Startup(_) => Msg::Startup,
        F::SslNegotiation(_) => Msg::Ssl,
        F::CancelRequest(c) => Msg::Cancel(
            c.pid,
            match c.secret_key {
                pgwire::messages::startup::SecretKey::I32(k) => k,
                pgwire::messages::startup::SecretKey::Bytes(k) if k.len() == 4 => {
                    i32::from_be_bytes(k[..].try_into().unwrap())
                }
                _ => return Err("key".into()),
            },
        ),
        F::Query(q) => Msg::Query(q.query),
        F::Parse(p) => Msg::Parse(p.query),
        F::Bind(_) => Msg::Bind,
        F::Describe(d) => Msg::Describe(d.target_type),
        F::Execute(_) => Msg::Execute,
        F::Sync(_) => Msg::Sync,
        F::Close(_) => Msg::Close,
        F::Flush(_) => Msg::Flush,
        F::Terminate(_) => Msg::End,
        _ => return Err("unsupported".into()),
    })
}
