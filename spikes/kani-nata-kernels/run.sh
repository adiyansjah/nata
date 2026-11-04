#!/usr/bin/env bash
# Run Kani harnesses ONE AT A TIME (never in parallel), a time cap each; one line per harness.
# Usage: run.sh [seconds-cap, default 600] [name-regex | --local]
#   --local : the cheap subset (rows <= 2, rows 0 only for split_ne / filter_ne / the CASE pipeline, no grouped sums, and
#   the 8-bit primitive proofs); what was run on the dev machine
#   (no filter) : every harness, for the 64 GB GCP VM. Exit status 1 if any harness is not SUCCESSFUL.
cd "$(dirname "$0")" || exit 2
cap="${1:-600}"; filt="${2:-.}"
LOCAL_RE='^(primitive_overflowing_sub_i64_is_exact|primitive_overflowing_mul_i16_is_exact|primitive_checked_div_i8_is_exact|characterisation_of_truncating_division_i8|div_zero_message_text)$|^(each_visits_the_selection_in_order|mul_logic_matches_the_spec|sub_const_left_logic_matches_the_spec|sub_const_left_exact_matches_the_spec|div_logic_matches_the_spec|div_const_left_logic_matches_the_spec|merge_branches_matches_the_spec|sum_matches_the_spec)_rows[0-2]$|^(split_ne_partitions_the_selection|filter_ne_matches_the_spec|case_guarded_division_matches_the_spec)_rows0$|^filter_range_i64_rows[0-2]_'
[ "$filt" = "--local" ] && filt="$LOCAL_RE"
# a harness is the fn that follows a #[kani::proof] attribute
harnesses=$(awk '/#\[kani::proof\]/{p=1;next} p && /^ *fn /{sub(/^ *fn /,"");sub(/\(.*/,"");print;p=0}' src/lib.rs | grep -E "$filt")
[ -n "$harnesses" ] || { echo "no harness matches '$filt'"; exit 2; }
rc=0
for h in $harnesses; do
  line=$(./run1.sh "$h" "$cap"); echo "$line"
  case "$line" in *": SUCCESSFUL |"*) ;; *) rc=1;; esac
done
exit $rc
