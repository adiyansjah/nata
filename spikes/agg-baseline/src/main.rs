#[cfg(nata_loom)]
fn main() {}
#[cfg(not(nata_loom))]
fn main() -> agg_baseline::spill::R<()> {
    use agg_baseline::{spill::*, work::*};
    use std::{hint::black_box, time::Instant};
    let root = std::path::PathBuf::from(std::env::var("TMPDIR").expect("on-disk TMPDIR required"));
    let states = generated(5, 1024);
    println!(
        "local, indicative only; warm page cache; buffered writes, no fsync; seed=5 rows=1024 rounds=7"
    );
    let path = root.join(format!("agg-bench-{}", std::process::id()));
    for f in [Format::Ipc, Format::RecordControl] {
        write(&path, &states, f)?;
        assert_eq!(read(&path, 8 * MAX_PAYLOAD)?, states);
        std::fs::remove_file(&path)?;
    }
    for round in 0..7 {
        let fs = [Format::Ipc, Format::RecordControl, Format::Ipc];
        for f in fs {
            let start = Instant::now();
            let bytes = write(&path, &states, f)?;
            let w = start.elapsed().as_secs_f64();
            let start = Instant::now();
            let got = read(&path, 8 * MAX_PAYLOAD)?;
            let r = start.elapsed().as_secs_f64();
            assert_eq!(got, states);
            std::fs::remove_file(&path)?;
            println!(
                "spill,{round},{f:?},bytes={bytes},write_ms={:.4},read_ms={:.4},write_MiBs={:.2},read_MiBs={:.2}",
                w * 1000.,
                r * 1000.,
                bytes as f64 / w / 1048576.,
                bytes as f64 / r / 1048576.
            );
        }
    }
    let input = rows(8192, false);
    let oracle = std_control(&input);
    let mut reused = ReusedTable::default();
    assert_eq!(reused.aggregate(&input), oracle);
    for round in 0..7 {
        for arm in if round % 2 == 0 {
            [0, 1, 2, 3]
        } else {
            [3, 2, 1, 0]
        } {
            let start = Instant::now();
            for _ in 0..16 {
                let out = if arm == 0 || arm == 3 {
                    std_control(&input)
                } else if arm == 1 {
                    ReusedTable::default().aggregate(&input)
                } else {
                    reused.aggregate(&input)
                };
                assert_eq!(black_box(out), oracle);
            }
            println!(
                "table,{round},{arm},ms={:.4},capacity={}",
                start.elapsed().as_secs_f64() * 1000.,
                reused.capacity()
            );
        }
    }
    let input = rows(4096, false);
    for round in 0..7 {
        let arms = [
            (Queue::Fifo, false),
            (Queue::Deque, false),
            (Queue::Fifo, true),
            (Queue::Deque, true),
            (Queue::Fifo, false),
        ];
        for i in 0..5 {
            let idx = if round % 2 == 0 { i } else { 4 - i };
            let (q, p) = arms[idx];
            let (ms, c) = schedule(q, p, 4, 64, &input);
            println!(
                "schedule,{round},{q:?},affinity={p},twin={},ms={ms:.4},{c:?}",
                idx == 4
            );
        }
    }
    Ok(())
}
