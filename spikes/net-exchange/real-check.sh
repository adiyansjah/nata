#!/usr/bin/env bash
# real-check.sh <results-dir>
#
# Verification-plan item 28, STAGE B: the same TCP exchange as stage A (gcp-check.sh), but between SEPARATE VMs on a real network (one
# zone, the project's default network). Runs on the DRIVER VM of a multi-VM run (orchestrate.sh --peers K): /mnt/work/PEERS lists the
# internal addresses (the driver's own first), /mnt/work/peer_key is the run's throwaway key, which every peer accepts for the user nata.
# Node 0 runs on the driver, node i on peer i. A node's exit time is taken ON THE DRIVER when its ssh session returns (no clock skew between
# machines; the ssh round trip, about a millisecond, is included).
#   1. build in release, copy the binary to every peer, measure the round trip (ping) to each peer
#   2. S1 correctness and throughput on the real network, NX_REPS reps: every node's RESULT must equal `netx expect`, bounds checked;
#      the in-process baseline (`netx inproc`) on the driver for comparison
#   3. S3 credit sweep on the real network
#   4. S4 cancel: node 0 cancels mid-query: every node stops
#   5. S5 kill -9 of the last node's process (on its peer VM): every survivor fails fast, never a wrong answer, never a hang
#   6. S6 partition: the last peer drops the exchange's TCP port (iptables; ssh stays up): only the liveness timer can notice
# Exit status: 0 all checks passed, 1 a check failed, 4 setup failed.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?usage: real-check.sh <results-dir>}
mkdir -p "$OUT/runs"; OUT=$(cd "$OUT" && pwd)
PEERS_FILE=${NX_PEERS_FILE:-/mnt/work/PEERS}; KEY=${NX_PEER_KEY:-/mnt/work/peer_key}
REPS=${NX_REPS:-3}; ROWS=${NX_ROWS:-20000000}; CREDIT=${NX_CREDIT:-$((4 << 20))}; DEAD_MS=${NX_DEAD_MS:-3000}; PORT=${NX_PORT:-7000}
SUMMARY=$OUT/summary.txt; : > "$SUMMARY"; FAILED=0
log() { echo "$*" | tee -a "$SUMMARY"; }
say() { echo "[$(date -u +%H:%M:%S)] $*" >&2; }
check() { if [ "$2" = 0 ]; then log "CHECK $1 PASS $3"; else log "CHECK $1 FAIL $3"; FAILED=$((FAILED + 1)); fi; }
now_ns() { date +%s%N; }
mapfile -t IPS < <(grep -E '^[0-9.]+$' "$PEERS_FILE" 2>/dev/null)
N=${#IPS[@]}
[ "$N" -ge 2 ] || { log "SETUP FAIL: $PEERS_FILE lists $N address(es); a multi-VM run needs at least 2"; exit 4; }
[ -r "$KEY" ] || { log "SETUP FAIL: no peer key at $KEY"; exit 4; }
SSHO=(-i "$KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=10)
rsh() { local ip=$1; shift; ssh "${SSHO[@]}" "nata@$ip" "$@"; }
log "stage B: $N nodes on separate VMs: ${IPS[*]} (node 0 is this driver); rows per node $ROWS, credit $CREDIT, $REPS reps"

# ---------------------------------------------------------------- setup
say "build"
( cd "$HERE" && RUSTFLAGS= cargo build --release --locked --bin netx ) > "$OUT/build.txt" 2>&1 || { log "SETUP FAIL: build (see build.txt)"; exit 4; }
NETX=$HERE/target/release/netx
for i in $(seq 1 $((N - 1))); do
  ip=${IPS[$i]}; ok=1
  for _ in $(seq 1 60); do rsh "$ip" 'test -f /var/run/nata-peer-ready' 2> /dev/null && { ok=0; break; }; sleep 5; done
  [ $ok = 0 ] || { log "SETUP FAIL: peer $i ($ip) never became ready"; exit 4; }
  scp "${SSHO[@]}" "$NETX" "nata@$ip:netx" > /dev/null || { log "SETUP FAIL: could not copy netx to peer $i"; exit 4; }
  rtt=$(ping -c 5 -q "$ip" 2> /dev/null | sed -n 's|.*= [0-9.]*/\([0-9.]*\)/.*|\1|p')
  log "peer $i ($ip): ready, binary copied, ping RTT avg ${rtt:-?} ms, machine $(rsh "$ip" 'nproc; grep -m1 "model name\|Model name" /proc/cpuinfo 2>/dev/null || lscpu | grep -m1 "Model name"' | tr '\n' ' ')"
done
addrs() { local i; for i in $(seq 0 $((N - 1))); do printf '%s:%s,' "${IPS[$i]}" "$PORT"; done | sed 's/,$//'; }
declare -a PIDS
launch() { # launch TAG args...: node 0 here, node i on peer i; files runs/TAG/node<i>.{out,exit} (exit: code and the driver's clock)
  local tag=$1; shift; local i
  RUN="$OUT/runs/$tag"; rm -rf "$RUN"; mkdir -p "$RUN"; PIDS=()
  ( "$NETX" node --me 0 --addrs "$(addrs)" "$@" > "$RUN/node0.out" 2> "$RUN/node0.err"; echo "$? $(now_ns)" > "$RUN/node0.exit" ) & PIDS+=($!)
  for i in $(seq 1 $((N - 1))); do
    ( rsh "${IPS[$i]}" "./netx node --me $i --addrs $(addrs) $*" > "$RUN/node$i.out" 2> "$RUN/node$i.err"; echo "$? $(now_ns)" > "$RUN/node$i.exit" ) & PIDS+=($!)
  done
}
wait_nodes() { local p; for p in "${PIDS[@]}"; do wait "$p"; done; }
kv() { sed -n "s/.*[ ]$2=\\([^ ]*\\).*/\\1/p" "$1" | head -1; }
exit_code() { cut -d' ' -f1 "$RUN/node$1.exit" 2> /dev/null || echo 255; }
exit_ns() { cut -d' ' -f2 "$RUN/node$1.exit" 2> /dev/null || echo 0; }
expect_for() { local f="$OUT/expect-$1.txt"; [ -s "$f" ] || "$NETX" expect --nodes "$N" --rows "$1" > "$f"; echo "$f"; }
verify_run() { # ROWS CREDIT
  local rows=$1 credit=$2 ef i; ef=$(expect_for "$rows")
  for i in $(seq 0 $((N - 1))); do
    [ "$(exit_code "$i")" = 0 ] || { echo "node $i exited with $(exit_code "$i"): $(head -c 200 "$RUN/node$i.out")"; return 1; }
    local wr wc gr gc pb pf
    wr=$(grep "EXPECT node=$i " "$ef" | sed -n 's/.*rows_in=\([0-9]*\).*/\1/p'); wc=$(grep "EXPECT node=$i " "$ef" | sed -n 's/.*checksum=\([0-9]*\).*/\1/p')
    gr=$(kv "$RUN/node$i.out" rows_in); gc=$(kv "$RUN/node$i.out" checksum)
    [ "$gr" = "$wr" ] && [ "$gc" = "$wc" ] || { echo "node $i: rows $gr checksum $gc, expected $wr $wc"; return 1; }
    pb=$(kv "$RUN/node$i.out" peak_buffered); pf=$(kv "$RUN/node$i.out" peak_inflight)
    [ "${pb:-0}" -le "$credit" ] && [ "${pf:-0}" -le "$credit" ] || { echo "node $i: peak buffered $pb / in flight $pf above $credit"; return 1; }
  done
}
run_secs() { local m=0 s i; for i in $(seq 0 $((N - 1))); do s=$(kv "$RUN/node$i.out" secs); m=$(awk -v a="$m" -v b="${s:-0}" 'BEGIN{print (b>a)?b:a}'); done; echo "$m"; }
bytes_moved() { local t=0 b i; for i in $(seq 0 $((N - 1))); do b=$(kv "$RUN/node$i.out" bytes_sent); t=$((t + ${b:-0})); done; echo "$t"; }
SETTLE_SH="$HERE/../../scripts/gcp/settle.sh"
settle() { [ -x "$SETTLE_SH" ] && "$SETTLE_SH" "$1" 0.5 "${NX_SETTLE_MAX:-300}" | tee -a "$SUMMARY"; return 0; }
leftover() { local n=0 i; n=$(pgrep -fc "netx node" || true); for i in $(seq 1 $((N - 1))); do n=$((n + $(rsh "${IPS[$i]}" 'pgrep -fc "netx node" || true'))); done; echo "$n"; }

# ---------------------------------------------------------------- S1
say "S1 correctness and throughput"
exact=0; mbs=""
for rep in $(seq 1 "$REPS"); do
  settle "s1-rep$rep"
  launch "s1-$rep" --rows "$ROWS" --credit-bytes "$CREDIT" --dead-ms 20000 --connect-timeout-s 60; wait_nodes
  why=$(verify_run "$ROWS" "$CREDIT") || { exact=1; log "S1 rep $rep: $why"; }
  s=$(run_secs); b=$(bytes_moved); m=$(awk -v b="$b" -v s="$s" 'BEGIN{ if (s>0) printf "%.0f", b/s/1e6; else print 0 }')
  mbs="$mbs $m"; log "S1 rep $rep: slowest node ${s} s, ${b} bytes sent in total, ${m} MB/s across the cluster"
done
check s1-real-exact $exact "(every node's rows and checksum equal the expected, buffered/in-flight <= credit, $REPS reps)"
inproc=$("$NETX" inproc --nodes "$N" --rows "$ROWS" 2>&1 | tail -1); log "in-process baseline on the driver (same shuffle, no sockets): $inproc"
# ---------------------------------------------------------------- S3
say "S3 credit sweep"
SWR=${NX_SWEEP_ROWS:-5000000}
for c in 65536 262144 1048576 4194304; do
  settle "s3-$c"
  launch "s3-$c" --rows "$SWR" --batch-rows $((N * 1024)) --credit-bytes "$c" --dead-ms 20000 --connect-timeout-s 60; wait_nodes
  why=$(verify_run "$SWR" "$c") && ok=0 || { ok=1; log "S3 credit $c: $why"; }
  s=$(run_secs); b=$(bytes_moved); log "S3 credit $c: ${s} s, $(awk -v b="$b" -v s="$s" 'BEGIN{ if (s>0) printf "%.0f", b/s/1e6 }') MB/s"
  check "s3-credit-$c-exact" $ok ""
done
BIG=400000000
# ---------------------------------------------------------------- S4
say "S4 cancel"
t0=$(now_ns); launch s4 --rows "$BIG" --batch-rows $((N * 1024)) --credit-bytes $((1 << 20)) --cancel-at 0:300 --dead-ms 20000 --connect-timeout-s 60; wait_nodes
bad=0
for i in $(seq 0 $((N - 1))); do
  rc=$(exit_code "$i"); el=$(awk -v a="$t0" -v b="$(exit_ns "$i")" 'BEGIN{printf "%.2f", (b-a)/1e9}'); log "S4 node $i: exit $rc after $el s: $(head -c 140 "$RUN/node$i.out")"
  if [ "$i" = 0 ]; then [ "$rc" = 3 ] || bad=1; else [ "$rc" = 3 ] || [ "$rc" = 4 ] || bad=1; fi
  awk -v e="$el" 'BEGIN{exit !(e < 15)}' || bad=1
done
check s4-cancel-stops-every-node $bad "(node 0 exits 3, the others 3 or 4, all within 15 s)"
l=$(leftover); check s4-no-process-left "$([ "$l" = 0 ] && echo 0 || echo 1)" "($l left)"
# ---------------------------------------------------------------- S5
say "S5 kill -9 of node $((N - 1))"
launch s5 --rows "$BIG" --batch-rows $((N * 1024)) --credit-bytes $((1 << 20)) --dead-ms 20000 --connect-timeout-s 60
sleep 3; tk=$(now_ns); rsh "${IPS[$((N - 1))]}" 'pkill -9 -f "netx node"'; wait_nodes
bad=0
for i in $(seq 0 $((N - 1))); do
  rc=$(exit_code "$i"); el=$(awk -v a="$tk" -v b="$(exit_ns "$i")" 'BEGIN{printf "%.2f", (b-a)/1e9}'); log "S5 node $i: exit $rc after $el s: $(head -c 140 "$RUN/node$i.out")"
  [ "$i" = $((N - 1)) ] && continue
  [ "$rc" = 4 ] || [ "$rc" = 3 ] || bad=1
  awk -v e="$el" 'BEGIN{exit !(e < 5)}' || bad=1
done
check s5-kill-survivors-fail-fast $bad "(every survivor exits 4 or 3 within 5 s; none exits 0)"
# ---------------------------------------------------------------- S6
say "S6 partition of node $((N - 1)) (liveness timeout $DEAD_MS ms)"
P=${IPS[$((N - 1))]}
launch s6 --rows "$BIG" --batch-rows $((N * 1024)) --credit-bytes $((1 << 20)) --dead-ms "$DEAD_MS" --ping-ms 250 --connect-timeout-s 60
sleep 3
tp=$(now_ns)
rsh "$P" "sudo iptables -I INPUT -p tcp --dport $PORT -j DROP; sudo iptables -I INPUT -p tcp --sport $PORT -j DROP; sudo iptables -I OUTPUT -p tcp --dport $PORT -j DROP; sudo iptables -I OUTPUT -p tcp --sport $PORT -j DROP"
wait_nodes
rsh "$P" 'sudo iptables -F INPUT; sudo iptables -F OUTPUT'
bad=0; maxel=0
for i in $(seq 0 $((N - 1))); do
  rc=$(exit_code "$i"); el=$(awk -v a="$tp" -v b="$(exit_ns "$i")" 'BEGIN{printf "%.2f", (b-a)/1e9}'); log "S6 node $i: exit $rc after $el s: $(head -c 140 "$RUN/node$i.out")"
  [ "$rc" = 4 ] || [ "$rc" = 3 ] || bad=1
  maxel=$(awk -v a="$maxel" -v b="$el" 'BEGIN{print (b>a)?b:a}')
  awk -v e="$el" -v d="$DEAD_MS" 'BEGIN{exit !(e >= d/1000 - 0.5 && e < d/1000 + 5)}' || bad=1
done
check s6-partition-detected-by-liveness-timer $bad "(every node exits 4 or 3 between $DEAD_MS ms - 0.5 s and + 5 s; slowest $maxel s)"
l=$(leftover); check no-process-left "$([ "$l" = 0 ] && echo 0 || echo 1)" "($l left)"
# ---------------------------------------------------------------- verdict
if [ "$FAILED" -eq 0 ]; then log "VERDICT PASS (every check passed; REAL network between $N VMs in one zone)"; exit 0; fi
log "VERDICT FAIL ($FAILED check(s) failed; see the CHECK lines)"; exit 1
