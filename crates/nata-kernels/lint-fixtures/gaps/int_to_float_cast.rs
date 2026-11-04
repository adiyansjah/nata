// expect: pass
// KNOWN GAP: `i64 as f64` loses precision above 2^53; cast_possible_truncation does not cover it
// (cast_precision_loss would). Not enabled because SQL DOUBLE conversion is lossy by definition.
pub fn f(a: i64) -> f64 {
    a as f64
}
