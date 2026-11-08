#!/usr/bin/env python3
"""Summarize the same timed invocations, without converting samples into CPU time."""
import json
from pathlib import Path
import statistics
import sys

root = Path(sys.argv[1])
samples = json.loads((root/'native-samples.json').read_text())
summary = json.loads((root/'native-summary.json').read_text())
records=[]
markdown=[]
for cell in summary['cells']:
    query, threads, budget = (cell[k] for k in ('query','threads','budget'))
    arms={label:[s for s in samples if (s['query'],s['threads'],s['budget'],s['arm']) == (query,threads,budget,label)] for label in ('before','after','control')}
    assert all(len(v)==9 for v in arms.values())
    for arm in arms.values():
        assert all(s['exit']==0 and s['stats']['version']==1 for s in arm)
        assert all(Path(s['stats_file']).is_file() for s in arm)
    stages={label:{stage:statistics.median(s['stats']['stages'][stage]['service_wall_ns']/1e6 for s in rows) for stage in rows[0]['stats']['stages']} for label,rows in arms.items()}
    counters={label:{name:statistics.median(s['stats']['counter_values'][i] for s in rows) for i,name in enumerate(rows[0]['stats']['counter_names'])} for label,rows in arms.items()}
    records.append(dict(query=query,threads=threads,budget=budget,stage_service_wall_ms=stages,counters=counters,stats_complete=all(s['stats']['complete'] for rows in arms.values() for s in rows),missing=arms['before'][0]['stats']['missing']))
    markdown += [f'### {query}, {threads} threads, {budget//(1<<20)} MiB', '', '| Stage (service wall ms) | Before | After |', '|---|---:|---:|']
    for stage,before in stages['before'].items():
        markdown.append(f"| {stage} | {before:.3f} | {stages['after'][stage]:.3f} |")
    markdown += ['', '| Counter | Before | After |', '|---|---:|---:|']
    for counter,before in counters['before'].items():
        markdown.append(f"| {counter} | {before:,.0f} | {counters['after'][counter]:,.0f} |")
    markdown.append('')
(root/'stats-summary.json').write_text(json.dumps(records,indent=2)+'\n')
(root/'stage-tables.md').write_text('\n'.join(markdown))
