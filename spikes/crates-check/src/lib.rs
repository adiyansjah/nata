//! Helpers for the crate checks: deterministic, semi-compressible test data and a CPU feature report.

/// `n` bytes that compress by a factor of about 3-6: runs of a small alphabet with a slowly changing pattern.
pub fn test_data(n: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let run = 4 + (s % 29) as usize;
        let b = b'a' + ((s >> 32) % 8) as u8;
        for _ in 0..run.min(n - v.len()) {
            v.push(b);
        }
    }
    v
}

/// The CPU features that decide which fast paths the C and SIMD code of these crates can use.
pub fn cpu_report() -> String {
    let mut out = format!("arch: {}\n", std::env::consts::ARCH);
    #[cfg(target_arch = "aarch64")]
    {
        out += &format!(
            "neon={} aes={} crc={} sha2={} lse={} sve={}\n",
            std::arch::is_aarch64_feature_detected!("neon"),
            std::arch::is_aarch64_feature_detected!("aes"),
            std::arch::is_aarch64_feature_detected!("crc"),
            std::arch::is_aarch64_feature_detected!("sha2"),
            std::arch::is_aarch64_feature_detected!("lse"),
            std::arch::is_aarch64_feature_detected!("sve"),
        );
    }
    #[cfg(target_arch = "x86_64")]
    {
        out += &format!(
            "sse4.2={} avx2={} bmi2={} pclmulqdq={} avx512f={}\n",
            std::arch::is_x86_feature_detected!("sse4.2"),
            std::arch::is_x86_feature_detected!("avx2"),
            std::arch::is_x86_feature_detected!("bmi2"),
            std::arch::is_x86_feature_detected!("pclmulqdq"),
            std::arch::is_x86_feature_detected!("avx512f"),
        );
    }
    out
}
