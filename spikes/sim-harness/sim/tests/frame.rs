use nata_sim::frame::*;

fn samples() -> Vec<Msg> {
    vec![
        Msg::Assign { task: 3, attempt: 2, file: 3 },
        Msg::Ack { task: 1, attempt: 0, next: 9, stale: true },
        Msg::Ack { task: 1, attempt: 0, next: 9, stale: false },
        Msg::Cancel,
        Msg::Shutdown,
        Msg::HbAck,
        Msg::Heartbeat { born: 12_345_678_901 },
        Msg::Data { task: 4, attempt: 1, seq: 7, last: false, rows: vec![1, -2, i64::MAX, i64::MIN] },
        Msg::Data { task: 4, attempt: 1, seq: 8, last: true, rows: vec![] },
        Msg::CancelAck { born: 5 },
        Msg::Bye { born: 6 },
        Msg::ClientCancel,
    ]
}

#[test]
fn every_message_round_trips() {
    for m in samples() {
        assert_eq!(Msg::decode(&m.encode()), Ok(m.clone()), "{m:?}");
    }
}

#[test]
fn crc32_matches_the_standard_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn any_single_flipped_bit_is_rejected() {
    for m in samples() {
        let f = m.encode();
        for bit in 0..f.len() * 8 {
            let mut g = f.clone();
            g[bit / 8] ^= 1 << (bit % 8);
            assert!(Msg::decode(&g).is_err(), "{m:?}: flipping bit {bit} was not detected");
        }
    }
}

#[test]
fn truncation_and_trailing_bytes_are_rejected() {
    for m in samples() {
        let f = m.encode();
        for cut in 0..f.len() {
            assert!(Msg::decode(&f[..cut]).is_err(), "{m:?} cut at {cut}");
        }
        let mut g = f.clone();
        g.push(0);
        assert!(Msg::decode(&g).is_err(), "{m:?} with a trailing byte");
    }
}

#[test]
fn i64_columns_round_trip() {
    let v = vec![0, 1, -1, i64::MAX, i64::MIN, 42];
    assert_eq!(decode_i64s(&encode_i64s(&v)), v);
}
