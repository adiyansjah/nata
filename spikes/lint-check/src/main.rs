use arrow::array::{Int32Array, Int64Array};
use arrow::compute::kernels::numeric::{add, add_wrapping, mul_wrapping, sub_wrapping};
use arrow::compute::{cast, cast_with_options, sum, sum_checked, CastOptions};
use arrow::datatypes::DataType;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("overflow") {
        // Runtime probe: does `i32::MAX + 1` panic in this build?
        let v = std::hint::black_box(i32::MAX);
        println!("{}", v + std::hint::black_box(1));
        return;
    }
    let a = Int32Array::from(vec![1]);
    let b = Int64Array::from(vec![1i64 << 40]);

    // Each line below is expected to be flagged (E) or accepted (OK).
    let _ = add(&a, &a);                       // OK   checked add
    let _ = add_wrapping(&a, &a);              // E1   wrapping add
    let _ = sub_wrapping(&a, &a);              // E2   wrapping sub
    let _ = mul_wrapping(&a, &a);              // E3   wrapping mul
    let _ = sum(&b);                           // E4   plain sum
    let _ = sum_checked(&b);                   // OK   checked sum
    let _ = cast(&b, &DataType::Int32);        // E5   default-options cast
    let o = CastOptions { safe: false, ..Default::default() };  // E6   CastOptions::default via ..Default::default()
    let _ = cast_with_options(&b, &DataType::Int32, &o);        // E7   raw cast_with_options
    let _ = CastOptions::default();            // E8   explicit CastOptions::default()
    let _ = CastOptions { safe: true, ..CastOptions { safe: false, format_options: Default::default() } }; // E9  safe:true literal (a lint cannot see this)

    // Plain Rust arithmetic and casts, the spike's own bug class:
    let x: i128 = 1 << 100;
    let y = x as i64;                          // E10  truncating cast
    let z = y + 1;                             // E11  unchecked add
    let w = z * 2;                             // E12  unchecked mul
    println!("{y} {z} {w}");
}
