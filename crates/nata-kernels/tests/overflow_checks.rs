//! ADR 0003, decision 6: kernel crates build with `overflow-checks = true`, also in release.
//! This test is compiled with the release profile of the workspace, so it fails in `cargo test --release` if the
//! `[profile.release] overflow-checks = true` entry in the root Cargo.toml is removed.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the test needs one unchecked addition to see whether it panics"
)]
use std::hint::black_box;

#[test]
fn an_unchecked_addition_panics_instead_of_wrapping() {
    let r = std::panic::catch_unwind(|| black_box(i32::MAX) + black_box(1));
    assert!(
        r.is_err(),
        "i32::MAX + 1 must panic with 'attempt to add with overflow'; it wrapped to {:?}",
        r.ok()
    );
}
