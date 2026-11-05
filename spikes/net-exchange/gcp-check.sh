#!/usr/bin/env bash
# gcp-check.sh <results-dir>
#
# Verification-plan item 28, stage A: the REAL TCP exchange (flow control, cancellation, ordering, liveness) between N node PROCESSES,
# each in its own Linux network namespace, joined by a bridge, with `tc netem` delay / loss / rate on every node's egress. One VM.
#   1. build in release, run the unit tests (tests/net.rs: wire format, hostile peers, backpressure, cancel, crash, silence, shuffle)
#   2. the mutants (mutants/run.sh): each deliberate bug must be caught by a test                                       [NX_MUTANTS=0 skips]
#   3. build the network: bridge nxbr0, namespaces nx0..nxN-1, veth pairs, 10.77.0.1x/24; check that netem really delays (ping RTT)
#   4. S1 correctness and throughput under 6 network profiles, 3 reps each; every node's RESULT must equal `netx expect`; bounds checked
#   5. S2 backpressure: slow consumers, small credit, wide-area profile: results exact, buffered and in-flight bytes <= credit, senders waited
#   6. S3 credit sweep: throughput against credit size on a 20 ms round-trip path (the bandwidth-delay product, measured)
#   7. S4 cancel: one node cancels mid-query: every node must stop, how long it takes
#   8. S5 kill -9 of one node mid-query: every survivor must fail fast (never a wrong answer, never a hang), how long it takes
#   9. S6 network partition (the link of one node goes down, sockets stay open): only the liveness timer can notice; time to detect
# The in-process baseline (`netx inproc`: same shuffle, threads and bounded channels, no sockets) is run next to S1.
#
# Everything here is labelled SIMULATED NETWORK: namespaces on one VM (loopback-class bandwidth, netem delay and loss), not a real network
# between machines (stage B). Timings count only from the run itself; the VM's load is printed before each profile.
#
# Needs root (or passwordless sudo) for `ip netns` and `tc`, iproute2 with netem (the kernel module sch_netem), the Rust toolchain.
# Environment: NX_NODES (4), NX_ROWS (rows per node for S1, default 20000000), NX_REPS (3), NX_QUICK=1 (small sizes and fewer profiles,
# for a development machine), NX_MUTANTS (1), NX_SUDO (default "sudo", empty when already root), NX_DEAD_MS (3000, liveness timeout of S6).
# Exit status: 0 all checks passed, 1 a check failed, 4 setup failed (no netns/netem, build failed).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?usage: gcp-check.sh <results-dir>}
mkdir -p "$OUT/runs"; OUT=$(cd "$OUT" && pwd)
N=${NX_NODES:-4}
QUICK=${NX_QUICK:-0}
REPS=${NX_REPS:-3}; ROWS=${NX_ROWS:-20000000}
if [ "$QUICK" = 1 ]; then REPS=1; ROWS=${NX_ROWS:-1000000}; fi
DEAD_MS=${NX_DEAD_MS:-3000}
if [ -n "${NX_SUDO+x}" ]; then SUDO=$NX_SUDO; elif [ "$(id -u)" = 0 ]; then SUDO=""; else SUDO=sudo; fi
NETX="$HERE/target/release/netx"
# Anchored at the start of the command line: it matches the netx processes and NOT the shell, sudo or tool that mention the name.
RUN_BIN=$NETX
NPAT="^$HERE/target[a-z-]*/release/netx node"
SUMMARY="$OUT/summary.txt"; : > "$SUMMARY"
RESULTS_MD="$OUT/results.md"; : > "$RESULTS_MD"
FAILED=0
say() { printf '\n==== %s ====\n' "$*" | tee -a "$SUMMARY"; }
log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
md() { printf '%s\n' "$*" >> "$RESULTS_MD"; }
check() { # check NAME 0|1 [detail]
  if [ "$2" = 0 ]; then log "CHECK $1: PASS ${3:-}"; else log "CHECK $1: FAIL ${3:-}"; FAILED=$((FAILED + 1)); fi
}
now_ns() { date +%s%N; }

# ----------------------------------------------------------------------------------------------------------------- 0. machine
{
  date -u +%FT%TZ; uname -srm
  lscpu | grep -E 'Model name|Architecture|^CPU\(s\)' || true
  free -g | head -2
  ip -V 2>&1 | head -1
  echo "git: $(git -C "$HERE" rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "nodes=$N rows=$ROWS reps=$REPS quick=$QUICK dead_ms=$DEAD_MS sudo='${SUDO}'"
} | tee "$OUT/machine.txt" >> "$SUMMARY"

# ----------------------------------------------------------------------------------------------------------------- 1. build + tests
say "1. build and unit tests"
( cd "$HERE" && cargo build --release 2>&1 | tail -3 ) | tee "$OUT/build.txt"
if [ ! -x "$NETX" ]; then log "SETUP FAILED: the build produced no netx"; log "VERDICT FAIL (setup)"; exit 4; fi
( cd "$HERE" && cargo test --release 2>&1 ) > "$OUT/unit-tests.txt"; rc=$?
grep -E "^test result|FAILED|panicked" "$OUT/unit-tests.txt" | tee -a "$SUMMARY"
check unit-tests $rc "(tests/net.rs and the wire tests)"

# ----------------------------------------------------------------------------------------------------------------- 2. mutants
if [ "${NX_MUTANTS:-1}" = 1 ] && [ "$QUICK" != 1 ]; then
  say "2. mutants of the exchange"
  "$HERE/mutants/run.sh" "$OUT/mutants.txt" > /dev/null 2>&1; rc=$?
  cat "$OUT/mutants.txt" | tee -a "$SUMMARY"
  check mutants $rc "(each deliberate bug caught by a test)"
else
  log "2. mutants: skipped (NX_MUTANTS=0 or NX_QUICK=1)"
fi

# ----------------------------------------------------------------------------------------------------------------- 3. network
say "3. network namespaces"
net_down() {
  for i in $(seq 0 $((N - 1))); do $SUDO ip netns del "nx$i" 2>/dev/null; $SUDO ip link del "vx$i" 2>/dev/null; done
  $SUDO ip link del nxbr0 2>/dev/null
  return 0
}
cleanup() { $SUDO pkill -9 -f "$NPAT" 2>/dev/null; sleep 0.3; net_down; }
trap cleanup EXIT
net_down
ok=1
$SUDO ip link add nxbr0 type bridge && $SUDO ip link set nxbr0 up || ok=0
for i in $(seq 0 $((N - 1))); do
  $SUDO ip netns add "nx$i" \
    && $SUDO ip link add "vx$i" type veth peer name eth0 netns "nx$i" \
    && $SUDO ip link set "vx$i" master nxbr0 && $SUDO ip link set "vx$i" up \
    && $SUDO ip netns exec "nx$i" ip addr add "10.77.0.$((10 + i))/24" dev eth0 \
    && $SUDO ip netns exec "nx$i" ip link set eth0 up && $SUDO ip netns exec "nx$i" ip link set lo up || ok=0
done
if [ "$ok" != 1 ]; then log "SETUP FAILED: could not create the namespaces or the bridge (root? iproute2?)"; log "VERDICT FAIL (setup)"; exit 4; fi
# Segmentation offload would let netem drop a whole 64 KiB super-segment as ONE packet and make loss unrealistically coarse.
# ethtool lives in /usr/sbin, which is NOT on a normal user's PATH (found 2026-11-05: `command -v ethtool` said "not installed" on VMs where it
# may have been installed), so it is looked for and run through sudo, as `ip` and `tc` are.
ETHTOOL=0
$SUDO ethtool --version >/dev/null 2>&1 && ETHTOOL=1
if [ "$ETHTOOL" = 1 ]; then
  for i in $(seq 0 $((N - 1))); do
    $SUDO ip netns exec "nx$i" ethtool -K eth0 tso off gso off gro off >/dev/null 2>&1
    $SUDO ethtool -K "vx$i" tso off gso off gro off >/dev/null 2>&1
  done
  # read the state back: do not claim "off" without having seen it
  OFFLOAD_STATE=$($SUDO ip netns exec nx0 ethtool -k eth0 2>/dev/null | grep -E "^(tcp-segmentation|generic-segmentation|generic-receive)-offload" | tr -s ' \n' ' ')
  log "offload state of nx0 eth0 (read back): ${OFFLOAD_STATE:-unreadable}"
  echo "$OFFLOAD_STATE" | grep -q "offload: off" && ! echo "$OFFLOAD_STATE" | grep -q "offload: on"
  check offloads-off $? "(tso, gso and gro of the veth devices)"
else
  log "WARNING: ethtool is not available (not even through sudo): segmentation offload is on, so netem loss applies to super-segments (coarser than real packet loss)"
fi
profile() { # profile NAME "netem args" ; applied to the egress of every node
  PROFILE_NAME=$1
  for i in $(seq 0 $((N - 1))); do
    if [ -z "$2" ]; then $SUDO ip netns exec "nx$i" tc qdisc del dev eth0 root 2>/dev/null; true
    else $SUDO ip netns exec "nx$i" tc qdisc replace dev eth0 root netem $2 || return 1; fi
  done
}
rtt_ms() { $SUDO ip netns exec nx0 ping -c 5 -i 0.2 -q 10.77.0.11 2>/dev/null | awk -F'[/ ]' '/^rtt|^round-trip/ {print $8}'; }
profile clean ""
r0=$(rtt_ms); log "ping RTT nx0 -> nx1, no netem: ${r0:-?} ms"
profile selftest "delay 10ms"
r1=$(rtt_ms); log "ping RTT nx0 -> nx1, netem delay 10 ms on both egresses: ${r1:-?} ms (expected about 20)"
awk -v r="${r1:-0}" 'BEGIN { exit !(r >= 18 && r <= 30) }'; check netem-works $? "(RTT ${r1:-?} ms)"
profile clean ""
md "# Item 28 stage A: real TCP exchange between $N processes in network namespaces (SIMULATED NETWORK)"
md ""
md "Machine: $(grep -m1 'Model name' "$OUT/machine.txt" | sed 's/.*: *//'), $(nproc) vCPU, $(uname -r). Rows per node: $ROWS (16 bytes each), $N nodes, $REPS reps."
md "Ping RTT without netem: ${r0:-?} ms. Offloads off (read back): $([ "$ETHTOOL" = 1 ] && echo yes || echo NO)."

# ----------------------------------------------------------------------------------------------------------------- helpers
declare -a PIDS
addrs() { for i in $(seq 0 $((N - 1))); do printf '10.77.0.%d:7000,' $((10 + i)); done | sed 's/,$//'; }
launch() { # launch TAG args... : N node processes in the background; files in runs/TAG/node<i>.{out,err,exit}
  local tag=$1; shift
  RUN="$OUT/runs/$tag"; rm -rf "$RUN"; mkdir -p "$RUN"; PIDS=()
  for i in $(seq 0 $((N - 1))); do
    ( $SUDO ip netns exec "nx$i" "$RUN_BIN" node --me "$i" --addrs "$(addrs)" "$@" > "$RUN/node$i.out" 2> "$RUN/node$i.err"; echo "$? $(now_ns)" > "$RUN/node$i.exit" ) &
    PIDS+=($!)
  done
}
wait_nodes() { local p; for p in "${PIDS[@]}"; do wait "$p"; done; }
kv() { sed -n "s/.*[ ]$2=\\([^ ]*\\).*/\\1/p" "$1" | head -1; }   # kv FILE KEY
exit_code() { cut -d' ' -f1 "$RUN/node$1.exit" 2>/dev/null || echo 255; }
exit_ns() { cut -d' ' -f2 "$RUN/node$1.exit" 2>/dev/null || echo 0; }
load1() { cut -d' ' -f1 /proc/loadavg; }

# S1 helper: expected results are a pure function of (N, rows, seed): compute once per size
declare -A EXPECT_DONE
expect_for() { # expect_for ROWS -> file
  local f="$OUT/expect-$1.txt"
  [ -s "$f" ] || "$NETX" expect --nodes "$N" --rows "$1" > "$f"
  echo "$f"
}
verify_run() { # verify_run ROWS CREDIT -> 0 if every node's RESULT equals EXPECT and the byte bounds hold; prints the reason otherwise
  local rows=$1 credit=$2 ef; ef=$(expect_for "$rows")
  for i in $(seq 0 $((N - 1))); do
    [ "$(exit_code "$i")" = 0 ] || { echo "node $i exited with $(exit_code "$i"): $(head -c 200 "$RUN/node$i.out")"; return 1; }
    local want_r want_c got_r got_c pb pf
    want_r=$(grep "EXPECT node=$i " "$ef" | sed -n 's/.*rows_in=\([0-9]*\).*/\1/p'); want_c=$(grep "EXPECT node=$i " "$ef" | sed -n 's/.*checksum=\([0-9]*\).*/\1/p')
    got_r=$(kv "$RUN/node$i.out" rows_in); got_c=$(kv "$RUN/node$i.out" checksum)
    [ "$got_r" = "$want_r" ] && [ "$got_c" = "$want_c" ] || { echo "node $i: rows $got_r checksum $got_c, expected $want_r $want_c"; return 1; }
    pb=$(kv "$RUN/node$i.out" peak_buffered); pf=$(kv "$RUN/node$i.out" peak_inflight)
    [ "${pb:-0}" -le "$credit" ] && [ "${pf:-0}" -le "$credit" ] || { echo "node $i: peak buffered $pb / in flight $pf above the credit $credit"; return 1; }
  done
  return 0
}
run_secs() { local m=0 s; for i in $(seq 0 $((N - 1))); do s=$(kv "$RUN/node$i.out" secs); m=$(awk -v a="$m" -v b="${s:-0}" 'BEGIN{print (b>a)?b:a}'); done; echo "$m"; }
bytes_moved() { local t=0 b; for i in $(seq 0 $((N - 1))); do b=$(kv "$RUN/node$i.out" bytes_sent); t=$((t + ${b:-0})); done; echo "$t"; }
median() { sort -n | awk '{a[NR]=$1} END{ if (NR==0) print "n/a"; else print a[int((NR+1)/2)] }'; }

# ----------------------------------------------------------------------------------------------------------------- S1
say "S1. correctness and throughput under network profiles ($REPS reps each)"
CREDIT=$((4 << 20))
if [ "$QUICK" = 1 ]; then
  PROFILES=("clean|" "region|delay 10ms 2ms loss 0.05%")
else
  PROFILES=("clean|" "lan|delay 0.2ms" "zone|delay 1ms 0.2ms" "region|delay 10ms 2ms loss 0.05%" "wide|delay 25ms 5ms loss 1%" "capped|delay 10ms rate 400mbit")
fi
md ""; md "## S1. Correctness and throughput (SIMULATED NETWORK, credit $((CREDIT >> 20)) MiB per stream)"; md ""
md "| profile | netem on each egress | load at start | secs (median of $REPS) | MB/s moved, all nodes (median) | all runs exact | blocked waits (sum, last rep) |"
md "|---|---|---|---|---|---|---|"
inproc_line=$("$NETX" inproc --nodes "$N" --rows "$ROWS" --credit-bytes "$CREDIT")
log "$inproc_line"
for pr in "${PROFILES[@]}"; do
  name=${pr%%|*}; args=${pr#*|}
  profile "$name" "$args" || { check "profile-$name" 1 "(tc failed)"; continue; }
  secs_list=""; mbs_list=""; exact=0; why=""; waits=0
  l0=$(load1)
  for rep in $(seq 1 "$REPS"); do
    t0=$(now_ns)
    launch "s1-$name-$rep" --rows "$ROWS" --credit-bytes "$CREDIT" --dead-ms 20000 --connect-timeout-s 60
    wait_nodes
    wall=$(awk -v a="$t0" -v b="$(now_ns)" 'BEGIN{printf "%.3f", (b-a)/1e9}')
    why=$(verify_run "$ROWS" "$CREDIT") || { exact=1; log "S1 $name rep $rep: $why"; }
    s=$(run_secs); b=$(bytes_moved)
    secs_list="$secs_list $s"; mbs_list="$mbs_list $(awk -v b="$b" -v s="$s" 'BEGIN{printf "%.0f", b/1e6/s}')"
    waits=0; for i in $(seq 0 $((N - 1))); do w=$(kv "$RUN/node$i.out" blocked_waits); waits=$((waits + ${w:-0})); done
  done
  ms=$(echo $secs_list | tr ' ' '\n' | median); mb=$(echo $mbs_list | tr ' ' '\n' | median)
  md "| $name | ${args:-none} | $l0 | $ms | $mb | $([ $exact = 0 ] && echo yes || echo "**NO**") | $waits |"
  check "s1-$name-exact" $exact "(every node's rows and checksum equal the expected, buffered/in-flight <= credit)"
done
md ""; md "In-process baseline (threads, bounded channels, no sockets, same shuffle): \`$inproc_line\`"
profile clean ""

# ----------------------------------------------------------------------------------------------------------------- S1b
say "S1b. process exit race: many short runs on a path with delay (found 2026-11-05)"
# A process that exits right after sending its last frame closes sockets that still hold unread credit frames: Linux then sends a RST and
# DROPS the data it has not yet transmitted (the end-of-stream frame included), so the peer sees "connection reset" on a correct run.
# It needs latency to show (unsent data lingers); on loopback it almost never does. Each run is tiny; every one must be exact.
profile region "delay 10ms 2ms loss 0.05%"
RUNS=${NX_EXIT_RUNS:-30}; [ "$QUICK" = 1 ] && RUNS=${NX_EXIT_RUNS:-12}
exit_race() { # exit_race RUNS -> sets EXIT_BAD (runs that were not exact) and EXIT_FIRST
  EXIT_BAD=0; EXIT_FIRST=""
  local r why
  for r in $(seq 1 "$1"); do
    launch "s1b-$r" --rows 100000 --batch-rows $((N * 512)) --credit-bytes $((1 << 20)) --dead-ms 20000 --connect-timeout-s 60
    wait_nodes
    why=$(verify_run 100000 $((1 << 20))) || { EXIT_BAD=$((EXIT_BAD + 1)); [ -z "$EXIT_FIRST" ] && EXIT_FIRST="run $r: $why"; }
  done
}
exit_race "$RUNS"
check s1b-exit-race-$RUNS-runs-all-exact $([ "$EXIT_BAD" = 0 ] && echo 0 || echo 1) "($EXIT_BAD of $RUNS runs failed; ${EXIT_FIRST:-none})"
md ""; md "## S1b. Process-exit race: $RUNS short runs on a 10 ms path, $EXIT_BAD failed."
# S1c: the same runs against a build WITHOUT the fix (feature bug-no-linger): the stage must be able to fail, or it proves nothing
if [ "${NX_MUTANTS:-1}" = 1 ]; then
  ( cd "$HERE" && cargo build --release --features bug-no-linger --target-dir target-mut 2>&1 | tail -1 )
  if [ -x "$HERE/target-mut/release/netx" ]; then
    RUN_BIN="$HERE/target-mut/release/netx"; MRUNS=$((RUNS > 25 ? RUNS : 25)); exit_race "$MRUNS"; RUN_BIN=$NETX
    check s1c-exit-race-detector-catches-bug-no-linger $([ "$EXIT_BAD" -gt 0 ] && echo 0 || echo 1) "(the build without the fix failed $EXIT_BAD of $MRUNS runs; the stage must see at least one)"
    md ""; md "S1c. The same runs against a build without the fix (feature bug-no-linger): $EXIT_BAD of $MRUNS runs failed."
  else
    check s1c-exit-race-detector-catches-bug-no-linger 1 "(the mutant build failed)"
  fi
fi
profile clean ""

# ----------------------------------------------------------------------------------------------------------------- S1d
say "S1d. how much does a random-loss profile vary from run to run?"
# The same job gave 980, 82 and 698 MB/s on the region profile (10 ms + 0.05% loss) on three runs (2026-11-05). Repeat it and report the spread,
# so a number on a lossy profile is never quoted without it. Informational: not a pass/fail check, apart from every run being exact.
profile region "delay 10ms 2ms loss 0.05%"
VR=${NX_VAR_RUNS:-8}; [ "$QUICK" = 1 ] && VR=${NX_VAR_RUNS:-4}
VROWS=$((ROWS / 4)); [ "$QUICK" = 1 ] && VROWS=250000
var_list=""; var_bad=0
for r in $(seq 1 "$VR"); do
  launch "s1d-$r" --rows "$VROWS" --credit-bytes "$CREDIT" --dead-ms 20000 --connect-timeout-s 60
  wait_nodes
  verify_run "$VROWS" "$CREDIT" >/dev/null || var_bad=$((var_bad + 1))
  s=$(run_secs); b=$(bytes_moved)
  var_list="$var_list $(awk -v b="$b" -v s="$s" 'BEGIN{printf "%.0f", b/1e6/s}')"
done
sorted=$(echo $var_list | tr ' ' '\n' | sort -n | tr '\n' ' ')
vmin=$(echo $sorted | awk '{print $1}'); vmax=$(echo $sorted | awk '{print $NF}'); vmed=$(echo $var_list | tr ' ' '\n' | median)
log "S1d region profile, $VR runs of $VROWS rows per node: MB/s moved, sorted: $sorted (min $vmin, median $vmed, max $vmax; max/min $(awk -v a="$vmax" -v b="$vmin" 'BEGIN{ if (b>0) printf "%.1f", a/b; else print "n/a" }'))"
check s1d-loss-runs-all-exact $([ "$var_bad" = 0 ] && echo 0 || echo 1) "($var_bad of $VR runs inexact)"
md ""; md "## S1d. Spread of the random-loss profile (region: 10 ms, 0.05% loss), $VR runs of $VROWS rows per node"; md ""
md "MB/s moved, sorted: $sorted. Min $vmin, median $vmed, max $vmax."
profile clean ""

# ----------------------------------------------------------------------------------------------------------------- S2
say "S2. backpressure: slow consumers, small credit, wide-area profile"
profile region "delay 10ms 2ms loss 0.05%"
SC=$((256 << 10)); SR=$((ROWS / 4)); [ "$QUICK" = 1 ] && SR=200000
launch s2 --rows "$SR" --batch-rows $((N * 1024)) --credit-bytes "$SC" --consumer-delay-us 300 --dead-ms 20000 --connect-timeout-s 60
wait_nodes
why=$(verify_run "$SR" "$SC"); rc=$?
w=0; maxb=0; for i in $(seq 0 $((N - 1))); do x=$(kv "$RUN/node$i.out" blocked_waits); w=$((w + ${x:-0})); y=$(kv "$RUN/node$i.out" peak_buffered); [ "${y:-0}" -gt "$maxb" ] && maxb=$y; done
check s2-exact-and-bounded $rc "$why (max inbound queue $maxb bytes of $SC allowed)"
check s2-senders-waited $([ "$w" -gt 0 ] && echo 0 || echo 1) "(blocked waits $w: the consumers were slow enough to push back)"
md ""; md "## S2. Backpressure"; md ""
md "Slow consumers (300 microseconds per batch), credit $((SC >> 10)) KiB, region profile: results exact = $([ $rc = 0 ] && echo yes || echo NO), largest inbound queue $maxb bytes (limit $SC), blocked waits $w."
profile clean ""

# ----------------------------------------------------------------------------------------------------------------- S3
say "S3. credit sweep on a 20 ms round-trip path"
profile sweep "delay 10ms"
md ""; md "## S3. Throughput against credit size (netem delay 10 ms each way, RTT about 20 ms)"; md ""
md "| credit per stream | secs | MB/s moved, all nodes | exact |"; md "|---|---|---|---|"
SWR=$((ROWS / 2)); [ "$QUICK" = 1 ] && SWR=500000
for c in 65536 262144 1048576 4194304 16777216; do
  launch "s3-$c" --rows "$SWR" --batch-rows $((N * 1024)) --credit-bytes "$c" --dead-ms 20000 --connect-timeout-s 60
  wait_nodes
  why=$(verify_run "$SWR" "$c"); rc=$?
  s=$(run_secs); b=$(bytes_moved)
  md "| $((c >> 10)) KiB | $s | $(awk -v b="$b" -v s="$s" 'BEGIN{printf "%.0f", b/1e6/s}') | $([ $rc = 0 ] && echo yes || echo "**NO**: $why") |"
  check "s3-credit-$c-exact" $rc "$why"
done
profile clean ""

# ----------------------------------------------------------------------------------------------------------------- S4
say "S4. cancel: node 0 cancels after 300 batches"
BIG=400000000
t0=$(now_ns)
launch s4 --rows "$BIG" --batch-rows $((N * 1024)) --credit-bytes $((1 << 20)) --cancel-at 0:300 --dead-ms 20000 --connect-timeout-s 60
wait_nodes
cancel_ok=0; first=0
for i in $(seq 0 $((N - 1))); do
  rc=$(exit_code "$i"); el=$(awk -v a="$t0" -v b="$(exit_ns "$i")" 'BEGIN{printf "%.2f", (b-a)/1e9}')
  log "S4 node $i: exit $rc after ${el} s: $(head -c 160 "$RUN/node$i.out")"
  if [ "$i" = 0 ]; then [ "$rc" = 3 ] || cancel_ok=1; else [ "$rc" = 3 ] || [ "$rc" = 4 ] || cancel_ok=1; fi
  awk -v e="$el" 'BEGIN{exit !(e < 15)}' || cancel_ok=1
done
check s4-cancel-stops-every-node $cancel_ok "(node 0 exits 3, the others 3 or 4, all within 15 s although each had 400 million rows to send)"
leak=$(pgrep -f "$NPAT" | wc -l); check s4-no-process-left $([ "$leak" = 0 ] && echo 0 || echo 1) "($leak left)"

# ----------------------------------------------------------------------------------------------------------------- S5
say "S5. kill -9 of node $((N - 1)) mid-query"
md ""; md "## S5/S6. Failures (SIMULATED NETWORK)"; md ""
md "| scenario | profile | node | exit code | seconds after the fault | result line |"; md "|---|---|---|---|---|---|"
for pr in "clean|" "region|delay 10ms 2ms loss 0.05%"; do
  name=${pr%%|*}; args=${pr#*|}; profile "$name" "$args"
  launch "s5-$name" --rows "$BIG" --batch-rows $((N * 1024)) --credit-bytes $((1 << 20)) --dead-ms 20000 --connect-timeout-s 60
  sleep 3
  tk=$(now_ns); $SUDO pkill -9 -f "$NPAT --me $((N - 1)) "; wait_nodes
  bad=0
  for i in $(seq 0 $((N - 1))); do
    rc=$(exit_code "$i"); el=$(awk -v a="$tk" -v b="$(exit_ns "$i")" 'BEGIN{printf "%.2f", (b-a)/1e9}')
    md "| kill -9 node $((N - 1)) | $name | $i | $rc | $el | $(head -c 120 "$RUN/node$i.out" | tr '|' '/') |"
    if [ "$i" = $((N - 1)) ]; then continue; fi
    [ "$rc" = 4 ] || [ "$rc" = 3 ] || bad=1                       # fails with peer-lost (or cancelled), never 0 (a wrong or partial answer) and never a hang
    awk -v e="$el" 'BEGIN{exit !(e < 5)}' || bad=1               # RST/FIN arrive at once: 5 s is generous
  done
  check "s5-kill-$name-survivors-fail-fast" $bad "(every survivor exits 4 or 3 within 5 s; none exits 0)"
done
profile clean ""

# ----------------------------------------------------------------------------------------------------------------- S6
say "S6. partition: node $((N - 1))'s link goes down, sockets stay open (liveness timeout ${DEAD_MS} ms)"
launch s6 --rows "$BIG" --batch-rows $((N * 1024)) --credit-bytes $((1 << 20)) --dead-ms "$DEAD_MS" --ping-ms 250 --connect-timeout-s 60
sleep 3
tp=$(now_ns); $SUDO ip netns exec "nx$((N - 1))" ip link set eth0 down; $SUDO ip link set "vx$((N - 1))" down
wait_nodes
bad=0; maxel=0
for i in $(seq 0 $((N - 1))); do
  rc=$(exit_code "$i"); el=$(awk -v a="$tp" -v b="$(exit_ns "$i")" 'BEGIN{printf "%.2f", (b-a)/1e9}')
  md "| partition node $((N - 1)) (dead after ${DEAD_MS} ms) | clean | $i | $rc | $el | $(head -c 120 "$RUN/node$i.out" | tr '|' '/') |"
  [ "$rc" = 4 ] || [ "$rc" = 3 ] || bad=1
  maxel=$(awk -v a="$maxel" -v b="$el" 'BEGIN{print (b>a)?b:a}')
  awk -v e="$el" -v d="$DEAD_MS" 'BEGIN{exit !(e >= d/1000 - 0.5 && e < d/1000 + 5)}' || bad=1   # not before the timeout, and not much after it
done
check s6-partition-detected-by-liveness-timer $bad "(all nodes exit 4 or 3 between ${DEAD_MS} ms - 0.5 s and ${DEAD_MS} ms + 5 s; slowest $maxel s)"
$SUDO ip link set "vx$((N - 1))" up
leak=$(pgrep -f "$NPAT" | wc -l); check no-process-left $([ "$leak" = 0 ] && echo 0 || echo 1) "($leak left)"

# ----------------------------------------------------------------------------------------------------------------- verdict
say "verdict"
cp "$RESULTS_MD" "$OUT/results-table.md" 2>/dev/null
if [ "$FAILED" -eq 0 ]; then log "VERDICT PASS (every check passed; SIMULATED NETWORK on one VM, not a network between machines)"; exit 0; fi
log "VERDICT FAIL ($FAILED check(s) failed; see CHECK lines above)"
exit 1
