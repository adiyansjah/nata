//! Minimal stand-in for the one arrow type kernels.rs uses. ASSUMPTION (not proven): arrow's
//! `NullBuffer::is_valid(i)` returns the validity of slot i and panics for i >= len, and `null_count()` is the
//! number of invalid slots. The stub asserts `i < len` so a proof also shows the kernels never read past the end.
pub mod buffer {
    pub struct NullBuffer {
        pub bits: [bool; 8],
        pub len: usize,
    }
    impl NullBuffer {
        pub fn is_valid(&self, i: usize) -> bool {
            assert!(i < self.len);
            self.bits[i]
        }
        pub fn null_count(&self) -> usize {
            (0..self.len).filter(|&i| !self.bits[i]).count()
        }
    }
}
