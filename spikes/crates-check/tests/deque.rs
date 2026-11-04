//! crossbeam-deque: the work-stealing deque the scheduler of ADR 0002 is built on. Every pushed item must be taken
//! exactly once, by the owner or by a stealer, whatever the number of threads. The memory-ordering code inside is
//! where x86-64 (strong ordering) and ARM64 (weak ordering) can differ, so this runs many repetitions.
use crossbeam_deque::{Steal, Worker};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::Arc;

fn one_round(items: u64, stealers: usize, owner_pops: bool) {
    let w = Worker::new_fifo();
    let sts: Vec<_> = (0..stealers).map(|_| w.stealer()).collect();
    let sum = Arc::new(AtomicU64::new(0));
    let count = Arc::new(AtomicU64::new(0));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hs: Vec<_> = sts
        .into_iter()
        .map(|s| {
            let (sum, count, done) = (sum.clone(), count.clone(), done.clone());
            std::thread::spawn(move || loop {
                match s.steal() {
                    Steal::Success(x) => {
                        sum.fetch_add(x, SeqCst);
                        count.fetch_add(1, SeqCst);
                    }
                    Steal::Retry => {}
                    Steal::Empty => {
                        if done.load(SeqCst) && s.is_empty() {
                            break;
                        }
                        std::hint::spin_loop();
                    }
                }
            })
        })
        .collect();
    for i in 1..=items {
        w.push(i);
        if owner_pops && i % 3 == 0 {
            if let Some(x) = w.pop() {
                sum.fetch_add(x, SeqCst);
                count.fetch_add(1, SeqCst);
            }
        }
    }
    while let Some(x) = w.pop() {
        sum.fetch_add(x, SeqCst);
        count.fetch_add(1, SeqCst);
    }
    done.store(true, SeqCst);
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(count.load(SeqCst), items, "an item was lost or taken twice (stealers={stealers})");
    assert_eq!(sum.load(SeqCst), items * (items + 1) / 2, "the sum of the items differs (stealers={stealers})");
}

#[test]
fn every_item_is_taken_exactly_once() {
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    for stealers in [1, 2, cores, cores * 2] {
        for round in 0..200 {
            one_round(20_000, stealers, round % 2 == 0);
        }
    }
}
