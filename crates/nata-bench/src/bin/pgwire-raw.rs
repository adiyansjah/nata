//! Fast counted/discarded PostgreSQL text result reader. Startup is outside timing.
use anyhow::{Context, Result, ensure};
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn frame(r: &mut impl Read, body: &mut Vec<u8>) -> Result<u8> {
    let mut h = [0; 5];
    r.read_exact(&mut h)?;
    let n = u32::from_be_bytes(h[1..].try_into()?) as usize;
    ensure!((4..=64 * 1024 * 1024).contains(&n), "backend frame length");
    body.resize(n - 4, 0);
    r.read_exact(body)?;
    ensure!(
        h[0] != b'E',
        "backend error: {}",
        String::from_utf8_lossy(body)
    );
    Ok(h[0])
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 2 || args.len() == 3,
        "usage: pgwire-raw PORT SQL [--verify]"
    );
    let verify = args.get(2).is_some_and(|a| a == "--verify");
    let socket = TcpStream::connect(format!("127.0.0.1:{}", args[0])).context("connect")?;
    socket.set_nodelay(true)?;
    socket.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut write = socket.try_clone()?;
    let mut read = BufReader::with_capacity(256 * 1024, socket);
    let startup = b"\0\x03\0\0user\0test\0database\0nata\0\0";
    write.write_all(&((startup.len() + 4) as u32).to_be_bytes())?;
    write.write_all(startup)?;
    let mut body = Vec::with_capacity(1024);
    while frame(&mut read, &mut body)? != b'Z' {}
    let mut query = Vec::with_capacity(args[1].len() + 6);
    query.push(b'Q');
    query.extend(((args[1].len() + 5) as u32).to_be_bytes());
    query.extend(args[1].as_bytes());
    query.push(0);
    let start = Instant::now();
    write.write_all(&query)?;
    let mut rows = 0u64;
    let mut bytes = 0u64;
    let mut first = 0.0;
    loop {
        let tag = frame(&mut read, &mut body)?;
        bytes += body.len() as u64 + 5;
        if tag == b'D' {
            if rows == 0 {
                first = start.elapsed().as_secs_f64();
            }
            if verify {
                ensure!(body.get(..2) == Some(&[0, 1]), "one column");
                let n = i32::from_be_bytes(body.get(2..6).context("field length")?.try_into()?);
                ensure!(
                    n >= 0 && n as usize == body.len() - 6,
                    "non-NULL scalar length"
                );
                ensure!(
                    std::str::from_utf8(&body[6..])?.parse::<u64>()? == rows,
                    "ordered value at row {rows}"
                );
            }
            rows += 1;
        }
        if tag == b'Z' {
            break;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!("{{\"secs\":{secs:.9},\"first\":{first:.9},\"rows\":{rows},\"bytes\":{bytes}}}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_truncated_and_error_frames_are_rejected() {
        for bytes in [
            b"D\0\0\0\x03".as_slice(),
            b"D\0\0\0\x08x",
            b"E\0\0\0\x08oops",
        ] {
            assert!(frame(&mut std::io::Cursor::new(bytes), &mut Vec::new()).is_err());
        }
        let mut body = Vec::new();
        assert_eq!(
            frame(&mut std::io::Cursor::new(b"D\0\0\0\x08test"), &mut body).expect("frame"),
            b'D'
        );
        assert_eq!(body, b"test");
    }
}
