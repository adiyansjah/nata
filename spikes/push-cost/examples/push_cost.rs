//! The push-cost benchmark. See `bench/results/20261104-push-cost.md` and `spikes/push-cost/gcp-check.sh`.
//!
//! usage: push_cost [--rounds N] [--total-log2 L] [--csv FILE] [--smoke] [--selftest-only] [--ignore-selftest]
use spike_push_cost::bench::{Opts, run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match Opts::parse(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}\nusage: push_cost [--rounds N] [--total-log2 L] [--csv FILE] [--smoke] [--selftest-only] [--ignore-selftest]");
            std::process::exit(2);
        }
    };
    std::process::exit(run(&opts));
}
