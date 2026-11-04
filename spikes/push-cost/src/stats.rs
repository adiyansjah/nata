//! Quartiles and the verdict rule. Pure functions, unit-tested with known data.

#[derive(Clone, Copy, Debug)]
pub struct Summary {
    pub n: usize,
    pub min: f64,
    pub q1: f64,
    pub median: f64,
    pub q3: f64,
    pub max: f64,
}

impl Summary {
    pub fn iqr(&self) -> f64 {
        self.q3 - self.q1
    }
}

/// Quantile of an ascending slice by linear interpolation between order statistics (the usual "type 7" rule).
pub fn quantile_sorted(s: &[f64], q: f64) -> f64 {
    if s.is_empty() {
        return f64::NAN;
    }
    let pos = q * (s.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    s[lo] + (s[hi] - s[lo]) * (pos - lo as f64)
}

pub fn summarize(v: &[f64]) -> Summary {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    Summary {
        n: s.len(),
        min: s.first().copied().unwrap_or(f64::NAN),
        q1: quantile_sorted(&s, 0.25),
        median: quantile_sorted(&s, 0.5),
        q3: quantile_sorted(&s, 0.75),
        max: s.last().copied().unwrap_or(f64::NAN),
    }
}

/// The rule used for every cell: a difference whose interquartile range is wider than the difference itself is not a
/// result (docs/way-of-working.md section 6, rule 3).
pub fn is_noisy(s: &Summary) -> bool {
    s.iqr() > s.median.abs()
}

pub fn verdict(s: &Summary) -> &'static str {
    if is_noisy(s) {
        "NOISY (IQR > difference)"
    } else if s.median > 0.0 {
        "slower"
    } else if s.median < 0.0 {
        "faster"
    } else {
        "equal"
    }
}

/// Per-round paired overhead in percent: `(arm - control) / control * 100`, round by round.
pub fn paired_overhead_pct(arm: &[f64], control: &[f64]) -> Vec<f64> {
    arm.iter().zip(control).map(|(a, c)| (a / c - 1.0) * 100.0).collect()
}

/// Per-round paired difference converted to nanoseconds per batch per worker thread:
/// `(arm - control) seconds * threads / batches`.
pub fn paired_delta_ns_per_batch(arm: &[f64], control: &[f64], threads: usize, batches: usize) -> Vec<f64> {
    arm.iter()
        .zip(control)
        .map(|(a, c)| (a - c) * 1e9 * threads as f64 / batches as f64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartiles_of_known_data() {
        let s = summarize(&[5.0, 1.0, 3.0, 2.0, 4.0]);
        assert_eq!((s.min, s.q1, s.median, s.q3, s.max), (1.0, 2.0, 3.0, 4.0, 5.0));
        let s = summarize(&[4.0, 1.0, 3.0, 2.0]);
        assert_eq!((s.min, s.median, s.max), (1.0, 2.5, 4.0));
        assert!((s.q1 - 1.75).abs() < 1e-12 && (s.q3 - 3.25).abs() < 1e-12);
        let one = summarize(&[7.0]);
        assert_eq!((one.q1, one.median, one.q3), (7.0, 7.0, 7.0));
        assert!(summarize(&[]).median.is_nan());
    }

    #[test]
    fn verdict_flags_a_difference_smaller_than_its_spread() {
        // difference +1 but the middle half of the rounds spans 10 points: not resolved
        let wide = summarize(&[-6.0, -3.0, 1.0, 4.0, 9.0]);
        assert!(is_noisy(&wide));
        assert!(verdict(&wide).starts_with("NOISY"));
        // a stable +5
        let tight = summarize(&[4.9, 5.0, 5.0, 5.1, 5.2]);
        assert!(!is_noisy(&tight));
        assert_eq!(verdict(&tight), "slower");
        let faster = summarize(&[-4.9, -5.0, -5.0, -5.1, -5.2]);
        assert_eq!(verdict(&faster), "faster");
    }

    #[test]
    fn paired_overhead_is_computed_round_by_round() {
        let o = paired_overhead_pct(&[1.1, 2.4, 3.0], &[1.0, 2.0, 3.0]);
        assert!((o[0] - 10.0).abs() < 1e-9 && (o[1] - 20.0).abs() < 1e-9 && o[2].abs() < 1e-9);
        // 2 ms more on 4 threads over 1000 batches = 8000 ns per batch per worker
        let d = paired_delta_ns_per_batch(&[0.012], &[0.010], 4, 1000);
        assert!((d[0] - 8000.0).abs() < 1e-6);
    }
}
