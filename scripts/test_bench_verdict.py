"""Failing controls for card 0008's explicit (not slice 5) verdict policy."""
import copy
from fractions import Fraction
import unittest
from bench_verdict import assess, paired, parse_stats, assess_off_cost, reconcile_stats, REQUIRED_STAGES, REQUIRED_COUNTERS, aggregate_limit


def stats_record():
    return dict(version=1, complete=True, dropped_events=0, trace_enabled=False,
                stages={s: dict(calls=0, service_wall_ns=0) for s in REQUIRED_STAGES}, scan=dict(rows=0, bytes=0, batches=0), aggregate=None,
                counter_names=list(REQUIRED_COUNTERS), counter_values=[0]*8, io=[[0]*3 for _ in range(3)], scheduler=[0]*6,
                process_cpu=dict(user_ticks=0, system_ticks=0, scope='process_delta'), events=[])


def samples(ratios=(1,)*5):
    return [dict(variant=v, role=role, round=r, exit=0,
                 seconds=ratios[r] if v == 'candidate' else 1,
                 stats=stats_record())
            for r in range(5) for v in ('baseline', 'candidate') for role in ('primary', 'twin')]


class Verdict(unittest.TestCase):
    def judge(self, rows, **kw):
        return assess(rows, 'candidate', 'baseline', 1.10, settled=True, **kw)['verdict']

    def test_slowdown_and_pass(self):
        self.assertEqual(self.judge(samples()), 'MET')
        self.assertEqual(self.judge(samples((1.11,)*5)), 'NOT MET')

    def test_four_and_median_both_required(self):
        self.assertEqual(self.judge(samples((1, 1, 1, 1.2, 1.2))), 'UNRESOLVED')
        self.assertEqual(self.judge(samples((1, 1, 1, 1, 1.2))), 'MET')
        self.assertEqual(self.judge(samples((1, 1.2, 1.2, 1.2, 1.2))), 'NOT MET')

    def test_noise_definition_and_ceiling(self):
        rows = samples()
        for s in rows:
            if s['variant'] == 'baseline' and s['role'] == 'twin':
                s['seconds'] = .9
        self.assertEqual(self.judge(rows), 'UNRESOLVED')
        result = assess(rows, 'candidate', 'baseline', 1.1, settled=True)
        self.assertAlmostEqual(result['twin_noise']['baseline'], 1/.9-1)

    def test_missing_duplicate_bad_and_failed_reference(self):
        self.assertEqual(self.judge([]), 'NOT RUN')
        rows = samples(); rows[0]['exit'] = 1
        self.assertEqual(self.judge(rows), 'UNASSESSED')
        rows = samples(); rows[2]['exit'] = 1
        self.assertEqual(self.judge(rows), 'NOT MET')
        for val in (0, -1, float('nan'), float('inf')):
            rows = samples(); rows[2]['seconds'] = val
            self.assertEqual(self.judge(rows), 'NOT RUN')
        rows = samples(); rows[4]['round'] = 0
        self.assertEqual(self.judge(rows), 'NOT RUN')
        self.assertEqual(self.judge(samples()+[samples()[0]]), 'NOT RUN')

    def test_records_are_mandatory_and_no_drops(self):
        for bad in (None, {}, dict(version=2, complete=True, dropped_events=0),
                    dict(version=1, complete=False, dropped_events=0),
                    dict(version=1, complete=True, dropped_events=1)):
            rows = samples(); rows[2]['stats'] = bad
            self.assertEqual(self.judge(rows), 'NOT RUN')

    def test_unsettled_and_floor_serial_work(self):
        self.assertEqual(assess(samples(), 'candidate', 'baseline', 1.1, settled=False)['verdict'], 'UNRESOLVED')
        self.assertEqual(assess(samples((1,)*5), 'candidate', 'baseline', 3.5, floor=True, settled=True)['verdict'], 'NOT MET')
        self.assertEqual(assess(samples((5,)*5), 'candidate', 'baseline', 5, floor=True, settled=True)['verdict'], 'MET')

    def test_round_identity_not_list_order(self):
        rows = samples((1, 1, 1, 1, 1.2))
        self.assertEqual(self.judge(rows), self.judge(list(reversed(rows))))
        self.assertEqual(paired(list(reversed(rows)), ['candidate'])['candidate', 'primary'], [1, 1, 1, 1, Fraction('1.2')])

    def test_bounds_and_failures(self):
        self.assertEqual(self.judge(samples((1.1,)*5)), 'MET')
        for key in ('correct', 'resources_ok'):
            rows = samples(); rows[2][key] = False
            self.assertEqual(self.judge(rows), 'NOT MET')
        rows = samples(); rows[2]['error'] = 'mismatch'
        self.assertEqual(self.judge(rows), 'NOT MET')

    def test_exact_noise_and_nanosecond_boundaries(self):
        rows = samples()
        for row in rows:
            if row['variant'] == 'baseline' and row['role'] == 'primary':
                row['seconds'] = 1.05
        self.assertEqual(self.judge(rows), 'MET')  # exactly 5%, not IEEE overshoot
        rows = samples()
        for row in rows:
            row['elapsed_ns'] = 1_100_000_000 if row['variant'] == 'candidate' else 1_000_000_000
            row['seconds'] = row['elapsed_ns']/1e9
        self.assertEqual(self.judge(rows), 'MET')
        for row in rows:
            if row['variant'] == 'candidate':
                row['elapsed_ns'] += 1
                row['seconds'] = row['elapsed_ns']/1e9
        self.assertEqual(self.judge(rows), 'NOT MET')

    def test_one_record_only(self):
        line = 'query_stats: {"version":1}\n'
        self.assertEqual(parse_stats(line), {'version': 1})
        self.assertIsNone(parse_stats('aggregate: n=1'))
        with self.assertRaises(ValueError):
            parse_stats(line+line)


class OffCost(unittest.TestCase):
    def test_off_cost_within_twins(self):
        self.assertEqual(assess_off_cost(samples(), 'candidate', 'baseline', settled=True)['verdict'], 'MET')
        self.assertEqual(assess_off_cost(samples((1.01,)*5), 'candidate', 'baseline', settled=True)['verdict'], 'NOT MET')
        self.assertEqual(assess_off_cost(samples(), 'candidate', 'baseline', settled=False)['verdict'], 'UNRESOLVED')


class Reconciliation(unittest.TestCase):
    def test_fake_same_run_and_mutations(self):
        s = stats_record()
        s.update(spill_levels=[[32, 32, 1] for _ in range(9)],
                 spill_level_fields=['write_bytes', 'read_bytes', 'opens'],
                 aggregate=dict(spill_bytes_written=288, spill_bytes_read=288, spill_files_opened=9),
                 events=[['wave_start',0,1,0,1,1], ['dispatch',1,1,3,1,0],
                         ['step_start',2,1,3,1,0], ['yield',3,1,3,1,1], ['wave_end',4,1,0,1,1]])
        self.assertEqual(reconcile_stats(s), [])
        bad = copy.deepcopy(s); del bad['stages']
        self.assertIn('missing stages', reconcile_stats(bad))
        bad = copy.deepcopy(s); del bad['stages']['filter']
        self.assertIn('missing/invalid stage filter', reconcile_stats(bad))
        bad = copy.deepcopy(s); bad['spill_levels'][1][0] += 1
        self.assertIn('spill total mismatch: write_bytes', reconcile_stats(bad))
        bad = copy.deepcopy(s); bad['events'].pop(2)
        self.assertTrue(reconcile_stats(bad))
        bad = copy.deepcopy(s); bad['dropped_events'] = 1
        self.assertIn('dropped events', reconcile_stats(bad))
        bad = copy.deepcopy(s); bad['spill_levels'] = []
        self.assertIn('missing spill levels', reconcile_stats(bad))


class FrozenLimits(unittest.TestCase):
    def test_full_matrix_latency_rules(self):
        for scale in (1, 10):
            for spilled in (False, True):
                self.assertEqual(aggregate_limit('q1', scale, spilled), 1.5)
                for query in ('agg_orderkey', 'agg_partkey'):
                    self.assertEqual(aggregate_limit(query, scale, spilled), 2)
            self.assertIsNone(aggregate_limit('high_cardinality_tpch', scale, False))
            self.assertEqual(aggregate_limit('high_cardinality_tpch', scale, True), 2)
        self.assertIsNone(aggregate_limit('q1', 100, False))
