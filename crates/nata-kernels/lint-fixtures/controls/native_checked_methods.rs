// expect: pass
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> Result<i64, arrow_schema::ArrowError> { x.add_checked(1)?.mul_checked(2) }
