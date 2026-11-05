//! Tests of the real TCP exchange on loopback. Each failure the protocol must survive is tested against a REAL misbehaving peer (a raw
//! socket that writes what a buggy or crashed peer would write), and `mutants/run.sh` re-runs the whole file against deliberately broken builds.
//! Every test that could hang runs under a timeout: a hang is a failure, not a slow test.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use spike_net_exchange::flow::{Config, NetError, Receiver, Sender};
use spike_net_exchange::shuffle::{NodeError, Params, expected, run_node};
use spike_net_exchange::wire::{self, Frame, HEADER, MAGIC, MAX_PAYLOAD, WireError, read_frame, write_frame};

fn within<T: Send + 'static>(secs: u64, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs)).unwrap_or_else(|_| panic!("TIMED OUT after {secs} s (a hang): {what}"))
}

fn cfg(credit: u64) -> Config {
    Config { credit_bytes: credit, dead_after: Duration::from_millis(1500), ping_every: Duration::from_millis(100) }
}

/// A listener, plus a connected (Sender, Receiver) pair.
fn pair(c: &Config) -> (Sender, Receiver) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let c2 = c.clone();
    let h = thread::spawn(move || Sender::start(TcpStream::connect(addr).unwrap(), 7, 99, &c2).unwrap());
    let rx = Receiver::accept(&l, c).unwrap();
    (h.join().unwrap(), rx)
}

/// A receiver whose peer is a raw socket the test controls.
fn raw_to_receiver(c: &Config) -> (TcpStream, Receiver) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let h = thread::spawn(move || TcpStream::connect(addr).unwrap());
    let c2 = c.clone();
    let rx = thread::spawn(move || Receiver::accept(&l, &c2));
    let mut raw = h.join().unwrap();
    write_frame(&mut raw, &Frame::Hello { src: 1, query: 5 }).unwrap();
    (raw, rx.join().unwrap().unwrap_or_else(|e| panic!("the receiver could not start: {e}")))
}

fn batch(i: u64, len: usize) -> Vec<u8> {
    (0..len).map(|j| (i as usize * 31 + j) as u8).collect()
}

// ------------------------------------------------------------------------------------------------------------------------ wire

#[test]
fn wire_round_trips_every_kind() {
    let frames = vec![
        Frame::Hello { src: 3, query: u64::MAX },
        Frame::Data { seq: 9, payload: vec![1, 2, 3] },
        Frame::Data { seq: 0, payload: vec![] },
        Frame::Eos { count: 12 },
        Frame::Credit { bytes: 1 << 40 },
        Frame::Cancel,
        Frame::Ping { n: 5 },
        Frame::Pong { n: 6 },
    ];
    let mut buf = Vec::new();
    for f in &frames {
        f.encode(&mut buf);
    }
    let mut r = &buf[..];
    for f in &frames {
        assert_eq!(&read_frame(&mut r).unwrap().unwrap(), f);
    }
    assert!(read_frame(&mut r).unwrap().is_none(), "clean end between frames is None");
}

/// A reader that returns ONE byte per call, like a slow network.
struct OneByte<'a>(&'a [u8]);
impl Read for OneByte<'_> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.0.is_empty() || b.is_empty() {
            return Ok(0);
        }
        b[0] = self.0[0];
        self.0 = &self.0[1..];
        Ok(1)
    }
}

#[test]
fn wire_decodes_when_the_network_delivers_one_byte_at_a_time() {
    let mut buf = Vec::new();
    Frame::Data { seq: 4, payload: batch(4, 300) }.encode(&mut buf);
    Frame::Eos { count: 5 }.encode(&mut buf);
    let mut r = OneByte(&buf);
    assert_eq!(read_frame(&mut r).unwrap().unwrap(), Frame::Data { seq: 4, payload: batch(4, 300) });
    assert_eq!(read_frame(&mut r).unwrap().unwrap(), Frame::Eos { count: 5 });
}

#[test]
fn wire_truncated_frame_is_an_error_not_a_clean_end() {
    let mut buf = Vec::new();
    Frame::Data { seq: 1, payload: vec![7; 100] }.encode(&mut buf);
    for cut in [1, HEADER - 1, HEADER, HEADER + 50, buf.len() - 1] {
        let mut r = &buf[..cut];
        assert!(matches!(read_frame(&mut r), Err(WireError::Truncated)), "cut at {cut}");
    }
}

fn header(magic: u8, kind: u8, seq: u64, len: u32) -> Vec<u8> {
    let mut h = vec![magic, kind];
    h.extend_from_slice(&seq.to_le_bytes());
    h.extend_from_slice(&len.to_le_bytes());
    h
}

#[test]
fn wire_rejects_bad_magic_unknown_kind_and_bad_control_lengths() {
    assert!(matches!(read_frame(&mut &header(0x00, 2, 0, 0)[..]), Err(WireError::BadMagic(0))));
    assert!(matches!(read_frame(&mut &header(MAGIC, 0, 0, 0)[..]), Err(WireError::BadKind(0))));
    assert!(matches!(read_frame(&mut &header(MAGIC, 200, 0, 0)[..]), Err(WireError::BadKind(200))));
    let mut credit_with_payload = header(MAGIC, 4, 5, 3);
    credit_with_payload.extend_from_slice(&[1, 2, 3]);
    assert!(matches!(read_frame(&mut &credit_with_payload[..]), Err(WireError::BadLength { kind: 4, len: 3 })));
    assert!(matches!(read_frame(&mut &header(MAGIC, 1, 1, 4)[..]), Err(WireError::BadLength { kind: 1, .. })), "Hello must be 8 bytes");
}

#[test]
fn wire_rejects_an_oversize_length_before_reading_or_allocating_it() {
    // only the header is present: a decoder that believed the length would report Truncated after trying to read 64 MiB + 1
    let r = read_frame(&mut &header(MAGIC, 2, 0, (MAX_PAYLOAD + 1) as u32)[..]);
    assert!(matches!(r, Err(WireError::TooLarge(_))), "{r:?}");
    let r = read_frame(&mut &header(MAGIC, 2, 0, u32::MAX)[..]);
    assert!(matches!(r, Err(WireError::TooLarge(_))), "{r:?}");
    let _ = wire::MAGIC;
}

// ------------------------------------------------------------------------------------------------------------------------ stream

#[test]
fn a_stream_arrives_complete_and_in_order_even_when_it_is_many_times_the_credit() {
    within(20, "round trip", || {
        let c = cfg(64 << 10); // 64 KiB of credit, 8 MiB of data
        let (mut tx, rx) = pair(&c);
        let sender = thread::spawn(move || {
            for i in 0..2000u64 {
                tx.send_batch(&batch(i, 4096)).unwrap();
            }
            tx.finish().unwrap()
        });
        let mut n = 0u64;
        while let Some(b) = rx.next().unwrap() {
            assert_eq!(b, batch(n, 4096), "batch {n}");
            n += 1;
        }
        assert_eq!(n, 2000);
        let st = sender.join().unwrap();
        assert_eq!(st.batches, 2000);
        assert_eq!(rx.stats().bytes, 2000 * 4096);
    });
}

#[test]
fn backpressure_keeps_buffered_and_in_flight_bytes_within_the_credit_for_a_slow_consumer() {
    let credit = 64u64 << 10;
    let (peak_buf, peak_fly, waits) = within(30, "backpressure", move || {
        let c = cfg(credit);
        let (mut tx, rx) = pair(&c);
        let sender = thread::spawn(move || {
            for i in 0..1500u64 {
                tx.send_batch(&batch(i, 4096)).unwrap();
            }
            tx.finish().unwrap()
        });
        let mut n = 0;
        while let Some(_b) = rx.next().unwrap() {
            n += 1;
            thread::sleep(Duration::from_micros(300));
        }
        assert_eq!(n, 1500);
        let st = sender.join().unwrap();
        (rx.stats().peak_buffered, st.peak_inflight, st.blocked_waits)
    });
    assert!(peak_buf <= credit, "receiver queue peaked at {peak_buf} bytes, above the credit of {credit}");
    assert!(peak_fly <= credit, "in-flight peaked at {peak_fly} bytes, above the credit of {credit}");
    assert!(peak_buf >= 4096 && peak_fly >= 4096, "the measurement saw nothing ({peak_buf}, {peak_fly})");
    assert!(waits > 0, "the sender never had to wait: the consumer was not slow enough to test anything");
}

#[test]
fn control_with_a_huge_credit_the_same_slow_consumer_does_buffer_more_than_the_small_credit() {
    // If this did not exceed 64 KiB, the test above could not tell a working bound from a stream that never gets ahead of its consumer.
    let peak = within(30, "control", || {
        let c = cfg(256 << 20);
        let (mut tx, rx) = pair(&c);
        let sender = thread::spawn(move || {
            for i in 0..1500u64 {
                tx.send_batch(&batch(i, 4096)).unwrap();
            }
            tx.finish().unwrap();
        });
        while let Some(_b) = rx.next().unwrap() {
            thread::sleep(Duration::from_micros(300));
        }
        sender.join().unwrap();
        rx.stats().peak_buffered
    });
    assert!(peak > (64 << 10), "control peak {peak}");
}

#[test]
fn a_batch_that_can_never_fit_the_credit_is_refused_not_a_deadlock() {
    within(10, "too big", || {
        let (mut tx, _rx) = pair(&cfg(1000));
        assert!(matches!(tx.send_batch(&vec![0u8; 1001]), Err(NetError::TooBig { .. })));
    });
}

#[test]
fn cancel_stops_a_sender_that_is_blocked_waiting_for_credit() {
    let (elapsed, sent) = within(20, "cancel", || {
        let c = cfg(32 << 10);
        let (mut tx, rx) = pair(&c);
        let sender = thread::spawn(move || {
            let mut sent = 0u64;
            for i in 0..1_000_000u64 {
                match tx.send_batch(&batch(i, 4096)) {
                    Ok(()) => sent += 1,
                    Err(e) => return (Some(e), sent, Instant::now()),
                }
            }
            (None, sent, Instant::now())
        });
        for _ in 0..10 {
            assert!(rx.next().unwrap().is_some());
        }
        // let the sender run into the end of its credit and block
        thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        rx.cancel();
        let (err, sent, when) = sender.join().unwrap();
        assert_eq!(err, Some(NetError::Cancelled), "the sender must be told, not just time out");
        assert!(matches!(rx.next(), Err(NetError::Cancelled)));
        (when.duration_since(t), sent)
    });
    assert!(elapsed < Duration::from_millis(500), "the blocked sender needed {elapsed:?} to notice the cancel");
    assert!(sent <= 10 + 8 + 2, "the sender sent {sent} batches although the consumer took 10 and the credit is 8 batches");
}

#[test]
fn a_sender_that_crashes_mid_stream_is_an_error_at_once_never_a_finished_stream() {
    within(10, "crash", || {
        let c = cfg(1 << 20);
        let (mut tx, rx) = pair(&c);
        tx.send_batch(&batch(0, 1000)).unwrap();
        tx.send_batch(&batch(1, 1000)).unwrap();
        tx.kill(); // no Eos
        let t = Instant::now();
        let r = loop {
            match rx.next() {
                Ok(Some(_)) => continue,
                other => break other,
            }
        };
        assert!(matches!(r, Err(NetError::PeerLost(_))), "a stream cut without Eos must not look complete: {r:?}");
        assert!(t.elapsed() < Duration::from_millis(500), "needed {:?} to notice the closed connection", t.elapsed());
    });
}

#[test]
fn a_silent_peer_with_an_open_connection_is_detected_by_the_timeout() {
    // a partition or a hung process: the socket stays open, nothing arrives. Only the liveness timer can notice.
    let c = Config { credit_bytes: 1 << 20, dead_after: Duration::from_millis(400), ping_every: Duration::from_millis(100) };
    let waited = within(10, "silent peer", move || {
        let (_raw, rx) = raw_to_receiver(&c); // _raw keeps the connection open and says nothing more
        let t = Instant::now();
        let r = rx.next();
        let e = t.elapsed();
        assert!(matches!(r, Err(NetError::PeerLost(ref s)) if s.contains("silent")), "{r:?}");
        e
    });
    assert!(waited >= Duration::from_millis(350) && waited < Duration::from_secs(2), "detected after {waited:?}");
}

#[test]
fn a_peer_that_answers_pings_but_sends_no_data_is_not_declared_dead() {
    // the other side of the liveness rule: a slow producer is alive
    within(10, "pings keep a quiet stream alive", || {
        let c = Config { credit_bytes: 1 << 20, dead_after: Duration::from_millis(400), ping_every: Duration::from_millis(100) };
        let (mut tx, rx) = pair(&c);
        let h = thread::spawn(move || {
            thread::sleep(Duration::from_millis(1200)); // three times dead_after of silence from the application
            tx.send_batch(&batch(0, 100)).unwrap();
            tx.finish().unwrap();
        });
        assert_eq!(rx.next().unwrap().unwrap(), batch(0, 100));
        assert!(rx.next().unwrap().is_none());
        h.join().unwrap();
    });
}

#[test]
fn a_gap_a_repeat_or_a_reorder_of_batch_numbers_is_detected() {
    for seqs in [vec![0u64, 2], vec![0, 0], vec![1], vec![0, 1, 3]] {
        let s2 = seqs.clone();
        within(10, "reorder", move || {
            let (mut raw, rx) = raw_to_receiver(&cfg(1 << 20));
            for q in &s2 {
                write_frame(&mut raw, &Frame::Data { seq: *q, payload: vec![1; 10] }).unwrap();
            }
            let r = loop {
                match rx.next() {
                    Ok(Some(_)) => continue,
                    other => break other,
                }
            };
            assert!(matches!(r, Err(NetError::Protocol(ref m)) if m.contains("out of order")), "{s2:?} gave {r:?}");
        });
    }
}

#[test]
fn an_end_of_stream_with_the_wrong_batch_count_is_an_error() {
    within(10, "eos count", || {
        let (mut raw, rx) = raw_to_receiver(&cfg(1 << 20));
        write_frame(&mut raw, &Frame::Data { seq: 0, payload: vec![1; 10] }).unwrap();
        write_frame(&mut raw, &Frame::Eos { count: 5 }).unwrap();
        let r = loop {
            match rx.next() {
                Ok(Some(_)) => continue,
                other => break other,
            }
        };
        assert!(matches!(r, Err(NetError::Protocol(_))), "{r:?}");
    });
}

#[test]
fn a_sender_that_ignores_its_credit_is_cut_off() {
    within(10, "credit violation", || {
        let (mut raw, rx) = raw_to_receiver(&cfg(10_000));
        for q in 0..5u64 {
            write_frame(&mut raw, &Frame::Data { seq: q, payload: vec![0; 4000] }).unwrap();
        }
        // the consumer must not be able to keep up and hide the violation: it starts only after all 20,000 bytes (credit 10,000) are in
        thread::sleep(Duration::from_millis(300));
        let r = loop {
            match rx.next() {
                Ok(Some(_)) => continue,
                other => break other,
            }
        };
        assert!(matches!(r, Err(NetError::Protocol(ref m)) if m.contains("credit")), "{r:?}");
    });
}

#[test]
fn garbage_on_the_wire_is_a_protocol_error_not_a_panic_or_a_hang() {
    for junk in [b"GET / HTTP/1.1\r\n\r\n".to_vec(), vec![0xFF; 64], header(MAGIC, 2, 0, u32::MAX)] {
        within(10, "garbage", move || {
            let (mut raw, rx) = raw_to_receiver(&cfg(1 << 20));
            raw.write_all(&junk).unwrap();
            let r = rx.next();
            assert!(matches!(r, Err(NetError::Protocol(_))), "{r:?}");
        });
    }
}

#[test]
fn a_connection_that_never_sends_hello_does_not_hang_the_accepting_side_forever() {
    let c = Config { credit_bytes: 1 << 20, dead_after: Duration::from_millis(400), ping_every: Duration::from_millis(100) };
    within(10, "no hello", move || {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let _silent = TcpStream::connect(addr).unwrap();
        let t = Instant::now();
        let r = Receiver::accept(&l, &c);
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(3));
    });
}

// ------------------------------------------------------------------------------------------------------------------------ shuffle

fn run_shuffle(p: Params) -> Vec<Result<spike_net_exchange::shuffle::NodeResult, NodeError>> {
    let n = p.nodes as usize;
    let listeners: Vec<TcpListener> = (0..n).map(|_| TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    let addrs: Vec<SocketAddr> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
    let hs: Vec<_> = listeners
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let (addrs, p) = (addrs.clone(), p.clone());
            thread::spawn(move || run_node(i as u32, l, &addrs, &p))
        })
        .collect();
    hs.into_iter().map(|h| h.join().unwrap()).collect()
}

#[test]
fn a_four_node_shuffle_gives_every_node_exactly_its_rows() {
    let results = within(60, "shuffle", || {
        let mut p = Params::new(4, 200_000);
        p.cfg = cfg(128 << 10);
        let r = run_shuffle(p.clone());
        (p, r)
    });
    let (p, results) = results;
    let mut total = 0;
    for (i, r) in results.into_iter().enumerate() {
        let r = r.unwrap_or_else(|e| panic!("node {i}: {e}"));
        let (rows, sum) = expected(i as u32, &p);
        assert_eq!((r.rows_in, r.checksum), (rows, sum), "node {i}");
        assert!(r.peak_buffered <= 128 << 10 && r.peak_inflight <= 128 << 10, "node {i}: {r:?}");
        total += r.rows_in;
    }
    assert_eq!(total, 4 * 200_000, "no row lost or duplicated overall");
}

#[test]
fn a_shuffle_with_a_slow_consumer_is_still_exact_and_bounded() {
    let (p, results) = within(60, "slow shuffle", || {
        let mut p = Params::new(3, 60_000);
        p.cfg = cfg(32 << 10);
        p.batch_rows = 3 * 512;
        p.consumer_delay = Duration::from_micros(200);
        let r = run_shuffle(p.clone());
        (p, r)
    });
    let mut waited = 0;
    for (i, r) in results.into_iter().enumerate() {
        let r = r.unwrap_or_else(|e| panic!("node {i}: {e}"));
        assert_eq!((r.rows_in, r.checksum), expected(i as u32, &p), "node {i}");
        assert!(r.peak_buffered <= 32 << 10 && r.peak_inflight <= 32 << 10, "node {i}: {r:?}");
        waited += r.blocked_waits;
    }
    assert!(waited > 0, "nobody ever waited for credit");
}

#[test]
fn when_one_node_crashes_every_other_node_fails_fast_instead_of_hanging_or_returning_a_wrong_answer() {
    let (results, secs) = within(60, "crash", || {
        let mut p = Params::new(4, 4_000_000);
        p.cfg = cfg(64 << 10);
        p.batch_rows = 4 * 1024; // about 16 KiB per destination, well inside the 64 KiB credit
        p.die_at = Some((3, 20));
        let t = Instant::now();
        let r = run_shuffle(p);
        (r, t.elapsed().as_secs_f64())
    });
    for (i, r) in results.iter().enumerate() {
        match (i, r) {
            (3, Err(NodeError::Died)) => {}
            (3, other) => panic!("the crashed node returned {other:?}"),
            (_, Err(NodeError::Net(NetError::PeerLost(_) | NetError::Cancelled))) => {}
            (_, other) => panic!("node {i} did not fail with PeerLost or Cancelled: {other:?}"),
        }
    }
    assert!(secs < 10.0, "the query needed {secs:.1} s to fail");
}

#[test]
fn when_a_consumer_cancels_the_senders_stop_and_nothing_hangs() {
    let (results, secs) = within(60, "cancel", || {
        let mut p = Params::new(3, 50_000_000);
        p.cfg = cfg(64 << 10);
        p.batch_rows = 3 * 1024;
        p.cancel_at = Some((0, 40));
        let t = Instant::now();
        let r = run_shuffle(p);
        (r, t.elapsed().as_secs_f64())
    });
    assert!(matches!(results[0], Err(NodeError::Net(NetError::Cancelled))), "{:?}", results[0]);
    for (i, r) in results.iter().enumerate().skip(1) {
        assert!(matches!(r, Err(NodeError::Net(NetError::Cancelled | NetError::PeerLost(_)))), "node {i}: {r:?}");
    }
    assert!(secs < 10.0, "50 million rows per node were not cut short: {secs:.1} s");
}
