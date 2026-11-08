"""Shared process boundary and versioned verdict rules. No cloud operations.

The legacy slice-5 policy deliberately has no automatic MET verdict. Card 0008
is a separate, explicit policy; its proposed numbers still need a reference freeze.
"""
import json
from fractions import Fraction
import math
import os
from pathlib import Path
import signal
import statistics
import subprocess
import time

POLICY = '0008-v1'
REQUIRED_STAGES = ('scan_decode', 'filter', 'expression', 'group_id', 'state_update',
                   'merge', 'finalize', 'output_sink', 'spill_write', 'spill_read',
                   'teardown', 'admission_wait', 'bind_metadata')
REQUIRED_COUNTERS = ('copied_bytes', 'materialized_bytes', 'col_materializations',
                     'allocation_capacity_bytes', 'allocation_sites', 'grants',
                     'grant_bytes', 'releases')


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


def invoke(argv, env, output, log, timeout, *, capture_usage=True):
    """b383422 pipe-EOF boundary, including startup, full output and teardown."""
    rss = log.with_suffix('.usage')
    load = os.getloadavg()
    start = time.perf_counter_ns()
    with output.open('w') as sink:
        command = ['/usr/bin/time', '-f', '%M %U %S', '-o', str(rss), *map(str, argv)] if capture_usage else list(map(str, argv))
        p = subprocess.Popen(command, env=env, stdout=sink,
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
            if any(s['exit'] or not math.isfinite(s['seconds']) or s['seconds'] <= 0 or ('elapsed_ns' in s and (not isinstance(s['elapsed_ns'], int) or s['elapsed_ns'] <= 0)) for s in rows):
                continue
            result[label, role] = [Fraction(s['elapsed_ns'], 1_000_000_000) if 'elapsed_ns' in s else Fraction(str(s['seconds'])) for s in sorted(rows, key=lambda s: s['round'])]
    return result


def twin_noise(arms, labels):
    return {label: statistics.median(abs(p/t-1) for p, t in zip(arms[label, 'primary'], arms[label, 'twin']))
            for label in labels}


def reconcile_stats(record):
    """Structural same-run controls, separate from completeness certification."""
    errors = []
    if not isinstance(record, dict) or record.get('version') != 1:
        return ['missing/version']
    for key in ('stages', 'scan', 'aggregate', 'counter_names', 'counter_values',
                'io', 'scheduler', 'process_cpu', 'events', 'trace_enabled'):
        if key not in record:
            errors.append('missing '+key)
    if len(record.get('counter_names', [])) != len(record.get('counter_values', [])):
        errors.append('counter schema mismatch')
    for stage in REQUIRED_STAGES:
        metrics = record.get('stages', {}).get(stage)
        if not isinstance(metrics, dict) or any(not isinstance(metrics.get(k), int) or metrics[k] < 0 for k in ('calls', 'service_wall_ns')):
            errors.append('missing/invalid stage '+stage)
    if not set(REQUIRED_COUNTERS) <= set(record.get('counter_names', [])):
        errors.append('missing counters')
    if any(not isinstance(n, int) or n < 0 for n in record.get('counter_values', [])):
        errors.append('invalid counter')
    if len(record.get('scheduler', [])) != 6 or len(record.get('io', [])) != 3 or any(len(row) != 3 for row in record.get('io', [])):
        errors.append('missing scheduler/I/O counters')
    if record.get('process_cpu') is None:
        errors.append('missing process CPU')
    if record.get('dropped_events'):
        errors.append('dropped events')
    aggregate = record.get('aggregate')
    if aggregate is not None:
        levels = record.get('spill_levels')
        fields = record.get('spill_level_fields', [])
        if not isinstance(levels, list) or len(levels) != 9 or any(len(row) != len(fields) for row in levels):
            errors.append('missing spill levels')
        else:
            for field, native in [('write_bytes', 'spill_bytes_written'), ('read_bytes', 'spill_bytes_read'), ('opens', 'spill_files_opened')]:
                if field not in fields or native not in aggregate:
                    errors.append('missing '+field)
                elif sum(row[fields.index(field)] for row in levels) != aggregate[native]:
                    errors.append('spill total mismatch: '+field)
    events = record.get('events', [])
    pending = {}
    for event in sorted(events, key=lambda e: e[1]):
        kind, ns, pipeline, task, wave, value = event
        identity = (pipeline, task, wave)
        if kind in ('dispatch', 'wave_start', 'io_queued'):
            pending[kind, identity] = ns
        elif kind == 'step_start':
            start = pending.pop(('dispatch', identity), None)
            if start is None or ns < start:
                errors.append('step without dispatch')
            pending['step_start', identity] = ns
        elif kind in ('yield', 'blocked', 'done'):
            if pending.pop(('step_start', identity), None) is None:
                errors.append('completion without step')
            if value == (1 << 64)-1:
                errors.append('worker CPU unavailable')
        elif kind == 'wave_end':
            if pending.pop(('wave_start', identity), None) is None:
                errors.append('wave without start')
        elif kind == 'io_start':
            if pending.pop(('io_queued', identity), None) is None:
                errors.append('I/O without submission')
            pending['io_start', identity] = ns
        elif kind in ('io_write_end', 'io_read_end', 'io_repartition_end'):
            if pending.pop(('io_start', identity), None) is None:
                errors.append('I/O completion without start')
        elif kind == 'io_rejected':
            pending.pop(('io_queued', identity), None)
    if pending:
        errors.append('unfinished intervals')
    return errors


def assess(samples, candidate, reference, limit, *, settled, floor=False,
           complete_reference=True, require_stats=True, tolerance=.05):
    """Card 0008: median AND four of five; largest median primary/twin noise.

    Missing cells are NOT RUN; failed reference has no denominator (UNASSESSED).
    Correctness/resource failures must be gated before calling this function.
    All timed Nata records must be complete, including zero dropped events.
    """
    result = dict(policy=POLICY, verdict='NOT RUN', limit=float(limit), floor=floor)
    limit, tolerance = Fraction(str(limit)), Fraction(str(tolerance))
    if any(s['exit'] for s in samples if s['variant'] == candidate):
        return dict(result, verdict='NOT MET', reason='candidate execution failure')
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
            if not record or record.get('version') != 1 or not record.get('complete') or record.get('dropped_events') != 0 or reconcile_stats(record):
                return dict(result, reason='missing/incomplete stats')
    ratios = [c/r for c, r in zip(arms[candidate, 'primary'], arms[reference, 'primary'])]
    noises = twin_noise(arms, (candidate, reference))
    floats = list(map(float, ratios))
    result.update(ratios=floats, distribution=distribution(floats), twin_noise={k: float(v) for k, v in noises.items()})
    if not settled or max(noises.values()) > tolerance:
        return dict(result, verdict='UNRESOLVED')
    within = [r >= limit if floor else r <= limit for r in ratios]
    median_ok = statistics.median(ratios) >= limit if floor else statistics.median(ratios) <= limit
    verdict = 'MET' if median_ok and sum(within) >= 4 else (
        'NOT MET' if not median_ok and within.count(False) >= 4 else 'UNRESOLVED')
    return dict(result, verdict=verdict)


def aggregate_limit(query, scale, spilled):
    """Frozen latency ceilings; HC residency has no invented speed target."""
    if scale < 100 and query == 'q1':
        return 1.5
    if scale < 100 and query in ('agg_orderkey', 'agg_partkey'):
        return 2.0
    return 2.0 if spilled else None


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




def assess_off_cost(samples, candidate, reference, *, settled):
    """Predeclared package-0 bound: default disabled vs compiled-out within twins.

    Both sides of 1±N must satisfy median/four-of-five, with N the maximum
    primary/twin noise and the existing 5% ceiling. No separate tolerance chosen
    from a measured candidate speedup.
    """
    probe = assess(samples, candidate, reference, 1, settled=settled, require_stats=False)
    if 'twin_noise' not in probe:
        return probe
    noise = max(twin_noise(paired(samples, (candidate, reference)), (candidate, reference)).values())
    upper = assess(samples, candidate, reference, 1+noise, settled=settled, require_stats=False)
    lower = assess(samples, candidate, reference, 1-noise, settled=settled, require_stats=False, floor=True)
    verdicts = {upper['verdict'], lower['verdict']}
    verdict = 'NOT MET' if 'NOT MET' in verdicts else ('MET' if verdicts == {'MET'} else 'UNRESOLVED')
    return dict(upper, verdict=verdict, lower_limit=float(1-noise), rule='within twin noise')


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
