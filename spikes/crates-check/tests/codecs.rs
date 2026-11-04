//! Every codec the Parquet reader and our spill path may use: round trip on 64 MiB, plus the sizes and the speed
//! (printed, not asserted: a speed number from a shared machine is not evidence, but a 10x difference between
//! architectures would show in the log).
use spike_crates_check::{cpu_report, test_data};
use std::io::{Read, Write};
use std::time::Instant;

const N: usize = 64 << 20;

fn report(name: &str, raw: usize, packed: usize, enc: f64, dec: f64) {
    println!("{name:<10} {raw} -> {packed} bytes ({:.2}x)  encode {:.0} MB/s  decode {:.0} MB/s", raw as f64 / packed as f64, raw as f64 / 1e6 / enc, raw as f64 / 1e6 / dec);
}

#[test]
fn the_cpu_report_prints() {
    println!("{}", cpu_report());
}

#[test]
fn snappy_round_trips() {
    let d = test_data(N, 1);
    let t = Instant::now();
    let c = snap::raw::Encoder::new().compress_vec(&d).unwrap();
    let e = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let r = snap::raw::Decoder::new().decompress_vec(&c).unwrap();
    assert!(r == d, "snappy round trip differs");
    report("snappy", d.len(), c.len(), e, t.elapsed().as_secs_f64());
}

#[test]
fn lz4_round_trips() {
    let d = test_data(N, 2);
    let t = Instant::now();
    let c = lz4_flex::block::compress(&d);
    let e = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let r = lz4_flex::block::decompress(&c, d.len()).unwrap();
    assert!(r == d, "lz4 round trip differs");
    report("lz4_flex", d.len(), c.len(), e, t.elapsed().as_secs_f64());
}

#[test]
fn gzip_round_trips() {
    let d = test_data(N / 4, 3);
    let t = Instant::now();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&d).unwrap();
    let c = enc.finish().unwrap();
    let e = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let mut r = Vec::new();
    flate2::read::GzDecoder::new(&c[..]).read_to_end(&mut r).unwrap();
    assert!(r == d, "gzip round trip differs");
    report("gzip", d.len(), c.len(), e, t.elapsed().as_secs_f64());
}

#[test]
fn zstd_round_trips_at_three_levels() {
    let d = test_data(N, 4);
    for level in [1, 3, 9] {
        let t = Instant::now();
        let c = zstd::bulk::compress(&d, level).unwrap();
        let e = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let r = zstd::bulk::decompress(&c, d.len()).unwrap();
        assert!(r == d, "zstd level {level} round trip differs");
        report(&format!("zstd-{level}"), d.len(), c.len(), e, t.elapsed().as_secs_f64());
    }
}

#[test]
fn corrupt_input_is_an_error_not_a_crash() {
    let d = test_data(1 << 20, 5);
    let mut c = zstd::bulk::compress(&d, 3).unwrap();
    let mid = c.len() / 2;
    c[mid] ^= 0xFF;
    c.truncate(c.len() - 5);
    assert!(zstd::bulk::decompress(&c, d.len()).is_err() || zstd::bulk::decompress(&c, d.len()).unwrap() != d);
    let mut s = snap::raw::Encoder::new().compress_vec(&d).unwrap();
    s.truncate(s.len() / 2);
    assert!(snap::raw::Decoder::new().decompress_vec(&s).is_err());
    assert!(lz4_flex::block::decompress(&[0xF0, 0x01, 0x02], 1 << 20).is_err());
}
