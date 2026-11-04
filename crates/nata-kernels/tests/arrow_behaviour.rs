//! Facts about arrow-rs 60.1.0 that the wrappers and the ban list rely on. If an arrow upgrade changes one of
//! them, a test here fails and the ADR 0003 assumptions are re-read.
use arrow_arith::numeric::{add, mul, neg, rem};
use arrow_array::{Array, Decimal128Array, Int64Array};
use arrow_schema::ArrowError;

#[test]
fn checked_integer_ops_report_overflow() {
    let max = Int64Array::from(vec![i64::MAX]);
    let one = Int64Array::from(vec![1i64]);
    assert!(matches!(
        add(&max, &one),
        Err(ArrowError::ArithmeticOverflow(_))
    ));
    assert!(matches!(
        mul(&max, &max),
        Err(ArrowError::ArithmeticOverflow(_))
    ));
    assert!(matches!(
        neg(&Int64Array::from(vec![i64::MIN])),
        Err(ArrowError::ArithmeticOverflow(_))
    ));
}

#[test]
fn arrow_rem_of_min_by_minus_one_is_zero_not_an_error() {
    // mathematically 0; DuckDB 2.0.0 raises an overflow error for `%` here (see the results file).
    let r = rem(
        &Int64Array::from(vec![i64::MIN]),
        &Int64Array::from(vec![-1i64]),
    )
    .expect("arrow accepts it");
    assert_eq!(
        r.as_any().downcast_ref::<Int64Array>().map(|a| a.value(0)),
        Some(0)
    );
}

#[test]
fn arrow_decimal_add_does_not_check_the_declared_precision() {
    // 10^38 - 1 is the largest DECIMAL(38,0). Adding 1 fits in i128, so arrow's "checked" add returns Ok
    // with a value that is NOT a valid DECIMAL(38,0). Decimal results must be validated by us (decimal-rules.md).
    let max38 = 10i128.pow(38) - 1;
    let a = Decimal128Array::from(vec![max38])
        .with_precision_and_scale(38, 0)
        .expect("valid");
    let b = Decimal128Array::from(vec![1i128])
        .with_precision_and_scale(38, 0)
        .expect("valid");
    let sum = add(&a, &b).expect("arrow reports no error");
    let sum = sum
        .as_any()
        .downcast_ref::<Decimal128Array>()
        .expect("decimal result");
    assert_eq!(sum.value(0), 10i128.pow(38));
    assert!(
        sum.validate_decimal_precision(38).is_err(),
        "the explicit precision check is what catches it"
    );
}

#[test]
fn arrow_decimal_mul_does_check_i128_overflow() {
    let max38 = 10i128.pow(38) - 1;
    let a = Decimal128Array::from(vec![max38])
        .with_precision_and_scale(38, 0)
        .expect("valid");
    assert!(matches!(
        mul(&a, &a),
        Err(ArrowError::ArithmeticOverflow(_))
    ));
}
