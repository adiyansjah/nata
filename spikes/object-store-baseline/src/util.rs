//! Small helpers: a seeded generator (every fault schedule is replayable from its seed), percentiles, redaction.

use std::time::Duration;

/// SplitMix64: tiny, seeded, good enough for jitter and fault schedules.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Percentile (0..=100) of an unsorted slice by the nearest-rank method; 0 for an empty slice.
pub fn percentile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = ((p / 100.0) * s.len() as f64).ceil() as usize;
    s[rank.clamp(1, s.len()) - 1]
}

pub fn median(v: &[f64]) -> f64 {
    percentile(v, 50.0)
}

pub fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

/// Replace every occurrence of a secret (bucket, endpoint, prefix, key) by a placeholder.
#[derive(Clone, Default, Debug)]
pub struct Redactor(Vec<String>);

impl Redactor {
    pub fn new(secrets: Vec<String>) -> Self {
        let mut s: Vec<String> = secrets.into_iter().filter(|x| x.len() >= 3).collect();
        s.sort_by_key(|x| std::cmp::Reverse(x.len()));
        Redactor(s)
    }
    pub fn apply(&self, text: &str) -> String {
        let mut t = text.to_string();
        for (i, s) in self.0.iter().enumerate() {
            t = t.replace(s.as_str(), &format!("<redacted-{i}>"));
        }
        t
    }
    pub fn leaks(&self, text: &str) -> bool {
        self.0.iter().any(|s| text.contains(s.as_str()))
    }
}

pub fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rng_is_replayable() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert_ne!(Rng::new(7).next_u64(), Rng::new(8).next_u64());
    }
    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        assert_eq!(percentile(&v, 50.0), 50.0);
        assert_eq!(percentile(&v, 99.0), 99.0);
        assert_eq!(percentile(&[], 99.0), 0.0);
    }
    #[test]
    fn redactor_hides_and_detects() {
        let r = Redactor::new(vec!["secret-bucket".into(), "ab".into()]);
        assert!(r.leaks("gs://secret-bucket/x"));
        assert_eq!(r.apply("gs://secret-bucket/x"), "gs://<redacted-0>/x");
        assert!(!r.leaks("ab")); // too short to be a secret: ignored
    }
}
