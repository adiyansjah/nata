//! ADR 0003 decision 6 and dependencies. Two facts about the build configuration, as tests:
//!
//! 1. Our own code is overflow-checked: `hash_ops::guard::plain_add(i64::MAX, 1)` must panic. If someone turns
//!    the checks off for everything (or the package override ever covers our crate) this fails.
//! 2. Dependencies are NOT overflow-checked. arrow computes on the slots under NULLs, whose contents are
//!    unspecified, with plain `+`: `MutableArrayData` (used by `concat` of dictionary arrays) re-bases every
//!    key, NULL slots included, by adding an offset (arrow-data 60.1.0, src/transform/primitive.rs:46,
//!    `*x + offset`). A legal dictionary array with `i32::MAX` under a NULL slot therefore overflows. With
//!    checks off in arrow that wraps and is harmless (the key is under a NULL); with checks on it panics.
//!    The minimal repro below uses only arrow. It fails when the dependencies are built with
//!    overflow-checks (the spike's `[profile.*.package."*"] overflow-checks = false` prevents that).
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, DictionaryArray, Int32Array, MutableArrayData, StringArray};
use arrow::buffer::NullBuffer;
use arrow::compute::concat;
use arrow::datatypes::Int32Type;

/// A 2-row dictionary array: row 0 is valid ("v"), row 1 is a NULL slot holding `junk` as its key.
fn dict_with_junk_null(value: &str, junk: i32) -> ArrayRef {
    let keys = Int32Array::new(vec![0, junk].into(), Some(NullBuffer::from(vec![true, false])));
    Arc::new(DictionaryArray::<Int32Type>::try_new(keys, Arc::new(StringArray::from(vec![value]))).expect("arrow accepts any key under a NULL slot"))
}

fn check_concat(c: &dyn Array) {
    let d = c.as_dictionary::<Int32Type>();
    assert_eq!((d.len(), d.is_valid(0), d.is_null(1), d.is_valid(2), d.is_null(3)), (4, true, true, true, true));
    let v = d.values().as_string::<i32>();
    assert_eq!((v.value(d.keys().value(0) as usize), v.value(d.keys().value(2) as usize)), ("a", "b"));
}

#[test]
fn arrow_concat_of_dictionaries_with_an_extreme_key_under_a_null_slot_does_not_panic() {
    // The second array's dictionary is re-based by +1 when the dictionaries are merged; i32::MAX + 1 overflows.
    let (a, b) = (dict_with_junk_null("a", 99), dict_with_junk_null("b", i32::MAX));
    let r = catch_unwind(AssertUnwindSafe(|| concat(&[a.as_ref(), b.as_ref()]).expect("concat")));
    let c = r.expect("arrow-data panicked: dependencies must be built WITHOUT overflow checks (see the module docs)");
    check_concat(c.as_ref());
}

#[test]
fn arrow_concat_control_a_key_that_does_not_overflow_the_offset_is_fine_with_checks_on_or_off() {
    // i32::MAX - 1 + 1 does not overflow: the panic above is the addition `key + offset`, not the extreme key as such.
    let (a, b) = (dict_with_junk_null("a", -1), dict_with_junk_null("b", i32::MAX - 1));
    let c = concat(&[a.as_ref(), b.as_ref()]).expect("concat");
    check_concat(c.as_ref());
}

#[test]
fn arrow_mutable_array_data_with_an_extreme_key_under_a_null_slot_does_not_panic() {
    // The primitive that concat is built on, called directly (no merging of dictionaries by us).
    let (a, b) = (dict_with_junk_null("a", 99), dict_with_junk_null("b", i32::MAX));
    let r = catch_unwind(AssertUnwindSafe(|| {
        let (da, db) = (a.to_data(), b.to_data());
        let mut m = MutableArrayData::new(vec![&da, &db], false, 4);
        m.try_extend(0, 0, 2).expect("extend");
        m.try_extend(1, 0, 2).expect("extend");
        m.freeze()
    }));
    r.expect("arrow-data panicked in MutableArrayData: dependencies must be built WITHOUT overflow checks");
}

#[test]
fn our_own_arithmetic_still_panics_on_overflow() {
    let r = catch_unwind(|| hash_ops::guard::plain_add(std::hint::black_box(i64::MAX), std::hint::black_box(1)));
    assert!(r.is_err(), "i64::MAX + 1 wrapped in this crate: overflow checks are OFF for our own code (ADR 0003 decision 6)");
}
