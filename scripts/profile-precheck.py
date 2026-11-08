#!/usr/bin/env python3
"""Fail closed before paid matrices. Compile a symbol-known native smoke workload.

Try software clocks AND hardware cycles on ARM; retain each attempted command,
exit and stderr. A counter-only perf stat success never substitutes for stacks.
No cloud calls; raw captures stay in the supplied private results directory.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys

EVENTS = ('cpu-clock:u', 'task-clock:u', 'cycles:u')
SOURCE = '''#include <time.h>
__attribute__((noinline)) unsigned long nata_profile_leaf(unsigned long x) {
  for (unsigned long i=0; i<1000000; ++i) x = x*1664525UL+1013904223UL;
  __asm__ volatile("" : "+r"(x)); return x;
}
__attribute__((noinline)) unsigned long nata_profile_parent(unsigned long x) {
  return nata_profile_leaf(x)+1;
}
int main(void) { struct timespec a,b; clock_gettime(CLOCK_MONOTONIC,&a);
  volatile unsigned long x=1; do { x=nata_profile_parent(x);
    clock_gettime(CLOCK_MONOTONIC,&b);
  } while ((b.tv_sec-a.tv_sec)+(b.tv_nsec-a.tv_nsec)/1e9 < 1.0); return x==0; }
'''


def useful(script):
    # Require both known frames in one callchain, not just a resolved leaf IP.
    chains = re.split(r'\n\s*\n', script)
    good = [c for c in chains if 'nata_profile_leaf' in c and 'nata_profile_parent' in c]
    return dict(useful_chains=len(good), unknown_frames=script.count('[unknown]'),
                total_frames=len(re.findall(r'^\s+[0-9a-f]+\s', script, re.M)),
                lost_records=len(re.findall(r'PERF_RECORD_LOST|LOST [0-9]+', script)))


def run(out, all_events=False):
    out.mkdir(parents=True, exist_ok=False)
    record = dict(version=1, architecture=platform.machine(), verdict='NOT RUN', attempts=[], working_events=[])
    def save():
        (out/'precheck.json').write_text(json.dumps(record, indent=2)+'\n')
    save()
    for executable in ('cc', 'perf', '/usr/bin/time'):
        if not shutil.which(executable):
            record.update(verdict='NOT MET', reason='missing '+executable); save(); return 1
    source, binary = out/'smoke.c', out/'smoke'
    source.write_text(SOURCE)
    subprocess.run(['cc', '-O1', '-g', '-fno-omit-frame-pointer', '-fno-optimize-sibling-calls',
                    str(source), '-o', str(binary)], check=True)
    for event in EVENTS:
        capture = out/(event.split(':')[0]+'.data')
        cmd = ['perf', 'record', '-e', event, '-F', '99', '--call-graph', 'dwarf',
               '-o', str(capture), '--', '/usr/bin/time', '-f', '%e %U %S',
               '-o', str(capture)+'.usage', str(binary)]
        attempt = dict(event=event, command=cmd)
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        attempt.update(exit=result.returncode, stderr=result.stderr)
        if not result.returncode:
            script_cmd = ['perf', 'script', '--show-lost-events', '-i', str(capture)]
            script = subprocess.run(script_cmd, capture_output=True, text=True, timeout=30)
            (capture.with_suffix('.stacks')).write_text(script.stdout)
            attempt.update(script_command=script_cmd, script_exit=script.returncode,
                           script_stderr=script.stderr, **useful(script.stdout))
            usage = Path(str(capture)+'.usage').read_text().split()
            attempt['wall_user_system_seconds'] = list(map(float, usage))
            attempt['unwind_report'] = 'known parent+leaf chains; unknown frames counted, raw stacks retained'
            if (not script.returncode and attempt['useful_chains'] >= 5
                    and attempt['lost_records'] == 0 and 'lost' not in result.stderr.lower()
                    and attempt['unknown_frames'] <= .05 * max(1, attempt['total_frames'])
                    and len(usage) == 3
                    and float(usage[0]) >= .9):
                record.update(verdict='MET')
                record.setdefault('selected_event', event)
                record['working_events'].append(event)
        record['attempts'].append(attempt); save()
        if record['verdict'] == 'MET' and not all_events:
            return 0
    if record['verdict'] == 'MET':
        return 0
    record.update(verdict='NOT MET', reason='no event produced useful symbol stacks with wall/CPU and zero reported loss')
    save(); return 1


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--all-events', action='store_true', help='record all ARM event trials, even after success')
    a = p.parse_args()
    sys.exit(run(a.out.resolve(), a.all_events))
