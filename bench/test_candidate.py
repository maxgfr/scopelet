import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import candidate
import export_candidate


def row(task, arm, repetition=1, passed=True, tokens=100, cost=0.01):
    usage = {'logical_input_tokens': tokens - 10, 'output_tokens': 10, 'total_cost_usd_reported': cost, 'usage_missing': False}
    return {'task': task, 'arm': arm, 'repetition': repetition, 'passed': passed, 'usage': usage, 'duration_seconds': 1.0,
            'exit_code': 0, 'timed_out': False}


def campaign(candidate_tokens=90, candidate_pass=True):
    rows = []
    for task in candidate.TASKS:
        for repetition in (1, 2):
            rows.append(row(task, 'native', repetition, tokens=120))
            rows.append(row(task, 'released', repetition, tokens=100))
            rows.append(row(task, 'candidate', repetition, tokens=candidate_tokens, passed=candidate_pass))
    return rows


class CandidateTests(unittest.TestCase):
    def test_matrix_is_bounded_balanced_and_reproducible(self):
        cases = candidate.matrix(2)
        self.assertEqual(len(cases), candidate.MAX_ATTEMPTS)
        self.assertEqual(cases, candidate.matrix(2))
        self.assertEqual(len({candidate.case_id(c) for c in cases}), 18)
        for arm in candidate.ARMS:
            self.assertEqual(sum(c['arm'] == arm for c in cases), 6)
        self.assertEqual(candidate.MODEL, 'claude-haiku-4-5-20251001')

    def test_dry_run_writes_the_plan_without_calling_a_model(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(candidate, 'run_case', side_effect=AssertionError('no calls')), patch('builtins.print'):
            out = Path(tmp) / 'plan'
            self.assertEqual(candidate.main(['--out', str(out)]), 0)
            report = json.loads((out / 'report.json').read_text())
        self.assertFalse(report['live'])
        self.assertEqual(len(report['matrix']), 18)
        self.assertEqual(report['runs'], [])
        self.assertIsNone(report['decision']['go'])
        self.assertIn('task4 and task2', report['decision_rule'])

    def test_missing_usage_and_failures_remain_in_summary(self):
        cell = candidate.summarize([{'task': 'task4', 'arm': 'candidate', 'passed': False}])['task4/candidate']
        self.assertEqual(cell['attempts'], 1)
        self.assertEqual(cell['passes'], 0)
        self.assertEqual(cell['missing_usage'], 1)
        self.assertIsNone(cell['mean_tokens'])
        self.assertIsNone(cell['total_cost_usd_reported'])

    def test_decision_follows_the_predeclared_rule(self):
        go = candidate.decision(candidate.summarize(campaign()))
        self.assertTrue(go['go'])
        self.assertEqual(go['combined_mean_tokens'], {'released': 200, 'candidate': 180})
        more_tokens = candidate.decision(candidate.summarize(campaign(candidate_tokens=101)))
        self.assertFalse(more_tokens['go'])
        self.assertIn('candidate combined mean tokens above released', more_tokens['reasons'])
        failure = candidate.decision(candidate.summarize(campaign(candidate_pass=False)))
        self.assertFalse(failure['go'])
        self.assertTrue(any('functional failure' in r for r in failure['reasons']))
        rows = campaign()
        rows[0]['task'] = 'task3'
        partial = candidate.decision(candidate.summarize([r for r in rows if r['task'] != 'task2']))
        self.assertIsNone(partial['go'])

    def test_ledger_never_overwrites_or_exceeds_the_budget(self):
        report = {'runs': [{'task': 'task4', 'arm': 'native', 'repetition': 1, 'status': 'started'}]}
        with self.assertRaises(ValueError):
            candidate.reserve(report, {'task': 'task4', 'arm': 'native', 'repetition': 1}, 18)
        with self.assertRaises(ValueError):
            candidate.reserve(report, {'task': 'task4', 'arm': 'released', 'repetition': 1}, 1)
        candidate.reserve(report, {'task': 'task4', 'arm': 'released', 'repetition': 1}, 2)
        self.assertEqual(len(report['runs']), 2)

    def test_export_strips_private_paths_and_keeps_failures(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            report = {'schema_version': 1, 'model': candidate.MODEL, 'live': True, 'private_paths': {'candidate': '/private/operator/candidate'},
                      'binary_sha256': {'candidate': 'abc'},
                      'runs': [{'task': 'task4', 'arm': 'candidate', 'repetition': 1, 'status': 'harness_error',
                                'error_type': 'ValueError', 'passed': False, 'command': ['/private/operator/claude']}]}
            (root / 'report.json').write_text(json.dumps(report))
            result = export_candidate.export(root)
        self.assertNotIn('/private/operator', json.dumps(result))
        self.assertFalse(result['runs'][0]['passed'])
        self.assertEqual(result['summary']['task4/candidate']['attempts'], 1)
        self.assertIsNone(result['decision']['go'])


if __name__ == '__main__':
    unittest.main()
