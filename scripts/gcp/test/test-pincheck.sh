#!/bin/bash
# Tests of the two pure parts of spikes/push-cost/pin-check.sh (the experiment on hardware threads): the CPU sets computed from the machine's topology, and the
# table parser. Both are checked against fixtures with KNOWN answers, because an error in either would silently invalidate the experiment (the first version of the
# parser read the wrong columns and printed "static 0.1" without complaint).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
PC=${PINCHECK:-$REPO/spikes/push-cost/pin-check.sh}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }

sets() { "$PC" --sets "$1" | tr '\n' ' ' | sed 's/ $//'; }
# `lscpu -p=CPU,CORE` fixtures
printf '# CPU,Core\n0,0\n1,1\n2,2\n3,3\n4,0\n5,1\n6,2\n7,3\n' > $T/ht-plus4.txt     # threads i and i+4 share a core
printf '# CPU,Core\n0,0\n1,0\n2,1\n3,1\n4,2\n5,2\n6,3\n7,3\n' > $T/ht-adjacent.txt  # threads 2k and 2k+1 share a core
printf '# CPU,Core\n0,0\n1,1\n2,2\n3,3\n4,4\n5,5\n6,6\n7,7\n' > $T/no-ht.txt         # one thread per core
printf '# CPU,Core\n0,0\n1,1\n' > $T/two.txt
printf '# CPU,Core\n0,5\n1,5\n2,9\n3,9\n4,7\n5,7\n6,2\n7,2\n' > $T/odd-ids.txt       # core ids that are not 0..n-1
check "threads i and i+4 share a core: distinct = 4 different cores, siblings = both threads of the first two cores" "[ \"\$(sets $T/ht-plus4.txt)\" = 'threads_per_core=2 distinct=0,1,2,3 siblings=0,4,1,5' ]"
check "threads 2k and 2k+1 share a core" "[ \"\$(sets $T/ht-adjacent.txt)\" = 'threads_per_core=2 distinct=0,2,4,6 siblings=0,1,2,3' ]"
check "one thread per core: siblings are 'none' (the experiment must not pretend)" "[ \"\$(sets $T/no-ht.txt)\" = 'threads_per_core=1 distinct=0,1,2,3 siblings=none' ]"
check "fewer than four cores: distinct has what exists" "[ \"\$(sets $T/two.txt)\" = 'threads_per_core=1 distinct=0,1 siblings=none' ]"
check "core ids need not be 0..n-1; cores are taken in order of appearance" "[ \"\$(sets $T/odd-ids.txt)\" = 'threads_per_core=2 distinct=0,2,4,6 siblings=0,1,2,3' ]"
# every set from the 8-thread fixtures: the distinct cpus are on different cores, the sibling cpus cover exactly two cores
core_of() { awk -F, -v c="$2" '!/^#/ && $1 == c { print $2 }' "$1"; }
d=$("$PC" --sets $T/ht-plus4.txt | sed -n 's/^distinct=//p'); cores=$(for c in ${d//,/ }; do core_of $T/ht-plus4.txt $c; done | sort -u | wc -l)
check "distinct cpus really are on 4 different cores" "[ $cores -eq 4 ]"
s=$("$PC" --sets $T/ht-plus4.txt | sed -n 's/^siblings=//p'); cores=$(for c in ${s//,/ }; do core_of $T/ht-plus4.txt $c; done | sort -u | wc -l)
check "sibling cpus cover exactly 2 cores" "[ $cores -eq 2 ]"

# the table parser: the summary rows of bench/results/20261105-timing-x86.md (c3d), copied as they are printed
cat > $T/x86.md <<'MD'
| cell | control ns/row | static | dyn-direct | dyn | static-in-sched | twin | quiet |
|---|---|---|---|---|---|---|---|
| cheap L1 B1024 T1 | 0.334 | +32.6 | +31.4 | +51.4 | +49.8 | -2.1 ? | yes |
| cheap L1 B1024 T4 | 0.092 | +29.2 | +31.1 | +94.2 | +95.2 | -0.5 ? | yes |
| cheap L1 B8192 T4 | 0.086 | +27.0 | +27.9 | +74.0 | +72.6 | +0.4 ? | yes |
| cheap L1 B65536 T4 | 0.098 | +45.7 | +45.7 | +93.7 | +93.6 | +0.0 ? | yes |
| cheap L3 B1024 T4 | 0.102 | +113.4 | +116.9 | +229.5 | +209.4 | -0.6 ? | yes |
| medium L1 B8192 T4 | 0.250 | +56.2 | +55.7 | +107.0 | +108.3 | -0.3 ? | no |
MD
row=$("$PC" --gap-row distinct $T/x86.md)
check "gap = dyn minus static: 65.0, 47.0, 48.0 percentage points for the three batch sizes" "[ \"\$row\" = '| distinct | 65.0 | 47.0 | 48.0 | 27.0 | 74.0 | yes/yes/yes |' ]"
check "rows of other cells (T1, three stages, the medium kernel) are ignored" "! echo '$row' | grep -q 'no'"
printf '| cheap L1 B1024 T4 | 0.1 | -3.0 ? | +1.0 | +10.0 | +9.0 | +0.0 ? | yes |\n| cheap L1 B8192 T4 | 0.1 | +5.0 | +5.0 | +2.0 | +2.0 | +0.0 ? | yes |\n| cheap L1 B65536 T4 | 0.1 | +0.0 ? | +0.0 | +0.0 | +0.0 | +0.0 ? | yes |\n' > $T/neg.md
check "negative numbers and the ? marker are parsed (static -3.0 ?, dyn +10.0 gives 13.0; dyn below static gives -3.0)" "[ \"\$($PC --gap-row x $T/neg.md)\" = '| x | 13.0 | -3.0 | 0.0 | 5.0 | 2.0 | yes/yes/yes |' ]"

[ "$fails" = 0 ] && echo "all pincheck tests passed" || { echo "PINCHECK TESTS FAILED ($fails)"; exit 1; }
