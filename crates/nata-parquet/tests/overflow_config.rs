//! ADR 0003 decision 6 and the scan. Two facts about the build configuration, as tests (the same two as `spikes/hash-ops/tests/overflow_config.rs`):
//!
//! 1. Code of this workspace is overflow-checked, also in release: `i32::MAX + 1` panics. (The test crate is a member of the workspace, so it shows the
//!    setting that `nata-parquet` itself is built with.)
//! 2. DEPENDENCIES are not: arrow computes on the slots under NULLs, whose contents are unspecified, with plain `+`. `MutableArrayData` (used by `concat` of
//!    dictionary arrays) re-bases every key, NULL slots included, by adding an offset (arrow-data 60.1.0 `src/transform/primitive.rs:46`, `*x + offset`). A
//!    legal dictionary array with `i32::MAX` under a NULL slot overflows there. With checks off in arrow that wraps and is harmless (the key is under a NULL);
//!    with checks on it PANICS. The scan hands out dictionary arrays (strings), so a later operator that concatenates batches would hit it. The test fails
//!    when the workspace `[profile.*.package."*"] overflow-checks = false` entries are removed.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the test needs one unchecked addition to see whether it panics"
)]

use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::Int32Type;
use arrow_array::{Array, ArrayRef, DictionaryArray, Int32Array, StringArray};
use arrow_buffer::NullBuffer;
use arrow_select::concat::concat;

/// 2 rows: row 0 is valid (`value`), row 1 is a NULL slot whose key is `junk`.
fn dict_with_junk_null(value: &str, junk: i32) -> ArrayRef {
    let keys = Int32Array::new(
        vec![0, junk].into(),
        Some(NullBuffer::from(vec![true, false])),
    );
    Arc::new(
        DictionaryArray::<Int32Type>::try_new(keys, Arc::new(StringArray::from(vec![value])))
            .expect("arrow accepts any key under a NULL slot"),
    )
}

fn check(c: &dyn Array) {
    let d = c.as_dictionary::<Int32Type>();
    assert_eq!(
        (
            d.len(),
            d.is_valid(0),
            d.is_null(1),
            d.is_valid(2),
            d.is_null(3)
        ),
        (4, true, true, true, true)
    );
    let v = d.values().as_string::<i32>();
    assert_eq!(
        (
            v.value(d.keys().value(0) as usize),
            v.value(d.keys().value(2) as usize)
        ),
        ("a", "b")
    );
}

#[test]
fn concat_of_dictionaries_with_an_extreme_key_under_a_null_slot_does_not_panic() {
    // the second dictionary is re-based by +1 when the dictionaries are merged; i32::MAX + 1 overflows
    let (a, b) = (
        dict_with_junk_null("a", 99),
        dict_with_junk_null("b", i32::MAX),
    );
    let r = catch_unwind(AssertUnwindSafe(|| {
        concat(&[a.as_ref(), b.as_ref()]).expect("concat")
    }));
    let c = r.expect("arrow-data panicked: dependencies must be built WITHOUT overflow checks (see the module docs and the workspace Cargo.toml)");
    check(c.as_ref());
}

#[test]
fn control_a_key_that_does_not_overflow_the_offset_is_fine_with_checks_on_or_off() {
    let (a, b) = (
        dict_with_junk_null("a", -1),
        dict_with_junk_null("b", i32::MAX - 1),
    );
    let c = concat(&[a.as_ref(), b.as_ref()]).expect("concat");
    check(c.as_ref());
}

#[test]
fn our_own_arithmetic_still_panics_on_overflow() {
    let r = catch_unwind(|| black_box(i32::MAX) + black_box(1));
    assert!(
        r.is_err(),
        "i32::MAX + 1 wrapped: overflow checks are OFF for the workspace's own code (ADR 0003 decision 6)"
    );
}
