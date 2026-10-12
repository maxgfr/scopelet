import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import live

STREAM = '\n'.join(json.dumps(e) for e in [
    {'type': 'system', 'subtype': 'init'},
    {'type': 'assistant', 'message': {'content': [{'type': 'text', 'text': 'Looking.'},
                                                  {'type': 'tool_use', 'name': 'Bash', 'input': {}}]}},
    {'type': 'user', 'message': {'content': [{'type': 'tool_result', 'content': 'x'}]}},
    {'type': 'assistant', 'message': {'content': [{'type': 'tool_use', 'name': 'Read', 'input': {}}]}},
    {'type': 'result', 'subtype': 'success', 'is_error': False, 'num_turns': 3, 'total_cost_usd': 0.0123,
     'result': 'RCPT-7139-QX', 'usage': {'input_tokens': 12, 'cache_creation_input_tokens': 3000,
                                         'cache_read_input_tokens': 20000, 'output_tokens': 150}},
])


class ParseTests(unittest.TestCase):
    def test_a_transcript_gives_usage_turns_tools_and_answer(self):
        record = live.parse_stream(STREAM)
        self.assertEqual(record['tool_calls'], 2)
        self.assertEqual(record['tools'], {'Bash': 1, 'Read': 1})
        self.assertEqual(record['turns'], 3)
        self.assertEqual(record['input_total'], 23012)
        self.assertEqual(record['output_tokens'], 150)
        self.assertEqual(record['answer'], 'RCPT-7139-QX')
        self.assertFalse(record['session_error'])

    def test_missing_usage_is_unknown_never_zero(self):
        record = live.parse_stream('not json\n')
        for key in ('input_tokens', 'input_total', 'output_tokens', 'turns', 'tool_calls'):
            self.assertIsNone(record[key], key)
        self.assertTrue(record['session_error'])
        partial = STREAM.replace('"cache_read_input_tokens": 20000, ', '')
        self.assertIsNone(live.parse_stream(partial)['input_total'])


class SummaryTests(unittest.TestCase):
    def test_failures_stay_in_the_denominator(self):
        records = [
            {'arm': 'off', 'task': 't', 'correct': True, 'input_total': 100, 'turns': 2},
            {'arm': 'off', 'task': 't', 'correct': False, 'input_total': None, 'turns': None, 'timed_out': True},
            {'arm': 'v4', 'task': 't', 'correct': True, 'input_total': 80, 'turns': 2},
        ]
        summary = live.summarize(records, ['off', 'v4'], ['t'])
        self.assertEqual(summary['t']['off']['runs'], 2)
        self.assertEqual(summary['t']['off']['correct'], 1)
        self.assertEqual(summary['t']['off']['with_usage'], 1)
        self.assertEqual(summary['t']['off']['median_input_total'], 100)
        self.assertEqual(summary['all']['v4']['median_input_total'], 80)

    def test_recorded_runs_aggregate_from_their_json(self):
        recorded = [dict(live.parse_stream(STREAM), arm=arm, task='find_receipt', correct=True)
                    for arm in ('off', 'v4')]
        summary = live.summarize(json.loads(json.dumps(recorded)), ['off', 'v4'], ['find_receipt'])
        self.assertEqual(summary['find_receipt']['v4']['median_tool_calls'], 2)
        self.assertEqual(summary['find_receipt']['v4']['median_cost_usd'], 0.0123)


class CheckTests(unittest.TestCase):
    def test_answers_are_checked_literally_and_as_numbers(self):
        self.assertTrue(live.check({'type': 'answer_contains', 'expected': 'src/auth.py'}, 'src/auth.py:42', Path('.')))
        self.assertFalse(live.check({'type': 'answer_contains', 'expected': 'src/auth.py'}, None, Path('.')))
        self.assertTrue(live.check({'type': 'answer_number', 'expected': 37}, 'There are 37.', Path('.')))
        self.assertFalse(live.check({'type': 'answer_number', 'expected': 37}, '370', Path('.')))

    def test_the_fixed_test_must_pass_with_its_tests_untouched(self):
        spec = live.load_tasks()['fix_test']['check']
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp) / 'repo'
            live.build('fix_test', repo)
            self.assertFalse(live.check(spec, '', repo))
            source = (repo / 'inventory.py').read_text()
            (repo / 'inventory.py').write_text(source.replace('quantity < threshold', 'quantity <= threshold'))
            self.assertTrue(live.check(spec, '', repo))
            (repo / 'test_inventory.py').write_text('')
            self.assertFalse(live.check(spec, '', repo))


class FixtureTests(unittest.TestCase):
    def test_generated_repositories_hold_exactly_the_answer(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            live.build('find_receipt', root / 'r')
            self.assertEqual((root / 'r/logs/app.log').read_text().count('RCPT-7139-QX'), 1)
            live.build('count_failures', root / 'c')
            rows = [json.loads(l) for l in (root / 'c/events.jsonl').read_text().splitlines()]
            self.assertEqual(sum(r['suite'] == 'billing' and r['status'] == 'failed' for r in rows),
                             live.load_tasks()['count_failures']['check']['expected'])
            live.build('review_diff', root / 'd')
            show = subprocess.run(['git', 'show', '--stat', 'HEAD'], cwd=root / 'd', capture_output=True, text=True).stdout
            self.assertIn('src/auth.py', show)
            self.assertIn('package-lock.json', show)


class LiveGuardTests(unittest.TestCase):
    def test_nothing_runs_without_live(self):
        with patch.object(live.subprocess, 'run', side_effect=AssertionError('called')):
            self.assertEqual(live.main(['--arm', 'off']), 2)

    def test_arms_parse_with_a_version(self):
        self.assertEqual(live.parse_arm('off')['binary'], None)
        arm = live.parse_arm('v4=/tmp/scopelet@4')
        self.assertEqual((arm['name'], arm['version']), ('v4', '4'))
        with self.assertRaises(Exception):
            live.parse_arm('v4=/tmp/scopelet')


if __name__ == '__main__':
    unittest.main()
