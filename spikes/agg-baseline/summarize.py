#!/usr/bin/env python3
"""Summarize the local indicative run, without external packages."""
import collections
import re
import statistics
import sys

rows = collections.defaultdict(list)
spill_rounds = collections.defaultdict(list)
for line in open(sys.argv[1]):
    fields = line.strip().split(',')
    if fields[0] == 'spill':
        arm = fields[2]
        for field in fields[3:]:
            name, value = field.split('=')
            rows[f'{arm}.{name}'].append(float(value))
        if arm == 'Ipc':
            spill_rounds[fields[1]].append(dict(item.split('=') for item in fields[3:]))
    elif fields[0] == 'table':
        name = ['std', 'hashbrown_fresh', 'hashbrown_retained', 'std_twin'][int(fields[2])]
        rows[name + '.ms'].append(float(fields[3].split('=')[1]))
    elif fields[0] == 'schedule':
        arm = '.'.join(fields[2:5])
        rows[arm + '.ms'].append(float(fields[5].split('=')[1]))
        for counter, value in re.findall(r'(claims|local|steals|retries|probes|pins): (\d+)', line):
            rows[arm + '.' + counter].append(int(value))
print('metric n min median max')
for name, values in sorted(rows.items()):
    print(name, len(values), f'{min(values):.4f}', f'{statistics.median(values):.4f}', f'{max(values):.4f}')
for metric in ['write_ms', 'read_ms']:
    ratios = [float(v[1][metric]) / float(v[0][metric]) for v in spill_rounds.values()]
    print('IPC_twin_' + metric + '_last/first', 'min/median/max', *[f'{n:.4f}' for n in [min(ratios), statistics.median(ratios), max(ratios)]])
for twin, control in [('std_twin.ms', 'std.ms'), ('Fifo.affinity=false.twin=true.ms', 'Fifo.affinity=false.twin=false.ms')]:
    ratios = [a/b for a,b in zip(rows[twin], rows[control])]
    print(twin + '/control', 'min/median/max', *[f'{n:.4f}' for n in [min(ratios), statistics.median(ratios), max(ratios)]])
