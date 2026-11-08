"""Shared process boundary and versioned verdict rules. No cloud operations.

The legacy slice-5 policy deliberately has no automatic MET verdict. Card 0008
is a separate, explicit policy; its proposed numbers still need a reference freeze.
"""
import json
import math
import os
from pathlib import Path
import signal
import statistics
import subprocess
import time

POLICY = '0008-v1'


def distribution(values):
    q1, _, q3 = statistics.quantiles(values, n=4, method='inclusive')
    return dict(n=len(values), min=min(values), q1=q1,
                median=statistics.median(values), q3=q3, max=max(values))


def parse_stats(stderr):
    records = [json.loads(line[len('query_stats: '):]) for line in stderr.splitlines()
               if line.startswith('query_stats: ')]
    if len(records) > 1:
        raise ValueError('multiple same-run stats records')
    return records[0] if records else None


def invoke(argv, env, output, log, timeout):
    """b383422 pipe-EOF boundary, including startup, full output and teardown."""
    rss = log.with_suffix('.usage')
    load = os.getloadavg()
    start = time.perf_counter_ns()
    with output.open('w') as sink:
        p = subprocess.Popen(['/usr/bin/time', '-f', '%M %U %S', '-o', str(rss),
                              *map(str, argv)], env=env, stdout=sink,
                             stderr=subprocess.PIPE, start_new_session=True)
        try:
            _, stderr = p.communicate(timeout=timeout)
            rc = p.returncode
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            _, stderr = p.communicate()
            rc = 124
    elapsed_ns = time.perf_counter_ns() - start
    log.write_bytes(stderr)
    text = stderr.decode(errors='replace')
    counters = {}
    for line in text.splitlines():
        if line.startswith('aggregate:'):
            counters = {k: int(v) for k, v in (s.split('=') for s in line.split()[1:])}
    usage = rss.read_text().splitlines()[-1].split() if rss.exists() and rss.stat().st_size else []
    return dict(exit=rc, seconds=elapsed_ns / 1e9, elapsed_ns=elapsed_ns, load=load,
                rss_bytes=int(usage[0])*1024 if len(usage) == 3 else None,
                user_seconds=float(usage[1]) if len(usage) == 3 else None,
                system_seconds=float(usage[2]) if len(usage) == 3 else None,
                output_bytes=output.stat().st_size, counters=counters,
                stats=parse_stats(text))


def paired(samples, labels, rounds=5):
    """Index by round, never pair by arrival order or silently truncate zip."""
    result = {}
    for label in labels:
        for role in ('primary', 'twin'):
            rows = [s for s in samples if (s['variant'], s['role']) == (label, role)]
            if len(rows) != rounds or {s.get('round') for s in rows} != set(range(rounds)):
                continue
            if any(s['exit'] or not math.isfinite(s['seconds']) or s['seconds'] <= 0 for s in rows):
                continue
            result[label, role] = [s['seconds'] for s in sorted(rows, key=lambda s: s['round'])]
    return result


def assess(samples, candidate, reference, limit, *, settled, floor=False,
           complete_reference=True, require_stats=True, tolerance=.05):
    """Card 0008: median AND four of five; largest median primary/twin noise.

    Missing cells are NOT RUN; failed reference has no denominator (UNASSESSED).
    Correctness/resource failures must be gated before calling this function.
    All timed Nata records must be complete, including zero dropped events.
    """
    result = dict(policy=POLICY, verdict='NOT RUN', limit=limit, floor=floor)
    if not complete_reference or any(s['exit'] for s in samples if s['variant'] == reference):
        return dict(result, verdict='UNASSESSED')
    arms = paired(samples, (candidate, reference))
    if len(arms) != 4:
        return result
    if any(s.get('error') or s.get('correct') is False or s.get('resources_ok') is False
           for s in samples):
        return dict(result, verdict='NOT MET', reason='correctness/resource failure')
    if require_stats:
        for s in samples:
            if s['variant'] == 'duckdb':
                continue
            record = s.get('stats')
            if not record or record.get('version') != 1 or not record.get('complete') or record.get('dropped_events') != 0:
                return dict(result, reason='missing/incomplete stats')
    ratios = [c/r for c, r in zip(arms[candidate, 'primary'], arms[reference, 'primary'])]
    noises = {label: statistics.median(abs(p/t-1) for p, t in zip(arms[label, 'primary'], arms[label, 'twin']))
              for label in (candidate, reference)}
    result.update(ratios=ratios, distribution=distribution(ratios), twin_noise=noises)
    if not settled or max(noises.values()) > tolerance:
        return dict(result, verdict='UNRESOLVED')
    within = [r >= limit if floor else r <= limit for r in ratios]
    median_ok = statistics.median(ratios) >= limit if floor else statistics.median(ratios) <= limit
    verdict = 'MET' if median_ok and sum(within) >= 4 else (
        'NOT MET' if not median_ok and within.count(False) >= 4 else 'UNRESOLVED')
    return dict(result, verdict=verdict)


def legacy_summary(samples, complete_reference, settled):
    """Frozen slice 5 policy, unchanged: noise judged by parent, never auto MET."""
    result = dict(verdict='UNRESOLVED' if complete_reference else 'UNASSESSED', arms={})
    for v in ('main', 'duckdb'):
        for role in ('primary', 'twin'):
            rows = [s for s in samples if (s['variant'], s['role']) == (v, role)]
            if len(rows) == 5 and all(s['exit'] == 0 for s in rows):
                result['arms'][v+'-'+role] = distribution([s['seconds'] for s in rows])
    result['twin_ratios'] = {}
    for v in ('main', 'duckdb'):
        pairs = [[s for s in samples if (s['variant'], s['role']) == (v, role)] for role in ('primary', 'twin')]
        if all(len(rows) == 5 and all(s['exit'] == 0 for s in rows) for rows in pairs):
            result['twin_ratios'][v] = distribution([t['seconds']/p['seconds'] for p, t in zip(*pairs)])
    if settled and all(k in result['arms'] for k in ('main-primary', 'duckdb-primary')):
        result['paired_latency_ratio'] = distribution([n['seconds']/d['seconds'] for n, d in zip(
            [s for s in samples if (s['variant'], s['role']) == ('main', 'primary')],
            [s for s in samples if (s['variant'], s['role']) == ('duckdb', 'primary')])])
    return result


def evaluate_manifest(manifest, observations):
    """Explicit dimensions prevent SF/thread/budget/affinity mixing."""
    if manifest['policy'] != POLICY:
        raise ValueError('unknown or unfrozen policy identity')
    results = []
    for cell in manifest['cells']:
        samples = [s for s in observations if all(s.get(k) == v for k, v in cell['where'].items())]
        result = assess(samples, cell['candidate'], cell['reference'], cell['limit'],
                        floor=cell.get('floor', False),
                        settled=bool(samples) and all(s.get('settled', False) for s in samples),
                        require_stats=cell.get('require_stats', True))
        results.append(dict(cell=cell['id'], **result, samples=samples))
    return results


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--timings', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = evaluate_manifest(json.loads(args.manifest.read_text()), json.loads(args.timings.read_text()))
    args.out.write_text(json.dumps(result, indent=2)+'\n')
    raise SystemExit(0 if all(c['verdict'] == 'MET' for c in result) else 1)
