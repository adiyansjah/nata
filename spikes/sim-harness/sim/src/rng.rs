//! Seeded randomness. Everything random in the harness comes from here, so a seed fixes the run.
//! SplitMix64: integer only (no floats, so no difference between architectures or build profiles).

#[derive(Clone, Debug)]
pub struct Rng(u64);

pub fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x1234_5678_9ABC_DEF1);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// An independent stream derived from a seed and a label, so adding a draw in one place (the network,
    /// say) never shifts the numbers another place (the store, the executor) sees.
    pub fn stream(seed: u64, tag: u64) -> Self {
        Rng(mix(seed, tag))
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish in `0..n` (modulo bias is irrelevant here); 0 when `n == 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    /// Inclusive range.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    /// True with probability `ppm` / 1,000,000. Draws nothing when `ppm == 0`.
    pub fn chance_ppm(&mut self, ppm: u32) -> bool {
        ppm > 0 && self.next() % 1_000_000 < ppm as u64
    }
}
