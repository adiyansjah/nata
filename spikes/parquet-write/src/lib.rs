//! Spike (verification-plan item 30): the write path. `data` makes adversarial tables, `write` is the writer wrapper (options, sorting, rolling
//! files), `verify` is an INDEPENDENT checker of what was written (it decodes the file and recomputes the truth), `faulty` writes files with known
//! bad statistics to show the checker fails on them.
pub mod data;
pub mod faulty;
pub mod verify;
pub mod write;

/// xorshift64*, so every table is reproducible from its seed.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    pub fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}
