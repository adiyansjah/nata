#!/usr/bin/env python3
"""Small source-backed SVG charts; no external scripts or plotting dependency."""
import html
import json
import math
from pathlib import Path
import sys

root=Path(sys.argv[1])
summary=json.loads((root/'native-summary.json').read_text())
stats=json.loads((root/'stats-summary.json').read_text())
lo=math.floor(min(c['control_ratio']['min'] for c in summary['cells'])*10)/10
hi=math.ceil(max(c['control_ratio']['max'] for c in summary['cells'])*10)/10
lo=min(lo,.8); hi=max(hi,1.2)
def x(v): return 190+(v-lo)/(hi-lo)*420
svg=['<svg height="290" viewBox="0 0 700 290" role="img" aria-label="Unsettled paired ratios; candidate IQR and identical-baseline control range">']
for i in range(round((hi-lo)*10)+1):
 v=round(lo+i/10,1);px=x(v)
 svg.append(f'<line class="grid" x1="{px}" x2="{px}" y1="15" y2="245"/><text x="{px}" y="268" text-anchor="middle">{v:.1f}×</text>')
svg.append(f'<line class="reference" x1="{x(1)}" x2="{x(1)}" y1="15" y2="245"/>')
for i,c in enumerate(summary['cells']):
 y=42+i*57;r=c['ratio'];control=c['control_ratio'];name=('Q1' if c['query']=='q1' else 'High cardinality')+f" / {c['threads']}t"
 svg.append(f'<g><title>{html.escape(name)}: candidate {r["median"]:.6f}, IQR {r["q1"]:.6f}–{r["q3"]:.6f}; control {control["min"]:.6f}–{control["max"]:.6f}</title>')
 svg.append(f'<text x="0" y="{y+5}">{name}</text><line class="control" x1="{x(control["min"])}" x2="{x(control["max"])}" y1="{y+10}" y2="{y+10}"/>')
 svg.append(f'<line class="candidate" x1="{x(r["q1"])}" x2="{x(r["q3"])}" y1="{y}" y2="{y}"/><circle class="point" cx="{x(r["median"])}" cy="{y}" r="5"/><text x="625" y="{y+5}">{r["median"]:.3f}×</text></g>')
svg.append('</svg>')
q1=next(s for s in stats if s['query']=='q1' and s['threads']==1)
stages=['scan_decode','filter','expression','group_id','state_update']
maximum=max(q1['stage_service_wall_ms'][a][s] for a in ('before','after') for s in stages)
bars=['<svg height="340" viewBox="0 0 700 340" role="img" aria-label="Q1 one-thread stage service wall milliseconds, same timed runs">']
for i,s in enumerate(stages):
 y=26+i*61;bars.append(f'<text x="0" y="{y+14}">{s.replace("_"," ")}</text>')
 for arm,dy,klass in [('before',0,'before'),('after',22,'after')]:
  value=q1['stage_service_wall_ms'][arm][s];w=value/maximum*380
  bars.append(f'<rect class="{klass}" x="180" y="{y+dy}" width="{w}" height="15"><title>{arm}: {value:.3f} ms</title></rect><text x="{190+w}" y="{y+dy+13}">{value:.1f} ms</text>')
bars.append('</svg>')
doc='''<!doctype html><html><head><meta charset="utf-8"><style>
body{font-family:var(--font-sans,system-ui);color:var(--foreground);margin:0}section{margin:16px 0}h3{font-size:15px;margin:0 0 8px}p{font-size:13px;line-height:1.5;color:var(--muted-foreground)}svg{width:100%;display:block;font-size:12px;overflow:visible}svg text{fill:currentColor}.grid{stroke:var(--border);stroke-width:1}.reference{stroke:currentColor;stroke-dasharray:4 4;opacity:.55}.control{stroke:var(--muted-foreground);stroke-width:3;stroke-dasharray:3 2}.candidate{stroke:var(--chart-1);stroke-width:6}.point{fill:var(--chart-1)}.before{fill:var(--muted-foreground);opacity:.65}.after{fill:var(--chart-1)}.legend{font-size:12px}.key{display:inline-block;width:18px;height:8px;margin-right:5px}.key.before{background:var(--muted-foreground)}.key.after{background:var(--chart-1)}
</style></head><body><section><h3>Unsettled local screen · nine paired rounds</h3><p>Candidate / baseline elapsed time. Dot: paired median; thick line: candidate IQR. Dashed line: identical-baseline control min–max. These are not confidence intervals. GCP verdict pending.</p>'''+''.join(svg)+'''</section><section><h3>Q1 / 1 thread · same-run stage timers</h3><p class="legend"><span class="key before"></span>Before &nbsp; <span class="key after"></span>After</p>'''+''.join(bars)+'''<p>Service wall medians in milliseconds; these are not a disjoint sum of CLI wall time. Package 0's incomplete-attribution flags remain unchanged. Full stages and counters are in stage-tables.md and stats-summary.json.</p></section></body></html>'''
(root/'comparison.html').write_text(doc)
