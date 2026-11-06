# Kani proof that the step-6 wrapping sums of the NULL path never wrap (2026-11-06)

Run `kani-solvers-arm64-r2` (job `kani-solvers`, `n4a-highmem-8`, Neoverse-N3, Debian 13), code `5c51da1`. The first attempt the same day
(`kani-solvers-arm64`, Debian 12, $0.009) proved nothing: the Kani 0.68 release bundle needs glibc 2.39 and every harness reported NO RESULT;
the Kani jobs now default to Debian 13 and the install step stops the job when `cargo kani` does not run (`5c51da1`).

## The claim

Step 6 of the word-wise NULL path (`spikes/null-path`, `wk::group_sums` with `WRAP = true`) accumulates with `wrapping_add` instead of the
checked `+`, which is only correct if it can never wrap. The argument had been reasoned, not checked (`bench/results/20261105-timing-arm64.md`
section 2). The two accumulations were moved, unchanged, to `spikes/null-path/src/wrapadd.rs`, which the null-path code calls and the proof
crate `spikes/kani-wrap-bound` includes by `#[path]` (the same file, not a copy). The null-path tests pass after the move (26 of 26).

Proof by induction, each harness checked by Kani for ALL values of its symbolic inputs:

| harness | what it proves | result |
|---|---|---|
| `sum_base_case` | 0 addends: the sum 0 is inside the invariant `-k * 2^63 <= s <= k * (2^63 - 1)` for k = 0 | SUCCESSFUL (0 of 5 checks failed) |
| `sum_step_never_wraps` | for every k < 2^64 - 1, every sum inside the bounds of k and every addend of the two shapes the code uses (`v * ok`, `v & -(bit)`, i.e. an i64 value or 0): the exact add does not overflow, `add128::<true>` equals it, and the result is inside the bounds of k + 1 | SUCCESSFUL (0 of 74 failed) |
| `bounds_fit_in_i128` | the invariant's own bounds never overflow i128 for any k | SUCCESSFUL (0 of 17 failed) |
| `counter_step_never_wraps` | a u64 counter at most the rows processed (< u64::MAX) plus 0 or 1: `add64::<true>` equals the checked add | SUCCESSFUL (0 of 60 failed) |
| `checked_and_wrapping_agree` | `WRAP = true` and `WRAP = false` give the same result on i64-sized operands | SUCCESSFUL (0 of 51 failed) |

The proofs can fail (`spikes/kani-wrap-bound/mutate.sh`, each on a temporary copy; INVALID never counts):

| known-bad version | harness | verdict |
|---|---|---|
| negative control: the addend may be any i128 | `negative_control_wide_addend_wraps` | CAUGHT |
| the sums' wrapping add adds 1 when the addend is 0 (a NULL row) | `sum_step_never_wraps` | CAUGHT |
| the counters' wrapping add sets the low bit of the addend | `counter_step_never_wraps` | CAUGHT |
| the induction allows k = u64::MAX (then k + 1 overflows) | `sum_step_never_wraps` | CAUGHT |

**Conclusion: the step-6 wrapping accumulations are equal to the checked ones for every input the sink can see, as long as fewer than
2^64 - 1 rows reach one group**, which cannot happen in practice (the row counter would overflow first, and that counter is proved too).
Step 6 can therefore be adopted without losing the overflow guarantee of ADR 0003 decision 6.

What this does not show: the rest of `group_sums` (the group-id computation, the selection words, the NULL masks) is covered by the null-path
tests and its 62 mutants, not by this proof; the proof is about the accumulation step only. Raw: `bench/results/raw/20261106-kani-solvers-arm64/`.
The second part of the same job, the five division proofs retried with other solvers, is recorded below when it has run.

## 2. The five division proofs with other solvers (same run)

The five harnesses that timed out with Kani's default CaDiCaL back end on 2026-11-04 (3,600 s each; `bench/results/20261104-kani-nata-kernels.md`)
were run once per other back end, 2,400 s each, 5 at a time on 64 GB (raw `table-solvers.txt`):

| harness | kissat | z3 | cvc5 |
|---|---|---|---|
| `primitive_checked_div_i32_is_exact` | TIMEOUT 2,400 s | TIMEOUT 2,400 s | TIMEOUT 2,400 s |
| `div_exact_matches_the_spec_rows1` / `rows2` | TIMEOUT / TIMEOUT | CBMC aborted (status 6) after 6 s / same | CBMC aborted after 5 s / same |
| `div_const_left_exact_matches_the_spec_rows1` / `rows2` | TIMEOUT / TIMEOUT | CBMC aborted / same | CBMC aborted / same |

**None of the five is proved by any back end within these limits.** The SMT back ends do not get as far as solving the four kernel harnesses
(CBMC itself aborts while handing them over; not investigated further); on the 32-bit primitive all three time out. Decision (taken by the
assistant under the owner's delegation, recorded in ADR 0003): these five stay an ACCEPTED TRUST POINT, not an open item. What the
division kernels rest on instead: the kernel LOGIC proofs (`div_logic_matches_the_spec`, `div_const_left_logic_matches_the_spec`, rows 0-2,
SUCCESSFUL) prove the kernels around the divide; the divide primitive itself is proved exact at 8 bits
(`primitive_checked_div_i8_is_exact`) and is one generic implementation in Rust's standard library for all widths; the differential
tests of the kernels against DuckDB and the decimal verification (56 of 56 mutants) exercise the 64-bit divide on real values. A larger
machine or a dedicated bit-vector solver (Bitwuzla, not packaged for Debian) is the way to try again; it is not planned.
