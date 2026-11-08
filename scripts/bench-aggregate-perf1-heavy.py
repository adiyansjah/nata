#!/usr/bin/env python3
"""Package committed revisions, then benchmark on one settled c3d-highcpu-8 or n4a-highcpu-8.

prepare runs locally and makes a git-free source bundle for the existing heavy
job uploader. run is for the parent's approved VM only; --plan launches nothing.
No cloud API, provisioning, credentials, or machine identifiers are used.
"""
import argparse
import collections
import csv
import decimal
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import tarfile
import tempfile
import time
from bench_verdict import distribution, invoke as invoke_process

ROUNDS = 5
BUDGETS = (8 << 30, 64 << 20)
QUERIES = ('q1', 'high_cardinality_tpch')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def safe_extract(archive, dest):
    """Extract a git-archive tarball safely. Python 3.11.2 (the Debian 12 VM image) has no extractall(filter=), found
    2026-11-07 when the first timing run failed on it, so fall back to checking every member ourselves."""
    try:
        archive.extractall(dest, filter='data')
        return
    except TypeError:
        pass
    root = Path(dest).resolve()
    for member in archive.getmembers():
        target = (root / member.name).resolve()
        if not (target == root or str(target).startswith(str(root) + os.sep)):
            raise ValueError(f'archive member escapes the destination: {member.name}')
        if member.issym() or member.islnk():
            link = (target.parent / member.linkname).resolve()
            if not str(link).startswith(str(root) + os.sep):
                raise ValueError(f'archive link escapes the destination: {member.name}')
        if not (member.isfile() or member.isdir() or member.issym()):
            raise ValueError(f'unexpected archive member type: {member.name}')
    archive.extractall(dest)


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()


def prepare(args):
    if not 1 <= len(args.fix) <= 4:
        raise ValueError('supply one to four cumulative fix commits, in order')
    entries = [('baseline', args.baseline)]
    for item in args.fix:
        label, ref = item.split('=', 1)
        if not re.fullmatch(r'[a-z][a-z0-9-]{0,31}', label) or label in ('baseline', 'duckdb'):
            raise ValueError('fix labels must be short, unique names using a-z, 0-9, -')
        entries.append((label, ref))
    if len({label for label, _ in entries}) != len(entries):
        raise ValueError('duplicate label')
    args.bundle.mkdir(parents=True, exist_ok=True)
    if (args.bundle / 'manifest.json').exists():
        raise ValueError('bundle already exists; choose a fresh output directory')
    revisions = []
    previous = None
    for label, ref in entries:
        commit = git(args.repo, 'rev-parse', '--verify', '--end-of-options', ref + '^{commit}')
        if previous:
            if previous == commit:
                raise ValueError('fix commits must be distinct')
            subprocess.run(['git', '-C', str(args.repo), 'merge-base', '--is-ancestor', previous, commit], check=True)
        name = label + '-' + commit + '.tar.gz'
        archive = args.bundle / name
        subprocess.run(['git', '-C', str(args.repo), 'archive', '--format=tar.gz', '--output', str(archive.resolve()), commit], check=True)
        revisions.append(dict(label=label, commit=commit, archive=name, archive_sha256=digest(archive)))
        previous = commit
    queries = {q: (args.repo / f'check/queries/aggregate/{q}.sql').read_text() for q in QUERIES}
    save(args.bundle / 'manifest.json', dict(version=1, revisions=revisions, queries=queries))
    print(f'Prepared {len(revisions)} committed source archives; the heavy job needs no .git directory.')


def manifest(bundle):
    value = json.loads((bundle / 'manifest.json').read_text())
    if value['version'] != 1 or not 2 <= len(value['revisions']) <= 5 or set(value['queries']) != set(QUERIES):
        raise ValueError('invalid bundle manifest')
    labels = set()
    for revision in value['revisions']:
        label, commit, name = (revision[k] for k in ('label', 'commit', 'archive'))
        if not re.fullmatch(r'[a-z][a-z0-9-]{0,31}', label) or label in labels or label == 'duckdb':
            raise ValueError('invalid/duplicate revision label')
        if not re.fullmatch(r'[0-9a-f]{40}', commit) or name != label + '-' + commit + '.tar.gz':
            raise ValueError('invalid revision identity')
        if digest(bundle / name) != revision['archive_sha256']:
            raise ValueError('source archive checksum mismatch')
        labels.add(label)
    if value['revisions'][0]['label'] != 'baseline':
        raise ValueError('baseline must be first')
    return value


def layout(work, revision):
    name = revision['label'] + '-' + revision['commit']
    return work / 'source' / name, work / 'target' / name


def arms(revisions, round_number):
    result = [(r['label'], role) for r in revisions for role in ('primary', 'twin')]
    result += [('duckdb', 'primary'), ('duckdb', 'twin')]
    return result if round_number % 2 == 0 else list(reversed(result))


def command(binary, sql, threads, budget, spill):
    if binary == 'duckdb':
        directory = str(spill).replace("'", "''")
        return ['duckdb', '-csv', '-c', f"SET threads={threads}; SET memory_limit='{budget}B'; SET temp_directory='{directory}'; " + sql]
    return [str(binary), 'sql', '--csv', '--report', '--threads', str(threads), '--aggregate-memory-bytes', str(budget), '--spill-directory', str(spill), '--spill-disk-bytes', str(4 << 30), '--command=' + sql]


def invoke(argv, env, destination=None):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        row = invoke_process(argv, env, destination or root/'out', root/'stderr', 600, capture_usage=False)
        if row['exit']:
            row['error'] = (root/'stderr').read_text()[-2000:]
        return row


def rows(path, query):
    with path.open(newline='') as stream:
        reader = csv.reader(stream)
        width = 10 if query == 'q1' else 4
        if len(next(reader)) != width:
            raise ValueError('unexpected CSV header width')
        result = collections.Counter()
        for row in reader:
            if len(row) != width:
                raise ValueError('unexpected CSV row width')
            result[tuple(value if query == 'q1' and i < 2 else decimal.Decimal(value) for i, value in enumerate(row))] += 1
        return result


def equal_values(expected, actual, query):
    if query != 'q1':
        return expected == actual
    # Four unique Q1 keys; preserve multiplicity as well as the AVG tolerance.
    left, right = sorted(expected.elements()), sorted(actual.elements())
    if len(left) != len(right):
        return False
    for a, b in zip(left, right):
        if a[:2] != b[:2]:
            return False
        for slot, (x, y) in enumerate(zip(a[2:], b[2:]), 2):
            if not (abs(x-y) < decimal.Decimal('1e-8') if 6 <= slot <= 8 else x == y):
                return False
    return True


def summarize(observations, labels):
    result = []
    for query in QUERIES:
        for threads in (1, 8):
            for budget in BUDGETS:
                cell = [o for o in observations if (o['query'], o['threads'], o['budget']) == (query, threads, budget)]
                by_arm = {}
                for label in [*labels, 'duckdb']:
                    for role in ('primary', 'twin'):
                        samples = sorted((o for o in cell if (o['variant'], o['role']) == (label, role)), key=lambda o: o['round'])
                        if len(samples) == ROUNDS and all(o['exit'] == 0 for o in samples):
                            by_arm[label, role] = [o['seconds'] for o in samples]
                row = dict(query=query, threads=threads, budget=budget, arms={}, twin_ratios={}, versus_baseline={}, versus_previous={}, versus_duckdb={})
                for label in [*labels, 'duckdb']:
                    for role in ('primary', 'twin'):
                        values = by_arm.get((label, role))
                        row['arms'][label + '-' + role] = distribution(values) if values else None
                    if (label, 'primary') in by_arm and (label, 'twin') in by_arm:
                        row['twin_ratios'][label] = distribution([t/p for p,t in zip(by_arm[label, 'primary'], by_arm[label, 'twin'])])
                for i, label in enumerate(labels):
                    current = by_arm.get((label, 'primary'))
                    if not current:
                        continue
                    for field, comparator in [('versus_baseline', labels[0]), ('versus_previous', labels[max(i-1, 0)]), ('versus_duckdb', 'duckdb')]:
                        reference = by_arm.get((comparator, 'primary'))
                        if reference:
                            # Candidate/reference latency ratio; lower is faster.
                            row[field][label] = distribution([v/r for v,r in zip(current, reference)])
                result.append(row)
    return result


def run(args):
    bundle = args.bundle.resolve()
    value = manifest(bundle)
    revisions = value['revisions']
    work, out = args.work.resolve(), args.out.resolve()
    labels = [r['label'] for r in revisions]
    builds = []
    for revision in revisions:
        source, target = layout(work, revision)
        builds.append(dict(variant=revision['label'], commit=revision['commit'], source=str(source), target=str(target), command=['cargo', 'build', '--locked', '--release', '-p', 'nata']))
    plan = dict(builds=builds, queries=list(QUERIES), threads=[1,8], budgets=list(BUDGETS), rounds=ROUNDS, roles=['primary','twin'], timed_processes=len(QUERIES)*2*2*ROUNDS*len(arms(revisions,0)), order=[arms(revisions,i) for i in range(ROUNDS)])
    if args.plan:
        print(json.dumps(plan, indent=2))
        return
    expected_arch = {'c3d-highcpu-8': 'x86_64', 'n4a-highcpu-8': 'aarch64'}[args.machine]
    if platform.machine() != expected_arch or os.cpu_count() != 8:
        raise ValueError(f'this run requires {args.machine} ({expected_arch}, 8 logical CPUs)')
    if (out / 'timings.json').exists():
        raise ValueError('use a fresh output directory; never mix interrupted sessions')
    out.mkdir(parents=True, exist_ok=True)
    temporary = work / 'tmp'
    temporary.mkdir(parents=True, exist_ok=True)
    spill = work / 'spill'
    spill.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, TMPDIR=str(temporary), CARGO_BUILD_JOBS='8', CARGO_INCREMENTAL='0', RUSTFLAGS='', CARGO_ENCODED_RUSTFLAGS='')
    for key in list(env):
        if key.startswith('CARGO_PROFILE_') or key in ('AI_AGENT', 'CODEX_CI'):
            env.pop(key)
    binaries = {}
    build_records = []
    for revision in revisions:
        source, target = layout(work, revision)
        if source.exists():
            shutil.rmtree(source)
        source.mkdir(parents=True)
        with tarfile.open(bundle / revision['archive'], 'r:gz') as archive:
            safe_extract(archive, source)
        for query in QUERIES:
            if (source / f'check/queries/aggregate/{query}.sql').read_text() != value['queries'][query]:
                raise ValueError('queries differ between the committed revisions or manifest')
        build_env = dict(env, CARGO_TARGET_DIR=str(target))
        with (out / ('build-' + revision['label'] + '.log')).open('w') as log:
            subprocess.run(['cargo','build','--locked','--release','-p','nata'], cwd=source, env=build_env, stdout=log, stderr=subprocess.STDOUT, check=True)
        binary = target / 'release/nata'
        binaries[revision['label']] = binary
        build_records.append(dict(**revision, binary_sha256=digest(binary)))
    parquet = args.data.resolve() / 'lineitem.parquet'
    sqls = {q: s.replace('{data}', str(args.data.resolve()).replace("'", "''")) for q,s in value['queries'].items()}
    metadata = dict(queries=value['queries'], expected_machine=args.machine, architecture=platform.machine(), logical_cpus=os.cpu_count(), cpu_model=next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith(('model name', 'Hardware', 'CPU part'))), platform.machine()), rustc=subprocess.check_output(['rustc','--version'],env=env,text=True).strip(), duckdb=subprocess.check_output(['duckdb','--version'],env=env,text=True).strip(), data_bytes=parquet.stat().st_size, data_sha256=digest(parquet), revisions=build_records, plan=plan, verdict='measurements only; parent judges attribution against twin noise and frozen targets')
    count_sql = "SELECT count(*) FROM '" + str(parquet).replace("'", "''") + "';"
    counted = subprocess.check_output(['duckdb','-csv','-c',count_sql],env=env,text=True)
    metadata['input_rows'] = int(list(csv.reader(counted.splitlines()))[1][0])
    if metadata['input_rows'] != 6_001_215:
        raise ValueError('this job requires SF1 lineitem (6,001,215 rows)')
    save(out / 'metadata.json', metadata)
    gates = []
    # Gate every variant/cell before ANY timed block, then settle away builds
    # and gate CPU load. No correctness process competes with timed processes.
    for query in QUERIES:
        for threads in (1,8):
            reference_path = temporary / 'reference.csv'
            oracle = invoke(command('duckdb', sqls[query], threads, BUDGETS[0], spill), env, reference_path)
            if oracle['exit']:
                raise RuntimeError('in-memory oracle failed: ' + oracle.get('error',''))
            expected = rows(reference_path, query)
            reference_path.unlink()
            for budget in BUDGETS:
                for label in labels:
                    actual_path = temporary / 'actual.csv'
                    observation = invoke(command(binaries[label],sqls[query],threads,budget,spill),env,actual_path)
                    if observation['exit'] or not equal_values(expected, rows(actual_path,query), query):
                        raise RuntimeError(f'correctness gate failed: {label}/{query}/{threads}/{budget}: {observation}')
                    actual_path.unlink()
                    metrics = observation['counters']
                    if not metrics or metrics['peak_reservation'] > budget:
                        raise RuntimeError('missing counters or budget breach')
                    if budget == BUDGETS[0] and metrics['spill_bytes_written'] != 0:
                        raise RuntimeError('memory control unexpectedly spilled')
                    if query == 'high_cardinality_tpch' and budget == BUDGETS[1] and metrics['spill_bytes_written'] == 0:
                        raise RuntimeError('small-budget HAVING did not spill')
                    gates.append(dict(query=query,threads=threads,budget=budget,variant=label,rows=sum(expected.values()),counters=metrics))
                    save(out / 'gates.json',gates)
            del expected
    observations = []
    settle = Path(__file__).resolve().parent / 'gcp/settle.sh'
    for query in QUERIES:
        for threads in (1,8):
            for budget in BUDGETS:
                cell = f'{query}-{threads}-{budget}'
                result = subprocess.run(['bash',str(settle),cell,'0.5','600'],env=env,capture_output=True,text=True)
                (out / ('settle-'+cell+'.txt')).write_text(result.stdout+result.stderr)
                if result.returncode:
                    raise RuntimeError('unsettled cell; no timing verdict: '+cell)
                for round_number in range(ROUNDS):
                    for label, role in arms(revisions,round_number):
                        binary = 'duckdb' if label == 'duckdb' else binaries[label]
                        observation = invoke(command(binary,sqls[query],threads,budget,spill),env)
                        observation.update(query=query,threads=threads,budget=budget,round=round_number,variant=label,role=role,settled=True)
                        observations.append(observation)
                        save(out / 'timings.json',observations)
                        if label != 'duckdb' and observation['exit']:
                            raise RuntimeError('candidate failed a timed run: '+str(observation))
                save(out / 'summary.json',summarize(observations,labels))
                print('completed',cell,flush=True)
    save(out / 'summary.json',summarize(observations,labels))
    print('COMPLETE: five rounds per primary/twin arm; failed DuckDB cells have no denominator. No automatic target verdict.')


def simulated_run(directory, revisions, machine='c3d-highcpu-8'):
    """Exercise the full orchestration with inert engines, without cloud/timings."""
    import contextlib
    import io
    from unittest.mock import patch

    bundle = directory / 'simulation-bundle'
    bundle.mkdir()
    queries = {q: f"SELECT '{q}';" for q in QUERIES}
    for revision in revisions:
        archive = bundle / revision['archive']
        with tarfile.open(archive, 'w:gz') as tar:
            for query, sql in queries.items():
                payload = sql.encode()
                info = tarfile.TarInfo(f'check/queries/aggregate/{query}.sql')
                info.size = len(payload)
                tar.addfile(info, io.BytesIO(payload))
        revision['archive_sha256'] = digest(archive)
    save(bundle/'manifest.json', dict(version=1, revisions=revisions, queries=queries))
    data = directory/'simulation-data'
    data.mkdir()
    (data/'lineitem.parquet').write_bytes(b'inert self-test fixture')
    args = argparse.Namespace(machine=machine, bundle=bundle, work=directory/'simulation-work', data=data, out=directory/'simulation-results', plan=False)
    built, calls = [], []

    def process(argv, **kwargs):
        if argv[:2] == ['cargo', 'build']:
            target = Path(kwargs['env']['CARGO_TARGET_DIR'])
            built.append(target)
            binary = target/'release/nata'
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b'inert engine')
        else:
            assert argv[0] == 'bash' and argv[1].endswith('/gcp/settle.sh')
        return subprocess.CompletedProcess(argv, 0, stdout='settled at load 0\n', stderr='')

    def output(argv, **kwargs):
        if argv == ['rustc', '--version']: return 'inert rustc\n'
        if argv == ['duckdb', '--version']: return 'inert duckdb\n'
        assert argv[:3] == ['duckdb', '-csv', '-c'] and 'count(*)' in argv[3]
        return 'n\n6001215\n'

    def engine(argv, env, destination=None):
        duck = argv[0] == 'duckdb'
        budget = int(re.search(r"memory_limit='([0-9]+)B'", argv[-1])[1]) if duck else int(argv[argv.index('--aggregate-memory-bytes')+1])
        query = 'high_cardinality_tpch' if 'high_cardinality_tpch' in argv[-1] else 'q1'
        calls.append((duck, destination is None, query, budget))
        if destination:
            destination.write_text('a,b,c,d,e,f,g,h,i,j\nA,F,1,2,3,4,5,6,0.1,10\n' if query == 'q1' else 'a,b,c,d\n1,2,3,4\n')
        return dict(seconds=1.0, load=[0,0,0], exit=0, counters=dict(peak_reservation=budget, spill_bytes_written=int(query=='high_cardinality_tpch' and budget==BUDGETS[1])))

    with patch.object(platform,'machine',return_value='aarch64' if machine == 'n4a-highcpu-8' else 'x86_64'), patch.object(os,'cpu_count',return_value=8), patch.object(subprocess,'run',side_effect=process), patch.object(subprocess,'check_output',side_effect=output), patch.dict(globals(),invoke=engine), contextlib.redirect_stdout(io.StringIO()):
        run(args)
    assert len(built) == len(set(built)) == len(revisions)
    assert len(json.loads((args.out/'gates.json').read_text())) == 8 * len(revisions)
    samples = json.loads((args.out/'timings.json').read_text())
    assert len(samples) == 80 * (len(revisions) + 1) and sum(timed for _,timed,_,_ in calls) == len(samples)
    assert all(row['arms']['baseline-primary']['n']==5 for row in json.loads((args.out/'summary.json').read_text()))
    assert {s['budget'] for s in samples} == set(BUDGETS)
    assert {s['threads'] for s in samples} == {1,8}
    assert all(not timed for _,timed,_,_ in calls[:8 * len(revisions) + 4])
    assert all(timed for _,timed,_,_ in calls[8 * len(revisions) + 4:])


def self_test(args):
    args.work.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.work) as directory:
        directory = Path(directory)
        left,right = directory/'left.csv',directory/'right.csv'
        left.write_text('k,n,p,v\n1,2,3.00,4.0000\n1,2,3,4\n')
        right.write_text('k,n,p,v\n1,2,3,4\n1,2,3.0,4.00\n')
        assert equal_values(rows(left,'high_cardinality_tpch'),rows(right,'high_cardinality_tpch'),'high_cardinality_tpch')
        right.write_text('k,n,p,v\n1,2,3,4\n')
        assert not equal_values(rows(left,'high_cardinality_tpch'),rows(right,'high_cardinality_tpch'),'high_cardinality_tpch')
        left.write_text('rf,ls,qty,base,disc,charge,avgq,avgp,avgd,n\nA,F,1,2,3,4,5.000000,6.000000,0.1,10\n')
        right.write_text('rf,ls,qty,base,disc,charge,avgq,avgp,avgd,n\nA,F,1,2,3,4,5.0000000001,6.000000,0.1,10\n')
        assert equal_values(rows(left,'q1'),rows(right,'q1'),'q1')
        right.write_text('rf,ls,qty,base,disc,charge,avgq,avgp,avgd,n\nA,F,1.0000000001,2,3,4,5.000000,6.000000,0.1,10\n')
        assert not equal_values(rows(left,'q1'),rows(right,'q1'),'q1')
        right.write_text('rf,ls,qty,base,disc,charge,avgq,avgp,avgd,n\nA,F,1,2,3,4,5,6,0.1\n')
        try:
            rows(right,'q1')
        except ValueError:
            pass
        else:
            raise AssertionError('truncated CSV row was accepted')
        revisions = [dict(label=label,commit=str(i)*40) for i,label in enumerate(['baseline','hash','polls','borrow','chunks'])]
        bundle = directory / 'bundle'
        bundle.mkdir()
        for revision in revisions:
            name = revision['label'] + '-' + revision['commit'] + '.tar.gz'
            with tarfile.open(bundle/name,'w:gz'):
                pass
            revision.update(archive=name, archive_sha256=digest(bundle/name))
        save(bundle/'manifest.json',dict(version=1,revisions=revisions,queries={q:'SELECT 1' for q in QUERIES}))
        assert len(manifest(bundle)['revisions']) == 5
        with (bundle/revisions[0]['archive']).open('ab') as corrupt:
            corrupt.write(b'corrupt')
        try:
            manifest(bundle)
        except ValueError:
            pass
        else:
            raise AssertionError('archive tampering was not rejected')
        assert len(arms(revisions,0)) == 12
        assert arms(revisions,0) == list(reversed(arms(revisions,1)))
        assert len({layout(directory,r)[1] for r in revisions}) == 5
        assert command('duckdb','SELECT 1',8,64<<20,directory)[3].startswith('SET threads=8; SET memory_limit=\'67108864B\';')
        samples=[]
        for round_number in range(ROUNDS):
            for label,role in arms(revisions,round_number):
                samples.append(dict(query='q1',threads=1,budget=BUDGETS[0],round=round_number,variant=label,role=role,seconds=2.0 if label=='baseline' else 1.0,exit=0))
        summary = summarize(samples,[r['label'] for r in revisions])[0]
        assert summary['versus_baseline']['hash']['median'] == 0.5
        assert summary['twin_ratios']['baseline']['median'] == 1.0
        for row in samples:
            if row['variant']=='duckdb':row['exit']=1
        assert not summarize(samples,[r['label'] for r in revisions])[0]['versus_duckdb']
        simulated_run(directory, revisions)
        with tempfile.TemporaryDirectory(dir=directory) as step2:
            simulated_run(Path(step2), revisions[:2], 'n4a-highcpu-8')
    print('PASS: full inert orchestration (5 builds, 40 gates before all 480 timed arms); numeric multisets/multiplicity, alternating twins, distinct targets, matched budgets, paired ratios and incomplete DuckDB denominators; two-revision ARM simulation also passes; no engines launched.')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    subs=parser.add_subparsers(dest='action',required=True)
    p=subs.add_parser('prepare');p.add_argument('--repo',type=Path,default=Path.cwd());p.add_argument('--baseline',default='e198d08');p.add_argument('--fix',action='append',default=[]);p.add_argument('--bundle',type=Path,required=True);p.set_defaults(function=prepare)
    p=subs.add_parser('run');p.add_argument('--machine', choices=['c3d-highcpu-8','n4a-highcpu-8'], default='c3d-highcpu-8');p.add_argument('--bundle',type=Path,required=True);p.add_argument('--work',type=Path,required=True);p.add_argument('--data',type=Path,required=True);p.add_argument('--out',type=Path,required=True);p.add_argument('--plan',action='store_true');p.set_defaults(function=run)
    p=subs.add_parser('self-test');p.add_argument('--work',type=Path,required=True);p.set_defaults(function=self_test)
    args=parser.parse_args();args.function(args)


if __name__=='__main__':
    main()
