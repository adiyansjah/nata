// expect: pass
// By design: u64 wrapping is not banned because hashing needs it. Review must keep it out of value arithmetic.
pub fn f(h: u64, x: u64) -> u64 {
    h.wrapping_mul(31).wrapping_add(x)
}
