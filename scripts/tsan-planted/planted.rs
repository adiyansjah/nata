#![feature(cfg_sanitize)]
#![cfg(all(test, sanitize = "thread"))]

use std::cell::UnsafeCell;
use std::sync::{Arc, Barrier};

struct DeliberateRace(UnsafeCell<u64>);

// Deliberately WRONG: this crate exists only to verify that TSan rejects it.
unsafe impl Sync for DeliberateRace {}

impl DeliberateRace {
    #[inline(never)]
    fn nata_unsynchronized_write(&self, value: u64) {
        // Deliberately WRONG: the barrier orders the start, not these writes.
        // Volatile keeps the writes present; it provides no synchronization.
        unsafe { self.0.get().write_volatile(value) };
    }
}

#[test]
fn nata_known_bad_two_writers() {
    let value = Arc::new(DeliberateRace(UnsafeCell::new(0)));
    let start = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        for writer in 1..=2 {
            let value = Arc::clone(&value);
            let start = Arc::clone(&start);
            scope.spawn(move || {
                start.wait();
                for _ in 0..1_000 {
                    value.nata_unsynchronized_write(writer);
                }
            });
        }
    });
}
