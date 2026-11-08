"""Failing controls for card 0008's explicit (not slice 5) verdict policy."""
import copy
import unittest
from bench_verdict import assess, paired, parse_stats


def samples(ratios=(1,)*5):
    return [dict(variant=v, role=role, round=r, exit=0,
                 seconds=ratios[r] if v == 'candidate' else 1,
                 stats=dict(version=1, complete=True, dropped_events=0))
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
        self.assertEqual(self.judge(rows), 'NOT RUN')
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
        self.assertEqual(paired(list(reversed(rows)), ['candidate'])['candidate', 'primary'], [1, 1, 1, 1, 1.2])

    def test_bounds_and_failures(self):
        self.assertEqual(self.judge(samples((1.1,)*5)), 'MET')
        for key in ('correct', 'resources_ok'):
            rows = samples(); rows[2][key] = False
            self.assertEqual(self.judge(rows), 'NOT MET')
        rows = samples(); rows[2]['error'] = 'mismatch'
        self.assertEqual(self.judge(rows), 'NOT MET')

    def test_one_record_only(self):
        line = 'query_stats: {"version":1}\n'
        self.assertEqual(parse_stats(line), {'version': 1})
        self.assertIsNone(parse_stats('aggregate: n=1'))
        with self.assertRaises(ValueError):
            parse_stats(line+line)
