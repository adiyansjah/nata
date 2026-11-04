//! Small helpers: a seedable RNG, percentiles, byte formatting, process CPU time.

/// splitmix64: tiny, seedable, good enough for latency draws and test data.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in the open interval (0, 1).
    pub fn f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.f64(), self.f64());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Nearest-rank percentile (`p` in 0..=100) of an ALREADY SORTED slice. `NaN` for an empty slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

pub fn median(v: &[f64]) -> f64 {
    percentile(&sorted(v.to_vec()), 50.0)
}

pub fn mib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

/// Environment variable, `None` when unset or empty.
pub fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// utime + stime of this process in seconds (Linux `/proc/self/stat`; 100 ticks per second). 0.0 elsewhere.
pub fn proc_cpu_seconds() -> f64 {
    let Ok(s) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // the command name is in parentheses and may contain spaces: skip past the last ')'
    let Some(rest) = s.rsplit_once(')').map(|x| x.1) else {
        return 0.0;
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    // after ')' the fields start at field 3 (state); utime is field 14, stime field 15 -> indexes 11 and 12
    let get = |i: usize| f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    (get(11) + get(12)) / 100.0
}

/// 1-minute load average (Linux). `None` when unreadable.
pub fn load1() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

pub fn nproc() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get())
}
