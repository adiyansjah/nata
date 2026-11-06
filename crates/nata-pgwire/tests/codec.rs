use nata_pgwire::codec::*;
fn frontend(tag: u8, body: &[u8]) -> Vec<u8> {
    packet(tag, body).expect("packet")
}
fn startup(body: &[u8]) -> Vec<u8> {
    let mut p = ((body.len() + 4) as i32).to_be_bytes().to_vec();
    p.extend(body);
    p
}
#[test]
fn every_frontend_message_and_every_truncation() {
    let cases = [
        (true, startup(b"\0\x03\0\0user\0test\0\0")),
        (true, startup(&80877103i32.to_be_bytes())),
        (true, startup(&80877104i32.to_be_bytes())),
        (
            true,
            startup(
                &[
                    80877102i32.to_be_bytes(),
                    1i32.to_be_bytes(),
                    2i32.to_be_bytes(),
                ]
                .concat(),
            ),
        ),
        (false, frontend(b'Q', b"select\0")),
        (false, frontend(b'P', b"\0select\0\0\x01\0\0\0\x17")),
        (
            false,
            frontend(
                b'B',
                b"\0\0\0\x01\0\0\0\x02\xff\xff\xff\xff\0\0\0\x01x\0\x01\0\0",
            ),
        ),
        (false, frontend(b'D', b"S\0")),
        (false, frontend(b'C', b"P\0")),
        (false, frontend(b'E', b"\0\0\0\0\x01")),
        (false, frontend(b'S', b"")),
        (false, frontend(b'H', b"")),
        (false, frontend(b'X', b"")),
    ];
    for (s, b) in cases {
        assert!(decode(&b, s).is_ok(), "{b:?}");
        for end in 0..b.len() {
            assert!(decode(&b[..end], s).is_err());
        }
        let mut extra = b;
        extra.push(0);
        assert!(decode(&extra, s).is_err());
    }
    assert_eq!(
        decode(&frontend(b'Q', b"hi\0"), false),
        Ok(Message::Query("hi".into()))
    );
    for b in [
        frontend(b'Q', b"\xff\0"),
        frontend(b'P', b"\0\0\xff\xff"),
        frontend(b'B', b"\0\0\x7f\xff"),
        frontend(b'E', b"\0\xff\xff\xff\xff"),
        frontend(b'D', b"X\0"),
    ] {
        assert!(decode(&b, false).is_err());
    }
    assert!(decode(&startup(b"\x04\xd2\x16\x2e"), true).is_err());
    assert!(
        decode(
            &frontend(b'Q', &[vec![b'a'; MAX_SQL + 1], vec![0]].concat()),
            false
        )
        .is_err()
    );
    for (s, max) in [(true, MAX_STARTUP), (false, MAX_FRAME)] {
        for n in [-1, 0, 3, (max + 1) as i32, i32::MAX] {
            assert!(length(n.to_be_bytes(), s).is_err());
        }
        assert_eq!(length((max as i32).to_be_bytes(), s), Ok(max));
    }
}
#[test]
fn backend_metadata_null_empty_and_character_position() {
    use nata_sql::types::SqlType::*;
    let types = [
        Boolean,
        TinyInt,
        SmallInt,
        Integer,
        BigInt,
        Float,
        Double,
        Decimal {
            precision: 38,
            scale: 9,
        },
        Varchar,
        Date,
    ];
    let expected = [
        (16, 1, -1),
        (25, -1, -1),
        (21, 2, -1),
        (23, 4, -1),
        (20, 8, -1),
        (700, 4, -1),
        (701, 8, -1),
        (1700, -1, 4 + (38 << 16) + 9),
        (25, -1, -1),
        (1082, 4, -1),
    ];
    for (t, want) in types.into_iter().zip(expected) {
        assert_eq!(type_info(t), want);
    }
    assert_eq!(
        &row(&[None, Some(String::new())]).expect("row")[5..],
        b"\0\x02\xff\xff\xff\xff\0\0\0\0"
    );
    assert_eq!(position("é\n SELECT", nata_sql::Pos { line: 2, col: 3 }), 5);
    let e = error("42601", "syntax error", Some(5));
    assert!(e.windows(3).any(|b| b == b"P5\0"));
    assert_eq!(
        description(&[("a".into(), Integer)]).expect("description")[0],
        b'T'
    );
    assert_eq!(packet(b'Z', b"I").expect("ready"), b"Z\0\0\0\x05I");
}
#[test]
fn seeded_decoder_fuzz() {
    let seed = std::env::var("NATA_PGWIRE_SEED")
        .ok()
        .map(|s| s.parse().expect("seed"))
        .unwrap_or(0x4b20261107u64);
    let cases = std::env::var("NATA_PGWIRE_CASES")
        .ok()
        .map(|s| s.parse().expect("cases"))
        .unwrap_or(10_000usize);
    eprintln!("pgwire seed={seed}, cases={cases} per corpus");
    for startup in [false, true] {
        let mut state = seed;
        for i in 0..cases {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let tag = b"QPBDECSHX"[(state as usize) % 9];
            let size = (state as usize >> 8) % 128;
            let n = [4, 2, i32::MAX, size as i32 + 4, size as i32 + 5][i % 5];
            let mut b = vec![tag];
            b.extend(n.to_be_bytes());
            for j in 0..size {
                b.push((state >> (j % 8 * 8)) as u8);
            }
            if i % 5 == 0 {
                b.truncate(5);
            }
            if startup {
                b.remove(0);
                if b.len() >= 8 {
                    b[4..8].copy_from_slice(
                        &if i % 2 == 0 { 196608i32 } else { 80877102i32 }.to_be_bytes(),
                    );
                }
            }
            let _ = decode(&b, startup);
        }
    }
}
