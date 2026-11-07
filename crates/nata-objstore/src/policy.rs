//! The request policy of ADR 0011 as data. Every number is the ADR's provisional TARGET (card 0006 part 2; the frozen ceilings of part 3 concern
//! timings, not these caps), not a measured optimum.
//!
//! [`Mutants`] switch on one deliberate defect of the wrapper each, so that every test can be shown to fail on a known-bad version. They take effect only
//! when the crate is built with the `mutants` feature (the test suites of this crate and the mutant sweep); in every other build the accessors are
//! constant `false` and the defects are compiled out.

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
    /// Range merging inside one row group: gap, merged response size, cumulative extra data as a percentage of needed data. ADR 0011: 1 MiB / 64 MiB / 25 %.
    /// Merging across row groups stays off (ADR 0011 amendment 3).
    pub merge_gap: u64,
    pub merge_max: u64,
    pub waste_pct: u64,
    /// Footer wire cap (rejected before allocation) and the first suffix read. ADR 0011: 16 MiB; the 64 KiB prefetch is the spike's (amendment 4).
    pub footer_cap: u64,
    pub footer_prefetch: u64,
    /// Bytes of an error response body the transport keeps (the rest is dropped unread).
    pub error_body_cap: usize,
    /// The most bytes of a SUCCESS body that is not a ranged object read (a LIST page, a credential or token JSON) the transport lets the library read.
    pub meta_body_cap: usize,
    /// Live response bytes one query may hold (ADR 0009 budget `B` of the query's memory service); reserved BEFORE a request is issued.
    pub grant_bytes: usize,
    /// The most objects one glob may list (bounded memory whatever the bucket holds).
    pub max_list_keys: usize,
    /// Parsed-footer cache: charged bytes, entries, and the largest single entry. ADR 0012: 64 MiB / 1,024 / 16 MiB.
    pub cache_bytes: u64,
    pub cache_entries: usize,
    pub cache_entry_max: u64,
}

impl Config {
    /// ADR 0011 and 0012's targets.
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
            meta_body_cap: 8 << 20,
            grant_bytes: 1 << 30,
            max_list_keys: 100_000,
            cache_bytes: 64 << 20,
            cache_entries: 1024,
            cache_entry_max: 16 << 20,
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

macro_rules! mutants {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        /// One deliberate defect each (all false = the real wrapper).
        #[derive(Clone, Debug, Default)]
        pub struct Mutants {
            $($(#[$doc])* pub $name: bool,)*
        }
        impl Mutants {
            $(
                #[inline]
                pub(crate) fn $name(&self) -> bool {
                    cfg!(feature = "mutants") && self.$name
                }
            )*
            /// Every mutant of the wrapper by name, each with only its switch set.
            pub fn all() -> Vec<(&'static str, Mutants)> {
                vec![$((stringify!($name), Mutants { $name: true, ..Default::default() })),*]
            }
        }
    };
}

mutants! {
    /// Retry forever (the attempt cap is ignored).
    no_attempt_cap,
    /// No operation deadline.
    no_deadline,
    /// No per-attempt timeout (client timeouts off too): a stalled body hangs.
    no_attempt_timeout,
    /// Do not compare the identity of a response with the pinned one, and send no If-Match / version.
    no_version_check,
    /// Do not check body length against the requested range (accept short and long bodies).
    no_length_check,
    /// Treat a permanent error (identity change, bad length, auth) as retryable.
    retry_permanent,
    /// A failed read is reported as an empty success.
    swallow_error,
    /// Plain transport: error response bodies are buffered unbounded by the client crate.
    no_transport_bound,
    /// Ignore Retry-After (sleep by backoff only, even when the header asks for longer than the deadline allows).
    ignore_retry_after,
    /// Keep the bytes of a failed attempt and append the retry's bytes (duplicate bytes).
    append_on_retry,
    /// No per-query or per-node cap on active attempts.
    no_inflight_cap,
    /// Success bodies the library reads itself (credential JSON, LIST XML) pass the transport unbounded.
    no_success_body_bound,
    /// Response bytes are not reserved against the memory grant before the request.
    no_grant,
}

/// Defects of the footer cache (ADR 0012).
#[derive(Clone, Debug, Default)]
pub struct CacheMutants {
    /// The key has no identity part: a replaced object finds the old entry.
    pub key_without_identity: bool,
    /// A warm lookup trusts the cache by path and issues no identity lookup.
    pub no_validation: bool,
    /// Evicting a pinned entry stops charging it at once (the live handle's bytes are uncounted).
    pub uncharged_pins: bool,
    /// Admit without evicting or checking the caps.
    pub no_cap: bool,
}

impl CacheMutants {
    pub(crate) fn key_without_identity(&self) -> bool {
        cfg!(feature = "mutants") && self.key_without_identity
    }
    pub(crate) fn no_validation(&self) -> bool {
        cfg!(feature = "mutants") && self.no_validation
    }
    pub(crate) fn uncharged_pins(&self) -> bool {
        cfg!(feature = "mutants") && self.uncharged_pins
    }
    pub(crate) fn no_cap(&self) -> bool {
        cfg!(feature = "mutants") && self.no_cap
    }

    pub fn all() -> Vec<(&'static str, CacheMutants)> {
        vec![
            (
                "key_without_identity",
                CacheMutants {
                    key_without_identity: true,
                    ..Default::default()
                },
            ),
            (
                "no_validation",
                CacheMutants {
                    no_validation: true,
                    ..Default::default()
                },
            ),
            (
                "uncharged_pins",
                CacheMutants {
                    uncharged_pins: true,
                    ..Default::default()
                },
            ),
            (
                "no_cap",
                CacheMutants {
                    no_cap: true,
                    ..Default::default()
                },
            ),
        ]
    }
}
