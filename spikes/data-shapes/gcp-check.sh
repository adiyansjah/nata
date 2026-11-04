#!/usr/bin/env bash
# gcp-check.sh <summary-dir> [scratch-base-dir]
#
# Verification-plan item 20: "results hold on other data". Generates skewed-key, string-heavy and many-NULL data
# sets with DuckDB (deterministic: every value is a hash of the row number), runs the SAME aggregate and join
# queries through the engines of this repository, and compares each result with DuckDB's, exactly (normalised text,
# bytewise-sorted lines, as spikes/hash-ops/check.sh does).
#   * hash join + hash aggregate of spikes/hash-ops   (bin `shapes`; 84 queries; at 1, 4 and nproc threads)
#   * generic-operator engine of spikes/generic-ops   (bin `gshapes`; 16 queries: its Q1 and Q6 on NULL-heavy lineitem data)
# TPC-DS is out of scope (no network for its generator).
#
# Needs on PATH: duckdb, cargo (rust-toolchain.toml at the repo root pins the toolchain; this script cd's into the
# repo so it applies), python3 (only for the mutant self-test), sort, timeout; /usr/bin/time is used for peak RSS
# when present (otherwise the engine's own VmHWM figure is reported).
#
# Resources (estimates until the first GCP run fills in the real numbers):
#   disk   peak about 10 GB for the default N = 4,000,000 (data sets are deleted as soon as their queries are done:
#          largest is the string set at about 1.5 GB of Parquet; result texts up to about 1.5 GB at a time; cargo
#          target dir about 2.5 GB; DuckDB spill is not expected). Stays under 15 GB. The script refuses to start
#          with less than 14 GB free (1 GB when N < 1,000,000, for the small validation runs).
#   time   about 40 to 90 minutes on an n4a-highcpu-16 (16 vCPU Arm), of which 6 to 12 minutes are the build and
#          the four mutant builds. Each engine run has a timeout (TIMEOUT_S, default 900 s).
#   memory the string aggregates hold up to ~1M distinct 100-500 byte keys per thread; 32 GB is enough, and an
#          engine that runs out is REPORTED (status CRASH), not hidden.
#
# Output (all under <summary-dir>): summary.txt (plain text, the report), results.tsv (one row per query),
# machine.txt, build.log, failures/<query>.txt (first differing line, engine stdout/stderr tail).
# Exit status: 0 only if every query is identical to DuckDB at every thread count, every expected refusal happened
# and every self-test behaved (a perturbed or mutated engine MUST be reported as different); 1 if any data set differs,
# the engine crashed or timed out, or a self-test failed; 2 if the harness itself failed (build, generation).
#
# Environment knobs:
#   N=4000000          rows of the base fact table; every other size derives from it (see "sizes" below)
#   THREADS_LIST       engine thread counts (default: 1, 4 and nproc, de-duplicated)
#   TIMEOUT_S=900      per engine run
#   ONLY_TAGS="skew nul10"   run only these data sets (tags: skew hot str nul00 nul01 nul10 nul50 nul90 nul99 nul100
#                      nulshape li00 li01 li10 li50 li90 li99 li100 linullkey)
#   DS_SKIP_MUTANTS=1  skip the four mutant builds (the self-test of a known-bad engine)
#   DS_NO_ENGINE=1     DuckDB-only mode for validating the generators, the SQL and the comparison plumbing on a tiny N
#                      (e.g. N=10000): the "engine" is DuckDB's own answer; no cargo build; mutants skipped.
#   DS_STANDIN_PERTURB=kind  with DS_NO_ENGINE=1: corrupt every stand-in answer (appendchar count delline dupline nullkey
#                      dropnl empty) to check that the harness reports it and exits 1
#   KEEP=1             keep the scratch directory
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
[ $# -ge 1 ] || { sed -n '2,3p' "$0" >&2; exit 2; }
SUMMARY="$1"; mkdir -p "$SUMMARY"; SUMMARY="$(cd "$SUMMARY" && pwd)"
BASE="${2:-${WORK:-/var/tmp}}"; mkdir -p "$BASE"

N="${N:-4000000}"
TIMEOUT_S="${TIMEOUT_S:-900}"
NO_ENGINE="${DS_NO_ENGINE:-0}"
NT="$(nproc)"
if [ -z "${THREADS_LIST:-}" ]; then
  THREADS_LIST="$(printf '%s\n' 1 "$(( NT < 4 ? NT : 4 ))" "$NT" | sort -n -u | tr '\n' ' ')"
fi
NEED_GB=14; [ "$N" -lt 1000000 ] && NEED_GB=1

# ---- sizes (all derived from N so a tiny N gives a tiny, structurally identical set) -------------------------------
K=$((N / 4))            # distinct keys of the skewed columns; dimension rows
M=$((N * 3 / 4))        # string fact rows
MH=$((M * 2 / 3))       # possible values of s_hi
ML=$((M / 3))           # possible values of s_long
MI=$((M / 10))          # possible values of l_shipinstruct
B=$((N / 2))            # hot_build rows
P_ROWS=$((N / 4))       # hot_probe rows (100 of them hot)
HH=$((N / 200))         # hh_a, hh_b rows
NP=$((N / 2))           # rows per NULL-percentage file
NS=$N                   # NULL-shape rows
NL=$((N / 4))           # lineitem-shaped rows
K10=$((K / 10))         # NULL-key rows of nul_dimn
if [ "$NS" -ge $((40 * 98304)) ]; then RGS=98304; else RGS=$((2048 * (NS / (2048 * 5) > 0 ? NS / (2048 * 5) : 1))); fi
TG=$((NS / RGS / 2))    # the one row group where kr / vr are not NULL
export SHAPES_B="$B"    # read by the `shapes` query table (filter constant of hb_chain_small)

SCRATCH="$(mktemp -d "$BASE/data-shapes.XXXXXX")"
DATA="$SCRATCH/data"; EXP="$SCRATCH/exp"; TMPD="$SCRATCH/tmp"; BIN="$SCRATCH/bin"
mkdir -p "$DATA" "$EXP" "$TMPD" "$BIN" "$SCRATCH/duck-tmp" "$SUMMARY/failures"
cleanup() { [ "${KEEP:-0}" = 1 ] || rm -rf "$SCRATCH"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

SUM="$SUMMARY/summary.txt"; TSV="$SUMMARY/results.tsv"
: > "$SUM"; printf 'query\ttag\tresult\tduckdb_ms\tengine_ms\trss_mb\tgroups\tout_rows\tdetail\n' > "$TSV"
say() { printf '%s\n' "$*" | tee -a "$SUM"; }
die() { say "HARNESS ERROR: $*"; exit 2; }

DUCK=(env -u AI_AGENT -u CLAUDECODE duckdb)
TIMEBIN=""; [ -x /usr/bin/time ] && TIMEBIN=/usr/bin/time
START_S=$SECONDS

# ---- preflight ------------------------------------------------------------------------------------------------------
for t in duckdb sort timeout cmp awk sed; do command -v "$t" > /dev/null || die "$t not found on PATH"; done
if [ "$NO_ENGINE" != 1 ]; then command -v cargo > /dev/null || die "cargo not found on PATH"; fi
FREE_GB=$(df -P -BG "$SCRATCH" | awk 'NR==2 {gsub("G","",$4); print $4}')
{
  echo "date:      $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host:      $(uname -srm)"
  echo "cpu:       $(grep -m1 -E 'model name|Model name' /proc/cpuinfo 2> /dev/null | sed 's/.*: *//' || true) ($NT cores)  $(lscpu 2> /dev/null | grep -m1 'Model name' | sed 's/.*: *//')"
  echo "memory:    $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "duckdb:    $("${DUCK[@]}" -csv -noheader -c 'select version();' | tail -1)"
  echo "commit:    $(git -C "$ROOT" rev-parse --short HEAD 2> /dev/null || echo unknown)$(git -C "$ROOT" diff --quiet -- spikes/hash-ops spikes/generic-ops spikes/data-shapes 2> /dev/null || echo ' (+ uncommitted changes in the spikes)')"
  echo "scratch:   $SCRATCH (${FREE_GB} GB free, need ${NEED_GB})"
  echo "N=$N K=$K M=$M MH=$MH ML=$ML MI=$MI B=$B P=$P_ROWS HH=$HH NP=$NP NS=$NS NL=$NL row-group-size(nulshape)=$RGS target-group=$TG"
  echo "threads:   $THREADS_LIST   timeout/run: ${TIMEOUT_S}s   engine mode: $([ "$NO_ENGINE" = 1 ] && echo 'NONE (duckdb stand-in, plumbing check only)' || echo real)"
} | tee "$SUMMARY/machine.txt" | tee -a "$SUM"
[ "${FREE_GB:-0}" -ge "$NEED_GB" ] || die "only ${FREE_GB} GB free in $SCRATCH, need $NEED_GB"
[ -x "$TIMEBIN" ] || say "note: /usr/bin/time not found; peak RSS is the engine's own VmHWM figure"

# ---- build the engines (or the SQL tool) -----------------------------------------------------------------------------
if [ "$NO_ENGINE" = 1 ]; then
  rustc --edition 2024 -O "$HERE/src/sqltool.rs" -o "$BIN/sqltool" > "$SUMMARY/build.log" 2>&1 || die "sqltool does not compile (see build.log)"
  SHAPES="$BIN/sqltool"; GSHAPES="$BIN/sqltool"
else
  say "building spikes/data-shapes (release, locked) ..."
  ( cd "$ROOT" && CARGO_TARGET_DIR="$SCRATCH/target" cargo build --release --locked --manifest-path spikes/data-shapes/Cargo.toml --bins ) > "$SUMMARY/build.log" 2>&1 \
    || { tail -30 "$SUMMARY/build.log"; die "build failed (see build.log)"; }
  cp "$SCRATCH/target/release/shapes" "$SCRATCH/target/release/gshapes" "$BIN/"
  SHAPES="$BIN/shapes"; GSHAPES="$BIN/gshapes"
fi
tool_for() { case "$1" in li_*) echo "$GSHAPES" ;; *) echo "$SHAPES" ;; esac; }
list_queries() {  # list_queries <tag>: name TAB tag TAB expect, from the right table(s)
  if [ "$NO_ENGINE" = 1 ]; then "$SHAPES" list "$1"; else "$SHAPES" list "$1"; "$GSHAPES" list "$1"; fi
}

# ---- mutants: known-bad engines (builds happen now, runs happen inside the data set they need) ----------------------
# name (exact, from spikes/hash-ops/mutate.py) | data-set tag | query. mutate.py --dry applies the edit to a private
# copy; nothing in the repository changes.
MUTANTS=(
  "NULL group dropped from the output|nul10|nl_agg_kn_p10"
  "merge loses the last thread's partial table|skew|sk_agg_k10"
  "fan-out: only the first match of a chain is emitted|hot|hb_hot_hot"
  "SUM does not count its NULL inputs (non-NULL count wrong)|nulshape|ns_agg_kall"
)
declare -A MUT_BIN=()
build_mutants() {
  [ "$NO_ENGINE" = 1 ] && return 0
  [ "${DS_SKIP_MUTANTS:-0}" = 1 ] && { say "mutant self-test: SKIPPED (DS_SKIP_MUTANTS=1)"; return 0; }
  command -v python3 > /dev/null || die "python3 not found (needed to build the mutants; DS_SKIP_MUTANTS=1 skips them)"
  local i=0 spec name tag q d
  for spec in "${MUTANTS[@]}"; do
    i=$((i + 1)); IFS='|' read -r name tag q <<< "$spec"
    d="$SCRATCH/mut/$i"; mkdir -p "$d/target/release" "$d/spikes/data-shapes"   # target/release exists: mutate.py will not seed it
    HASH_OPS_MUT_ROOT="$d" python3 "$ROOT/spikes/hash-ops/mutate.py" --dry "$name" > "$d/dry.log" 2>&1 \
      || { cat "$d/dry.log"; die "mutate.py cannot apply '$name' (spikes/hash-ops changed?)"; }
    mv "$d/work" "$d/spikes/hash-ops"
    cp -r "$HERE/Cargo.toml" "$HERE/Cargo.lock" "$HERE/src" "$d/spikes/data-shapes/"
    cp "$ROOT/rust-toolchain.toml" "$d/"
    ( cd "$d/spikes/data-shapes" && CARGO_TARGET_DIR="$SCRATCH/target" cargo build --release --locked --bin shapes ) > "$d/build.log" 2>&1 \
      || { tail -20 "$d/build.log"; die "mutant '$name' does not compile"; }
    cp "$SCRATCH/target/release/shapes" "$d/shapes"
    MUT_BIN["$q"]="${MUT_BIN[$q]:-}$d/shapes|$name;"
    say "built mutant $i: $name"
  done
  # leave nothing that could be mistaken for the real binary
  cp "$BIN/shapes" "$SCRATCH/target/release/shapes"
}

# ---- placeholders in the SQL templates -------------------------------------------------------------------------------
render() {  # render <sql file>... : prelude + files, placeholders filled
  local f
  for f in "$@"; do cat "$HERE/sql/prelude.sql" "$HERE/sql/$f"; done | sed \
    -e "s|@DIR@|$DATA|g" -e "s|@NAME@|${NAME:-}|g" -e "s|@PP@|${PP:-}|g" -e "s|@PK@|${PK:-0}|g" -e "s|@P@|${P:-0}|g" \
    -e "s|@N@|$N|g" -e "s|@K10@|$K10|g" -e "s|@K@|$K|g" -e "s|@MH@|$MH|g" -e "s|@ML@|$ML|g" -e "s|@MI@|$MI|g" \
    -e "s|@M@|$M|g" -e "s|@B@|$B|g" -e "s|@P_ROWS@|$P_ROWS|g" -e "s|@HH@|$HH|g" -e "s|@NP@|$NP|g" \
    -e "s|@NS@|$NS|g" -e "s|@NL@|$NL|g" -e "s|@RGS@|$RGS|g" -e "s|@TG@|$TG|g" -e "s|@PROBE@|$P_ROWS|g"
}
pp_to_p() { case "$1" in 00) echo 0.0 ;; 01) echo 0.01 ;; 10) echo 0.1 ;; 50) echo 0.5 ;; 90) echo 0.9 ;; 99) echo 0.99 ;; 100) echo 1.0 ;; *) die "bad pp $1" ;; esac; }

duck_script() {  # duck_script <rendered-sql-file> <log>
  "${DUCK[@]}" -bail -noheader -list -c "SET temp_directory='$SCRATCH/duck-tmp'" -c ".read $1" > "$2" 2>&1 < /dev/null
}

FAILS=0; SELF_FAILS=0; NQ=0; NIDENT=0
record() {  # record <query> <tag> <result> <duck_ms> <engine_ms> <rss_mb> <groups> <out_rows> <detail>
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$@" >> "$TSV"
}

# ---- comparison ------------------------------------------------------------------------------------------------------
CMP_MSG=""
cmp_results() {  # cmp_results <expected (sorted)> <engine file>; 0 identical, 1 different; sets CMP_MSG
  local exp="$1" got="$2" c ln
  if ! LC_ALL=C sort -c "$got" 2> /dev/null; then
    LC_ALL=C sort -T "$TMPD" -S "${SORT_MEM:-2G}" -o "$got.sorted" "$got" && got="$got.sorted"
  fi
  if cmp -s "$exp" "$got"; then CMP_MSG="identical ($(wc -l < "$exp") lines)"; return 0; fi
  c="$(cmp "$exp" "$got" 2>&1 | head -1 | sed -E 's#(/[^ /]+)+/([^ /]+)#\2#g')"
  ln="$(sed -n 's/.*line \([0-9][0-9]*\).*/\1/p' <<< "$c")"; ln="${ln:-1}"
  case "$c" in *EOF*) ln=$((ln + 1)) ;; esac
  CMP_MSG="DIFFERENT: ${c#cmp: }; first differing line $ln: duckdb=[$(sed -n "${ln}p" "$exp" | cut -c1-220)] engine=[$(sed -n "${ln}p" "$got" | cut -c1-220)]"
  return 1
}

DUCK_MS=0; DUCK_ERR=""
SETUP=""
duck_ref() {  # duck_ref <sql> <sorted output file>; sets DUCK_MS
  local raw="$TMPD/duck.raw" t0 t1 rc
  t0=$(date +%s%N)
  "${DUCK[@]}" -bail -noheader -list -separator '|' -nullvalue NULL -c "$SETUP" -c "SET temp_directory='$SCRATCH/duck-tmp'" -c "SET preserve_insertion_order=false" -c "$1" > "$raw" 2> "$raw.err" < /dev/null
  rc=$?
  t1=$(date +%s%N); DUCK_MS=$(((t1 - t0) / 1000000))
  if [ $rc -ne 0 ] || [ -s "$raw.err" ]; then DUCK_ERR="$(head -c 600 "$raw.err")"; rm -f "$raw"; return 1; fi
  LC_ALL=C sort -T "$TMPD" -S "${SORT_MEM:-2G}" -o "$2" "$raw"; rm -f "$raw"
  return 0
}

E_RC=0; E_MS=""; E_RSS=""; E_GROUPS=""; E_OUT=""; E_TAIL=""
engine_run() {  # engine_run <tool> <query> <threads> <outfile> [perturb]; sets E_*
  local tool="$1" q="$2" t="$3" out="$4" pert="${5:-}" so="$TMPD/eng.stdout" se="$TMPD/eng.stderr" tf="$TMPD/eng.time"
  : > "$tf"; rm -f "$out" "$out.sorted"
  if [ "$NO_ENGINE" = 1 ]; then   # stand-in: DuckDB's own answer, optionally perturbed in the text
    cp "$EXP/$q.txt" "$out"; E_RC=0; E_MS=0; E_RSS=0; E_GROUPS=$(wc -l < "$out"); E_OUT=0; E_TAIL=""
    [ -n "$pert" ] && text_perturb "$pert" "$out"
    # validation of the harness itself: corrupt the stand-in's answer (DS_STANDIN_PERTURB=appendchar|count|delline|...)
    [ -n "${DS_STANDIN_PERTURB:-}" ] && [ -s "$out" ] && text_perturb "$DS_STANDIN_PERTURB" "$out"
    return 0
  fi
  if [ -n "$TIMEBIN" ]; then
    "$TIMEBIN" -v -o "$tf" timeout "$TIMEOUT_S" env SHAPES_PERTURB="$pert" "$tool" run "$DATA" "$q" "$t" "$out" > "$so" 2> "$se" < /dev/null
  else
    timeout "$TIMEOUT_S" env SHAPES_PERTURB="$pert" "$tool" run "$DATA" "$q" "$t" "$out" > "$so" 2> "$se" < /dev/null
  fi
  E_RC=$?
  E_MS="$(sed -n 's/.*engine_ms=\([0-9.]*\).*/\1/p' "$so" | tail -1)"
  E_GROUPS="$(sed -n 's/.*groups=\([0-9]*\).*/\1/p' "$so" | tail -1)"
  E_OUT="$(sed -n 's/.*out_rows=\([0-9]*\).*/\1/p' "$so" | tail -1)"
  local kb; kb="$(sed -n 's/.*Maximum resident set size (kbytes): *\([0-9]*\).*/\1/p' "$tf" | tail -1)"
  [ -n "$kb" ] || kb="$(sed -n 's/.*rss_total_kb=\([0-9]*\).*/\1/p' "$so" | tail -1)"
  E_RSS=$(( ${kb:-0} / 1024 ))
  E_TAIL="$(tail -c 500 "$so" | tr '\n' ' ') $(tail -c 400 "$se" | tr '\n' ' ')"
  return 0
}

text_perturb() {  # text_perturb <kind> <file>: deliberate corruption of an engine result file
  local f="$2"
  case "$1" in
    count) sed -i '1s/|[0-9][0-9]*|/|9999999999|/' "$f" ;;
    appendchar) sed -i '1s/$/x/' "$f" ;;
    delline) sed -i '1d' "$f" ;;
    dupline) sed -i '1p' "$f" ;;
    nullkey) sed -i '1s/^[^|]*/NULL/' "$f" ;;
    dropnl) head -c -1 "$f" > "$f.t" && mv "$f.t" "$f" ;;
    empty) : > "$f" ;;
    reorder) tac "$f" > "$f.t" && mv "$f.t" "$f" ;;
    same) : ;;
    *) die "unknown text perturbation $1" ;;
  esac
}

classify() {  # classify <expect> -> sets R (result word) and RDET from E_RC and the comparison
  local expect="$1" out="$2"
  case "$E_RC" in
    0)
      if cmp_results "$EXP/$CUR.txt" "$out"; then R=identical; else R=DIFFERENT; fi
      RDET="$CMP_MSG"
      [ "$expect" = refuse ] && RDET="$RDET [expected a refusal, the engine answered]" ;;
    3) if [ "$expect" = refuse ]; then R=refused-as-expected; else R=REFUSED; fi; RDET="$E_TAIL" ;;
    124) R=TIMEOUT; RDET="no answer in ${TIMEOUT_S}s" ;;
    137) R=CRASH; RDET="killed (rc 137, out of memory?) $E_TAIL" ;;
    134) R=CRASH; RDET="abort (rc 134; allocation failure?) $E_TAIL" ;;
    101) R=CRASH; RDET="panic (rc 101) $E_TAIL" ;;
    4) R=ENGINE-ERROR; RDET="$E_TAIL" ;;
    *) R=CRASH; RDET="rc $E_RC $E_TAIL" ;;
  esac
}

declare -A MS_AT=() OUT_AT=()   # engine ms and joined rows, by "query@threads" (for the chain-scaling report)
CUR=""
run_query() {  # run_query <name> <tag> <expect>
  local name="$1" tag="$2" expect="$3" tool sql out st worst=ok t lastms="" lastrss="" detail="" results="" groups="" outrows=""
  CUR="$name"; NQ=$((NQ + 1))
  tool="$(tool_for "$name")"
  sql="$("$tool" sql "$name")" || die "cannot get the SQL of $name"
  if ! duck_ref "$sql" "$EXP/$name.txt"; then
    say "DUCKDB-ERROR  $name: $DUCK_ERR"; FAILS=$((FAILS + 1))
    record "$name" "$tag" DUCKDB-ERROR "$DUCK_MS" "" "" "" "" "$DUCK_ERR"
    printf '%s\n' "$sql" "$DUCK_ERR" > "$SUMMARY/failures/$name.txt"; return
  fi
  out="$TMPD/$name.engine"
  for t in $THREADS_LIST; do
    engine_run "$tool" "$name" "$t" "$out"
    classify "$expect" "$out"
    results="$results $t:$R"
    case "$R" in identical|refused-as-expected) ;; *) worst=bad
      [ -n "$detail" ] || detail="[threads=$t] $RDET"
      { echo "query $name threads=$t result=$R"; echo "sql: $sql"; echo "$RDET"; } >> "$SUMMARY/failures/$name.txt" ;; esac
    [ -n "$E_MS" ] && { lastms="$E_MS"; MS_AT["$name@$t"]="$E_MS"; }
    OUT_AT["$name@$t"]="${E_OUT:-0}"
    lastrss="$E_RSS"; groups="$E_GROUPS"; outrows="$E_OUT"
    rm -f "$out" "$out.sorted"
    # no point repeating a crash or timeout at every thread count
    case "$R" in CRASH|TIMEOUT) break ;; esac
  done
  local msall=""
  for t in $THREADS_LIST; do msall="$msall ${t}t=${MS_AT[$name@$t]:-na}ms"; done
  if [ "$worst" = ok ]; then
    NIDENT=$((NIDENT + 1))
    local lab=identical; case "$results" in *refused-as-expected*) lab=REFUSED-OK ;; esac
    say "$(printf '%-21s %-34s groups=%-8s duckdb=%sms engine:%s peak_rss=%sMB' "$lab" "$name" "${groups:-0}" "$DUCK_MS" "$msall" "$lastrss")"
    record "$name" "$tag" "ok:$results" "$DUCK_MS" "${lastms:-}" "$lastrss" "${groups:-}" "${outrows:-}" ""
  else
    FAILS=$((FAILS + 1))
    say "$(printf '*** %-17s %-34s' "FAIL" "$name")  $results"
    say "      $detail"
    record "$name" "$tag" "FAIL:$results" "$DUCK_MS" "${lastms:-}" "$lastrss" "${groups:-}" "${outrows:-}" "$detail"
  fi
  # known-bad engines for this query: the comparison MUST report a difference
  local specs="${MUT_BIN[$name]:-}" spec mbin mname
  if [ -n "$specs" ] && [ "$NO_ENGINE" != 1 ]; then
    while [ -n "$specs" ]; do
      spec="${specs%%;*}"; specs="${specs#*;}"; mbin="${spec%%|*}"; mname="${spec#*|}"
      engine_run "$mbin" "$name" 4 "$out"
      if [ "$E_RC" = 0 ] && ! cmp_results "$EXP/$name.txt" "$out"; then
        say "  self-test CAUGHT   mutant '$mname' on $name: $CMP_MSG" | cut -c1-400
        record "selftest:mutant:$mname" "$tag" caught "" "" "" "" "" "$CMP_MSG"
      elif [ "$E_RC" != 0 ]; then
        say "  self-test CAUGHT   mutant '$mname' on $name: engine rc $E_RC ($E_TAIL)" | cut -c1-400
        record "selftest:mutant:$mname" "$tag" caught-by-error "" "" "" "" "" "rc $E_RC"
      else
        say "  *** self-test FAIL mutant '$mname' SURVIVED on $name: results identical, this data set does not see the bug"
        SELF_FAILS=$((SELF_FAILS + 1)); record "selftest:mutant:$mname" "$tag" SURVIVED "" "" "" "" "" ""
      fi
      rm -f "$out" "$out.sorted"
    done
  fi
  [ "$name" = sk_agg_mode_filt ] && cp "$EXP/$name.txt" "$TMPD/self.exp"
  rm -f "$EXP/$name.txt"
}

# ---- self-tests of the comparison -----------------------------------------------------------------------------------
selftest_text() {  # needs $TMPD/self.exp (a real DuckDB answer with several lines)
  local k want f="$TMPD/self.got" lines
  lines=$(wc -l < "$TMPD/self.exp")
  [ "$lines" -ge 3 ] || { say "*** self-test FAIL: reference for the text self-test has only $lines lines"; SELF_FAILS=$((SELF_FAILS + 1)); return; }
  for k in same reorder appendchar count delline dupline nullkey dropnl empty; do
    cp "$TMPD/self.exp" "$f"; text_perturb "$k" "$f"
    case "$k" in same|reorder) want=0 ;; *) want=1 ;; esac
    cmp_results "$TMPD/self.exp" "$f"; local rc=$?
    if [ "$rc" = "$want" ]; then
      say "  self-test ok       text perturbation '$k': $([ $want = 1 ] && echo "reported as a difference" || echo "identical, as it must be (order is not part of the result)")"
    else
      say "  *** self-test FAIL text perturbation '$k': comparison said rc=$rc, wanted $want: $CMP_MSG"; SELF_FAILS=$((SELF_FAILS + 1))
    fi
  done
}
selftest_engine() {  # the real engine with SHAPES_PERTURB: its corrupted output must be reported as different
  [ "$NO_ENGINE" = 1 ] && return
  local k out="$TMPD/self.engine"
  for k in count sum drop key; do
    engine_run "$SHAPES" sk_agg_mode_filt 4 "$out" "$k"
    if [ "$E_RC" = 0 ] && ! cmp_results "$TMPD/self.exp" "$out"; then
      say "  self-test ok       engine output perturbed ('$k'): reported as a difference ($CMP_MSG)" | cut -c1-300
    else
      say "  *** self-test FAIL engine output perturbed ('$k') was NOT reported as a difference (rc $E_RC)"; SELF_FAILS=$((SELF_FAILS + 1))
    fi
    rm -f "$out" "$out.sorted"
  done
}

# ---- data profile checks ---------------------------------------------------------------------------------------------
GEN_FAILS=0
check_profile() {  # check_profile <check sql file>: rows of name|value|lo|hi; every value must be within [lo, hi]
  local f="$TMPD/check.sql" l
  render "$1" > "$f"
  "${DUCK[@]}" -bail -noheader -list -separator '|' -nullvalue NULL < "$f" > "$TMPD/check.out" 2> "$TMPD/check.err" \
    || { cat "$TMPD/check.err"; die "profile query $1 failed"; }
  while IFS='|' read -r n v lo hi; do
    [ -n "$n" ] || continue
    if awk -v v="$v" -v lo="$lo" -v hi="$hi" 'BEGIN { exit !(v != "NULL" && v + 0 >= lo + 0 && v + 0 <= hi + 0) }'; then
      say "  data ok            $n = $v (expected $lo .. $hi)"
    else
      say "  *** DATA CHECK FAIL $n = $v (expected $lo .. $hi): the generated data does not have the intended shape"; GEN_FAILS=$((GEN_FAILS + 1))
    fi
  done < "$TMPD/check.out"
}

PEAK_DISK_KB=0
note_disk() { local kb; kb=$(du -sk "$SCRATCH" | cut -f1); [ "$kb" -gt "$PEAK_DISK_KB" ] && PEAK_DISK_KB=$kb; return 0; }

run_tag() {
  local tag="$1" t0=$SECONDS spec file
  if [ -n "${ONLY_TAGS:-}" ] && ! grep -qw -- "$tag" <<< "$ONLY_TAGS"; then return; fi
  say ""; say "=== data set: $tag  (N=$N)  $(date -u +%H:%M:%SZ) ==="
  rm -f "$DATA"/*.parquet
  NAME=""; PP=""; P=0; PK=0
  case "$tag" in
    skew) render gen_skew.sql > "$TMPD/gen.sql" ;;
    hot) render gen_hot.sql > "$TMPD/gen.sql" ;;
    str) render gen_str.sql > "$TMPD/gen.sql" ;;
    nulshape) render gen_nuldims.sql gen_nulshape.sql > "$TMPD/gen.sql" ;;
    nul*) PP="${tag#nul}"; P="$(pp_to_p "$PP")"; render gen_nuldims.sql gen_nul.sql > "$TMPD/gen.sql" ;;
    linullkey) NAME=nullkey; P=0.1; PK=0.1; render gen_li.sql > "$TMPD/gen.sql" ;;
    li*) PP="${tag#li}"; NAME="p$PP"; P="$(pp_to_p "$PP")"; PK=0; render gen_li.sql > "$TMPD/gen.sql" ;;
    *) die "unknown tag $tag" ;;
  esac
  local g0=$SECONDS
  duck_script "$TMPD/gen.sql" "$TMPD/gen.log" || { tail -20 "$TMPD/gen.log"; die "data generation for $tag failed"; }
  say "generated in $((SECONDS - g0)) s: $(cd "$DATA" && ls -l *.parquet | awk '{printf "%s %.1f MB; ", $9, $5/1048576}')"
  note_disk
  case "$tag" in
    skew) check_profile check_skew.sql ;;
    hot) check_profile check_hot.sql ;;
    str) check_profile check_str.sql ;;
    nulshape) check_profile check_nulshape.sql ;;
    nul*) check_profile check_nul.sql ;;
    li*) check_profile check_li.sql ;;
  esac
  SETUP=""
  for file in "$DATA"/*.parquet; do SETUP+="CREATE OR REPLACE VIEW $(basename "$file" .parquet) AS SELECT * FROM read_parquet('$file');"; done
  local qn qt qe ql
  mapfile -t ql < <(list_queries "$tag")
  [ "${#ql[@]}" -gt 0 ] || die "no queries listed for $tag"
  for line in "${ql[@]}"; do
    IFS=$'\t' read -r qn qt qe <<< "$line"
    run_query "$qn" "$qt" "$qe"
    note_disk
  done
  if [ "$tag" = skew ]; then selftest_text; selftest_engine; fi
  rm -f "$DATA"/*.parquet "$EXP"/* "$TMPD"/self.* 2> /dev/null
  say "--- $tag done in $((SECONDS - t0)) s"
}

# ---- main -------------------------------------------------------------------------------------------------------------
build_mutants
for tag in skew hot str nul00 nul01 nul10 nul50 nul90 nul99 nul100 nulshape li00 li01 li10 li50 li90 li99 li100 linullkey; do
  run_tag "$tag"
done

# ---- chain-length scaling (a quadratic engine shows as ns per joined row growing with the chain) -------------------
say ""; say "=== hot key on the build side: time per joined row (chain 30% of the build rows) ==="
for t in $THREADS_LIST; do
  s="${MS_AT[hb_chain_small@$t]:-}"; b="${MS_AT[hb_chain_big@$t]:-}"; so="${OUT_AT[hb_chain_small@$t]:-0}"; bo="${OUT_AT[hb_chain_big@$t]:-0}"
  [ -n "$s" ] && [ -n "$b" ] && [ "${so:-0}" -gt 0 ] && [ "${bo:-0}" -gt 0 ] || continue
  r="$(awk -v s="$s" -v b="$b" -v so="$so" -v bo="$bo" 'BEGIN { ns = s*1e6/so; nb = b*1e6/bo; printf "%.1f %.1f %.2f", ns, nb, nb/ns }')"
  set -- $r
  verdict=ok
  awk -v x="$3" 'BEGIN { exit !(x > 4) }' && { verdict="QUADRATIC? (ratio > 4)"; FAILS=$((FAILS + 1)); }
  awk -v x="$3" 'BEGIN { exit !(x > 2 && x <= 4) }' && verdict="WARN (ratio > 2: look at it)"
  say "  threads=$t: build chain B/4 -> $so joined rows, $1 ns/row; chain B -> $bo joined rows, $2 ns/row; ratio $3  [$verdict]"
done

# ---- summary -----------------------------------------------------------------------------------------------------------
say ""
say "=== summary ==="
say "queries run: $NQ; identical to DuckDB at every thread count (or refused as expected): $NIDENT; failing: $FAILS; self-test failures: $SELF_FAILS; data-shape check failures: $GEN_FAILS"
say "peak scratch disk: $((PEAK_DISK_KB / 1048576)) GB ($((PEAK_DISK_KB / 1024)) MB); wall time $(( (SECONDS - START_S) / 60 )) min"
if [ "$FAILS" -eq 0 ] && [ "$SELF_FAILS" -eq 0 ] && [ "$GEN_FAILS" -eq 0 ]; then
  say "RESULT: ALL IDENTICAL to DuckDB; self-tests behaved"; exit 0
fi
say "RESULT: FAILURES (see the lines marked *** above, failures/<query>.txt, results.tsv)"; exit 1
