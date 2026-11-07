//! Strict SQL numeric column kernels. Type-derived factors and bounds are
//! prepared once per column; every selected, non-NULL operation stays checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    IntDiv,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    Overflow,
    DivZero,
}

pub fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}
pub fn fits(v: i128, p: u8) -> bool {
    pow10(u32::from(p)).is_some_and(|limit| v.unsigned_abs() < limit.unsigned_abs())
}
pub fn div_round_away(n: i128, d: i128) -> Option<i128> {
    let q = n.checked_div(d)?;
    let r = n.checked_rem(d)?;
    if r == 0 {
        return Some(q);
    }
    if r.unsigned_abs().checked_mul(2)? >= d.unsigned_abs() {
        if (n < 0) != (d < 0) {
            q.checked_sub(1)
        } else {
            q.checked_add(1)
        }
    } else {
        Some(q)
    }
}
pub fn rescale(v: i128, from: u8, to: u8) -> Option<i128> {
    match to.cmp(&from) {
        std::cmp::Ordering::Equal => Some(v),
        std::cmp::Ordering::Greater => v.checked_mul(pow10(u32::from(to.checked_sub(from)?))?),
        std::cmp::Ordering::Less => div_round_away(v, pow10(u32::from(from.checked_sub(to)?))?),
    }
}

/// Write only selected, jointly valid slots. Output buffers are allocated under
/// the executor's existing expression grant; unselected slots remain NULL.
#[allow(
    clippy::too_many_arguments,
    reason = "borrowed column inputs and output buffers"
)]
pub fn binary<T: Copy, U>(
    a: &[T],
    av: &[bool],
    b: &[T],
    bv: &[bool],
    sel: &[u32],
    out: &mut [U],
    valid: &mut [bool],
    mut op: impl FnMut(T, T) -> Result<U, Fail>,
) -> Result<(), Fail> {
    for &row in sel {
        let i = row as usize;
        if av[i] && bv[i] {
            out[i] = op(a[i], b[i])?;
            valid[i] = true;
        }
    }
    Ok(())
}

/// Unary conversion/negation with the same selection and NULL contract.
pub fn unary<T: Copy, U, E>(
    a: &[T],
    av: &[bool],
    sel: &[u32],
    out: &mut [U],
    valid: &mut [bool],
    mut op: impl FnMut(T) -> Result<U, E>,
) -> Result<(), E> {
    for &row in sel {
        let i = row as usize;
        if av[i] {
            out[i] = op(a[i])?;
            valid[i] = true;
        }
    }
    Ok(())
}

/// Comparison adapter: the reused comparator owns type/scale semantics.
pub fn compare<E>(
    av: &[bool],
    bv: &[bool],
    sel: &[u32],
    out: &mut [bool],
    valid: &mut [bool],
    mut op: impl FnMut(usize) -> Result<bool, E>,
) -> Result<(), E> {
    for &row in sel {
        let i = row as usize;
        if av[i] && bv[i] {
            out[i] = op(i)?;
            valid[i] = true;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub struct Decimal {
    op: Op,
    left: i128,
    right: i128,
    limit: u128,
    input_limits: Option<(u128, u128)>,
}
impl Decimal {
    pub fn new(op: Op, sa: u8, sb: u8, p: u8, s: u8) -> Result<Self, Fail> {
        let left = match op {
            Op::Div => pow10(
                u32::from(s)
                    .checked_add(u32::from(sb))
                    .and_then(|v| v.checked_sub(u32::from(sa)))
                    .ok_or(Fail::Overflow)?,
            ),
            _ => pow10(u32::from(s.saturating_sub(sa))),
        }
        .ok_or(Fail::Overflow)?;
        let right = pow10(u32::from(s.saturating_sub(sb))).ok_or(Fail::Overflow)?;
        Ok(Self {
            op,
            input_limits: None,
            left,
            right,
            limit: pow10(u32::from(p)).ok_or(Fail::Overflow)?.unsigned_abs(),
        })
    }
    /// Admit by declared precision, then validate magnitudes once per batch.
    /// Scan/Arrow metadata alone does not establish the value invariant.
    pub fn with_precisions(mut self, pa: u8, pb: u8, p: u8) -> Self {
        if self.op == Op::Mul && product_precision(pa, pb, p) {
            self.input_limits = pow10(u32::from(pa))
                .zip(pow10(u32::from(pb)))
                .map(|(a, b)| (a.unsigned_abs(), b.unsigned_abs()));
        }
        self
    }
    /// Dispatch once per column, so multiplication has neither per-row type
    /// dispatch nor a per-row power-of-ten calculation.
    #[allow(
        clippy::too_many_arguments,
        reason = "borrowed column inputs and output buffers"
    )]
    #[allow(clippy::disallowed_methods, reason = "R23: batch admission bounds the exact product")]
    pub fn column(
        self,
        a: &[i128],
        av: &[bool],
        b: &[i128],
        bv: &[bool],
        sel: &[u32],
        out: &mut [i128],
        valid: &mut [bool],
    ) -> Result<(), Fail> {
        if let Some((la, lb)) = self.input_limits
            && batch_magnitudes(a, av, b, bv, sel, la, lb)
        {
            return binary(a, av, b, bv, sel, out, valid, |x, y| {
                // Admission bounds the exact result below 10^P <= 10^38.
                // Full-width arithmetic lemma: improvements.md R23.
                Ok(x.wrapping_mul(y))
            });
        }
        match self.op {
            Op::Mul => binary(a, av, b, bv, sel, out, valid, |x, y| {
                self.check(self.product(x, y))
            }),
            Op::Add => binary(a, av, b, bv, sel, out, valid, |x, y| {
                let (x, y) = self.aligned(x, y)?;
                self.check(x.checked_add(y))
            }),
            Op::Sub => binary(a, av, b, bv, sel, out, valid, |x, y| {
                let (x, y) = self.aligned(x, y)?;
                self.check(x.checked_sub(y))
            }),
            _ => binary(a, av, b, bv, sel, out, valid, |x, y| self.scalar(x, y)),
        }
    }
    // A checked i64 product is sufficient for a Decimal64 result. If it
    // overflows i64 it also exceeds the stricter decimal limit. Wide operands
    // still use checked i128, including a wide operand multiplied by zero.
    fn product(self, x: i128, y: i128) -> Option<i128> {
        if self.limit <= 1_000_000_000_000_000_000
            && let (Ok(a), Ok(b)) = (i64::try_from(x), i64::try_from(y))
        {
            return a.checked_mul(b).map(i128::from);
        }
        x.checked_mul(y)
    }
    fn check(self, v: Option<i128>) -> Result<i128, Fail> {
        v.filter(|v| v.unsigned_abs() < self.limit)
            .ok_or(Fail::Overflow)
    }
    fn aligned(self, x: i128, y: i128) -> Result<(i128, i128), Fail> {
        Ok((
            self.check(x.checked_mul(self.left))?,
            self.check(y.checked_mul(self.right))?,
        ))
    }
    pub fn scalar(self, x: i128, y: i128) -> Result<i128, Fail> {
        match self.op {
            Op::Add | Op::Sub => {
                let (x, y) = self.aligned(x, y)?;
                self.check(if self.op == Op::Add {
                    x.checked_add(y)
                } else {
                    x.checked_sub(y)
                })
            }
            Op::Mul => self.check(self.product(x, y)),
            Op::Div => {
                if y == 0 {
                    return Err(Fail::DivZero);
                }
                self.check(x.checked_mul(self.left).and_then(|x| div_round_away(x, y)))
            }
            Op::Mod => {
                // Remainder aligns in i128, without the addition operand-fit rule.
                let x = x.checked_mul(self.left).ok_or(Fail::Overflow)?;
                let y = y.checked_mul(self.right).ok_or(Fail::Overflow)?;
                if y == 0 {
                    return Err(Fail::DivZero);
                }
                self.check(x.checked_rem(y))
            }
            Op::IntDiv => Err(Fail::Overflow),
        }
    }
}

pub fn integer(op: Op, x: i64, y: i64, lo: i64, hi: i64) -> Result<i64, Fail> {
    let v = match op {
        Op::Add => x.checked_add(y),
        Op::Sub => x.checked_sub(y),
        Op::Mul => x.checked_mul(y),
        Op::Mod | Op::IntDiv => {
            if y == 0 {
                return Err(Fail::DivZero);
            }
            if y == -1 && x == lo {
                return Err(Fail::Overflow);
            }
            if op == Op::Mod {
                x.checked_rem(y)
            } else {
                x.checked_div(y)
            }
        }
        Op::Div => return Err(Fail::Overflow),
    };
    v.filter(|v| *v >= lo && *v <= hi).ok_or(Fail::Overflow)
}

/// Sufficient type rule; capped products deliberately fail admission.
pub fn product_precision(pa: u8, pb: u8, p: u8) -> bool {
    (1..=38).contains(&pa)
        && (1..=38).contains(&pb)
        && (1..=38).contains(&p)
        && u16::from(pa).saturating_add(u16::from(pb)) <= u16::from(p)
}

/// Compare the maxima over jointly valid selected rows. A failed scan is not
/// an SQL error: the original checked loop must decide at the original row.
#[allow(
    clippy::too_many_arguments,
    reason = "borrowed batch inputs and bounds"
)]
pub fn batch_magnitudes(
    a: &[i128],
    av: &[bool],
    b: &[i128],
    bv: &[bool],
    sel: &[u32],
    la: u128,
    lb: u128,
) -> bool {
    let (mut ma, mut mb) = (0, 0);
    for &row in sel {
        let i = row as usize;
        if av[i] && bv[i] {
            ma = ma.max(a[i].unsigned_abs());
            mb = mb.max(b[i].unsigned_abs());
        }
    }
    ma < la && mb < lb
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    reason = "bounded independent scalar reference on values of magnitude at most twelve"
)]
mod tests {
    use super::*;
    #[test]
    fn bounded_columns_match_an_independent_exact_reference() {
        for op in [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Mod] {
            let kernel = Decimal::new(op, 0, 0, 2, 0).expect("type");
            for a in -12i128..=12 {
                for b in -12i128..=12 {
                    let reference = if matches!(op, Op::Div | Op::Mod) && b == 0 {
                        Err(Fail::DivZero)
                    } else {
                        let v = match op {
                            Op::Add => a + b,
                            Op::Sub => a - b,
                            Op::Mul => a * b,
                            Op::Mod => a % b,
                            _ => {
                                let q = a / b;
                                let r = a % b;
                                if r.abs() * 2 >= b.abs() {
                                    q + if (a < 0) != (b < 0) { -1 } else { 1 }
                                } else {
                                    q
                                }
                            }
                        };
                        if v.abs() >= 100 {
                            Err(Fail::Overflow)
                        } else {
                            Ok(v)
                        }
                    };
                    let (mut out, mut valid) = ([0; 1], [false; 1]);
                    let got = kernel
                        .column(&[a], &[true], &[b], &[true], &[0], &mut out, &mut valid)
                        .map(|()| out[0]);
                    assert_eq!(got, reference, "{op:?} {a} {b}");
                }
            }
        }
    }
    #[test]
    fn selected_nulls_and_strict_bounds() {
        let k = Decimal::new(Op::Mul, 2, 2, 18, 4).expect("type");
        let (mut narrow_out, mut narrow_valid) = ([0], [false]);
        assert_eq!(
            k.column(
                &[1i128 << 59],
                &[true],
                &[32],
                &[true],
                &[0],
                &mut narrow_out,
                &mut narrow_valid
            ),
            Err(Fail::Overflow)
        );
        let (mut out, mut valid) = ([0; 4], [false; 4]);
        let a = [123, i128::MAX, i128::MAX, 999_999_999_999_999_999];
        let b = [5, 2, 2, 2];
        k.column(
            &a,
            &[true, false, true, true],
            &b,
            &[true; 4],
            &[0, 1],
            &mut out,
            &mut valid,
        )
        .expect("masked");
        assert_eq!(out, [615, 0, 0, 0]);
        assert_eq!(valid, [true, false, false, false]);
        assert_eq!(
            k.column(&a, &[true; 4], &b, &[true; 4], &[2], &mut out, &mut valid),
            Err(Fail::Overflow)
        );
        assert_eq!(
            k.column(&a, &[true; 4], &b, &[true; 4], &[3], &mut out, &mut valid),
            Err(Fail::Overflow)
        );
    }
    #[test]
    fn perf2c_product_boundaries_and_fallback_prefix() {
        for pa in 1..=37 {
            for pb in 1..=38 - pa {
                let p = pa + pb;
                let k = Decimal::new(Op::Mul, 0, 0, p, 0)
                    .unwrap()
                    .with_precisions(pa, pb, p);
                for x in [
                    pow10(u32::from(pa)).unwrap() - 1,
                    1 - pow10(u32::from(pa)).unwrap(),
                    0,
                ] {
                    for y in [
                        pow10(u32::from(pb)).unwrap() - 1,
                        1 - pow10(u32::from(pb)).unwrap(),
                        0,
                    ] {
                        let (mut out, mut valid) = ([0], [false]);
                        k.column(&[x], &[true], &[y], &[true], &[0], &mut out, &mut valid)
                            .unwrap();
                        assert_eq!(out[0], x.checked_mul(y).unwrap());
                        assert!(fits(out[0], p));
                    }
                }
            }
        }
        let k = Decimal::new(Op::Mul, 0, 0, 2, 0)
            .unwrap()
            .with_precisions(1, 1, 2);
        for bad in [10, 100, i128::MIN, i128::MAX] {
            let a = [2, 3, bad];
            let b = [2, 2, 10];
            let (mut out, mut valid) = ([0; 3], [false; 3]);
            assert_eq!(
                k.column(
                    &a,
                    &[true; 3],
                    &b,
                    &[true; 3],
                    &[0, 1, 2],
                    &mut out,
                    &mut valid
                ),
                Err(Fail::Overflow)
            );
            assert_eq!(out, [4, 6, 0]);
            assert_eq!(valid, [true, true, false]);
            k.column(
                &a,
                &[true; 3],
                &b,
                &[true; 3],
                &[0, 1],
                &mut out,
                &mut valid,
            )
            .unwrap();
        }
        assert!(!product_precision(1, 2, 2)); // weakened P+1 rule would admit 9*99.
        assert!(9 * 99 >= 100);
    }
}
