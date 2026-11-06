#!/usr/bin/env bash
# spill_summary.py must FAIL on a failed run and on row counts that differ between arms, and pass on a clean table.
set -u; H=$(cd "$(dirname "$0")" && pwd); T=$(mktemp -d); trap 'rm -rf "$T"' EXIT; fail=0
hdr="query,threads,round,arm,secs,engine_secs,peak_rss_mb,rows,spilled_mb,spill_files,max_open_files,leftover,status"
printf '%s\n%s\n%s\n' "$hdr" "q,1,1,mem,1.0,1,10,5,0,0,0,0,ok" "q,1,1,g64,1.2,1,8,5,40,64,32,0,ok" > "$T/ok.csv"
python3 -I "$H/spill_summary.py" "$T/ok.csv" > /dev/null && echo "ok   clean table passes" || { echo "FAIL clean table"; fail=1; }
printf '%s\n%s\n%s\n' "$hdr" "q,1,1,mem,1.0,1,10,5,0,0,0,0,ok" "q,1,1,g64,1.2,1,8,6,40,64,32,0,ok" > "$T/rows.csv"
python3 -I "$H/spill_summary.py" "$T/rows.csv" > /dev/null && { echo "FAIL differing row counts passed"; fail=1; } || echo "ok   differing row counts fail"
printf '%s\n%s\n%s\n' "$hdr" "q,1,1,mem,1.0,1,10,5,0,0,0,0,ok" "q,1,1,g64,,,8,,,,,,FAILED" > "$T/st.csv"
python3 -I "$H/spill_summary.py" "$T/st.csv" > /dev/null && { echo "FAIL a failed run passed"; fail=1; } || echo "ok   a failed run fails"
exit $fail
