//! The request policy of ADR 0011 as data. Every number is a provisional TARGET of the ADR (card 0006 part 2), not a measured optimum.
//! `Mutants` switch on one deliberate defect of the wrapper each, so that every test can be shown to fail on a known-bad version.

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    /// Total attempts of one logical read (first try + retries). ADR 0011: 4.
    pub max_attempts: u32,
    /// Equal-jitter exponential backoff: nominal = min(initial * 2^(k-1), max), delay in [nominal/2, nominal].
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    /// Headers and body of one attempt share this timeout. ADR 0011: 30 s.
    pub attempt_timeout: Duration,
    /// The whole logical read (permit wait, attempts, backoff). ADR 0011: 120 s.
    pub op_deadline: Duration,
    /// Active attempts per query / per node. ADR 0011: 16 / 32.
    pub query_inflight: usize,
    pub node_inflight: usize,
    /// Range merging: gap, merged response size, cumulative extra data as a percentage of needed data. ADR 0011: 1 MiB / 64 MiB / 25 %.
    pub merge_gap: u64,
    pub merge_max: u64,
    pub waste_pct: u64,
    /// Footer wire cap (rejected before allocation) and the first suffix read. ADR 0011: 16 MiB.
    pub footer_cap: u64,
    pub footer_prefetch: u64,
    /// Bytes of an error response body the transport keeps (the rest is dropped unread).
    pub error_body_cap: usize,
    /// Live response bytes the node may hold (a stand-in for the ADR 0009 grant); reserved BEFORE a request is issued.
    pub grant_bytes: u64,
}

impl Config {
    /// ADR 0011's targets.
    pub fn adr() -> Config {
        Config {
            max_attempts: 4,
            backoff_initial: Duration::from_millis(100),
            backoff_max: Duration::from_secs(5),
            attempt_timeout: Duration::from_secs(30),
            op_deadline: Duration::from_secs(120),
            query_inflight: 16,
            node_inflight: 32,
            merge_gap: 1 << 20,
            merge_max: 64 << 20,
            waste_pct: 25,
            footer_cap: 16 << 20,
            footer_prefetch: 64 << 10,
            error_body_cap: 4 << 10,
            grant_bytes: 1 << 30,
        }
    }
    /// Same structure, time constants scaled so that a fault test takes seconds (the attempt count, caps and sizes are unchanged).
    pub fn fast_tests() -> Config {
        Config {
            backoff_initial: Duration::from_millis(20),
            backoff_max: Duration::from_millis(200),
            attempt_timeout: Duration::from_millis(800),
            op_deadline: Duration::from_secs(6),
            ..Config::adr()
        }
    }
}

/// One deliberate defect each (all false = the real wrapper).
#[derive(Clone, Debug, Default)]
pub struct Mutants {
    /// Retry forever (the attempt cap is ignored).
    pub no_attempt_cap: bool,
    /// No operation deadline.
    pub no_deadline: bool,
    /// No per-attempt timeout (client timeouts off too): a stalled body hangs.
    pub no_attempt_timeout: bool,
    /// Do not compare the identity of a response with the pinned one, and send no If-Match / version.
    pub no_version_check: bool,
    /// Do not check body length against the requested range (accept short and long bodies).
    pub no_length_check: bool,
    /// Treat a permanent error (identity change, bad length, auth) as retryable.
    pub retry_permanent: bool,
    /// A failed read is reported as an empty success.
    pub swallow_error: bool,
    /// Plain transport: error response bodies are buffered unbounded by the client crate.
    pub no_transport_bound: bool,
    /// Ignore Retry-After (sleep by backoff only, even when the header asks for longer than the deadline allows).
    pub ignore_retry_after: bool,
    /// Keep the bytes of a failed attempt and append the retry's bytes (duplicate bytes).
    pub append_on_retry: bool,
}

impl Mutants {
    pub fn all() -> Vec<(&'static str, Mutants)> {
        vec![
            ("no_attempt_cap", Mutants { no_attempt_cap: true, ..Default::default() }),
            ("no_deadline", Mutants { no_deadline: true, ..Default::default() }),
            ("no_attempt_timeout", Mutants { no_attempt_timeout: true, ..Default::default() }),
            ("no_version_check", Mutants { no_version_check: true, ..Default::default() }),
            ("no_length_check", Mutants { no_length_check: true, ..Default::default() }),
            ("retry_permanent", Mutants { retry_permanent: true, ..Default::default() }),
            ("swallow_error", Mutants { swallow_error: true, ..Default::default() }),
            ("no_transport_bound", Mutants { no_transport_bound: true, ..Default::default() }),
            ("ignore_retry_after", Mutants { ignore_retry_after: true, ..Default::default() }),
            ("append_on_retry", Mutants { append_on_retry: true, ..Default::default() }),
        ]
    }
}
