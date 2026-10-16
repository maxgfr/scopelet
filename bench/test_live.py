import gzip
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import live

STREAM = '\n'.join(json.dumps(e) for e in [
    {'type': 'system', 'subtype': 'init'},
    {'type': 'assistant', 'message': {'model': 'claude-haiku-5-5', 'content': [{'type': 'text', 'text': 'Looking.'},
                                                                              {'type': 'tool_use', 'name': 'Bash', 'input': {}}]}},
    {'type': 'user', 'message': {'content': [{'type': 'tool_result', 'content': 'x'}]}},
    {'type': 'assistant', 'message': {'content': [{'type': 'tool_use', 'name': 'Read', 'input': {}}]}},
    {'type': 'result', 'subtype': 'success', 'is_error': False, 'num_turns': 3, 'total_cost_usd': 0.0123,
     'result': 'RCPT-7139-QX', 'usage': {'input_tokens': 12, 'cache_creation_input_tokens': 3000,
                                         'cache_read_input_tokens': 20000, 'output_tokens': 150},
     'modelUsage': {'claude-haiku-5-5': {'inputTokens': 12}, 'claude-haiku-5-5-mini': {'inputTokens': 1}}},
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

    def test_served_models_come_from_messages_and_model_usage(self):
        self.assertEqual(live.parse_stream(STREAM)['models_served'], ['claude-haiku-5-5', 'claude-haiku-5-5-mini'])
        bare = STREAM.replace('"model": "claude-haiku-5-5", ', '').replace('"modelUsage"', '"other"')
        self.assertIsNone(live.parse_stream(bare)['models_served'])

    def test_missing_usage_is_unknown_never_zero(self):
        record = live.parse_stream('not json\n')
        for key in ('input_tokens', 'input_total', 'output_tokens', 'turns', 'tool_calls', 'models_served'):
            self.assertIsNone(record[key], key)
        self.assertTrue(record['session_error'])
        partial = STREAM.replace('"cache_read_input_tokens": 20000, ', '')
        self.assertIsNone(live.parse_stream(partial)['input_total'])

    def test_views_shown_are_counted_in_the_tool_results_the_model_read(self):
        self.assertEqual(live.parse_stream(STREAM)['views_shown'], 0)
        result = lambda content: json.dumps({'type': 'user', 'message': {'content': [
            {'type': 'tool_result', 'tool_use_id': 't', 'content': content}]}})
        stream = '\n'.join([
            result('[scopelet v5 0123456789ab omitted=3; gaps are not absence]\n1-4\n x\n'),
            # Both streams of one Bash result, each a view.
            result([{'type': 'text', 'text': '[scopelet v4 0123456789abcdef complete]\nx\n'},
                    {'type': 'text', 'text': '[scopelet v5 0123456789ab repeat: identical (400 lines)]\n'}]),
            # Not views: a quoted marker, a capture note, the raw output.
            result('let header = "[scopelet v5 {id}";\n[scopelet capture_complete=false exit_code=1]\n'),
            # The hook's raw response is not what the model read.
            json.dumps({'type': 'user', 'message': {'content': 'prompt'}, 'tool_use_result': {
                'stdout': '[scopelet v5 0123456789ab complete]\n'}}),
        ])
        self.assertEqual(live.parse_stream(stream)['views_shown'], 3)


def run(arm, task, round_, correct=True, cost=0.01, input_total=100, turns=3):
    return {'arm': arm, 'task': task, 'round': round_, 'correct': correct, 'cost_usd': cost,
            'input_total': input_total, 'turns': turns}


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

    def test_cost_per_success_counts_failed_runs_and_skips_unknown_costs(self):
        rows = [run('a', 't', 0, cost=0.02), run('a', 't', 1, correct=False, cost=0.04),
                run('a', 't', 2, cost=None)]
        cps, unknown = live.cost_per_success(rows)
        self.assertAlmostEqual(cps, 0.06)
        self.assertEqual(unknown, 1)
        self.assertEqual(live.cost_per_success([run('a', 't', 0, correct=False)]), (None, 0))
        cell = live.summarize(rows, ['a'], ['t'])['all']['a']
        self.assertAlmostEqual(cell['cost_per_success'], 0.06)
        self.assertEqual(cell['cost_unknown'], 1)
        self.assertAlmostEqual(cell['total_cost_usd'], 0.06)

    def test_arms_are_paired_by_task_and_round(self):
        records = [run('v4', 't1', 0, cost=0.10, input_total=1000, turns=4),
                   run('v5', 't1', 0, cost=0.08, input_total=900, turns=4),
                   run('v4', 't2', 0, cost=0.10, input_total=None),
                   run('v5', 't2', 0, cost=0.06, input_total=700, turns=2),
                   # Unpaired: the baseline never ran t2 in round 1.
                   run('v5', 't2', 1, cost=5.0)]
        paired = live.summarize(records, ['v4', 'v5'], ['t1', 't2'], baseline='v4')['paired']['v5']
        self.assertEqual(paired['pairs'], 2)
        self.assertAlmostEqual(paired['mean_delta']['cost_usd']['value'], -0.03)
        self.assertEqual(paired['mean_delta']['input_total']['value'], -100)
        self.assertEqual(paired['mean_delta']['input_total']['pairs_unknown'], 1)
        self.assertAlmostEqual(paired['cost_per_success_delta']['value'], (0.14 - 0.20) / 2)
        self.assertEqual(paired['correct'], {'v5': 2, 'v4': 2})
        self.assertNotIn('paired', live.summarize(records, ['v4', 'v5'], ['t1'], baseline='v3'))

    def test_the_bootstrap_resamples_tasks_with_a_fixed_seed(self):
        # Every task gains 1: the interval is that point.
        same = live.bootstrap([[1], [1], [1]], lambda s: sum(g[0] for g in s) / len(s))
        self.assertEqual(same['ci95'], [1, 1])
        # Two tasks, 1 and 3: resampled means are 1, 2 or 3, with 1/4, 1/2, 1/4.
        two = live.bootstrap([[1], [3]], lambda s: sum(g[0] for g in s) / len(s))
        self.assertEqual(two['ci95'], [1, 3])
        self.assertEqual(two, live.bootstrap([[1], [3]], lambda s: sum(g[0] for g in s) / len(s)))
        undefined = live.bootstrap([[1], [None]], lambda s: None if any(g[0] is None for g in s) else 1, draws=1000)
        self.assertGreater(undefined['undefined_draws'], 600)
        self.assertEqual(undefined['ci95'], [1, 1])

    def test_paired_intervals_cover_cost_per_success(self):
        records = []
        for t in range(6):
            for r in range(3):
                records += [run('v4', f't{t}', r, cost=0.10), run('v5', f't{t}', r, cost=0.10 - 0.01 * (t + 1))]
        paired = live.paired(records, 'v5', 'v4', [f't{t}' for t in range(6)], draws=2000)
        low, high = paired['cost_per_success_delta']['ci95']
        self.assertLess(high, 0)
        self.assertLessEqual(low, paired['cost_per_success_delta']['value'])
        self.assertEqual(paired['mean_delta']['turns']['ci95'], [0, 0])


class CheckTests(unittest.TestCase):
    def test_answers_are_checked_literally_and_as_numbers(self):
        self.assertTrue(live.check({'type': 'answer_contains', 'expected': 'src/auth.py'}, 'src/auth.py:42', Path('.')))
        self.assertFalse(live.check({'type': 'answer_contains', 'expected': 'src/auth.py'}, None, Path('.')))
        self.assertTrue(live.check({'type': 'answer_number', 'expected': 37}, 'There are 37.', Path('.')))
        self.assertFalse(live.check({'type': 'answer_number', 'expected': 37}, '370', Path('.')))
        near = {'type': 'answer_contains', 'expected': 'src/a.rs:8', 'forbidden': ['src/b.rs']}
        self.assertTrue(live.check(near, 'src/a.rs:8', Path('.')))
        self.assertFalse(live.check(near, 'src/a.rs:8 or src/b.rs:8', Path('.')))
        # A `path:line only` answer is judged on its first nonempty line: an
        # explanation that rejects the near miss does not fail it.
        first = {**near, 'first_line': True}
        explained = '\nsrc/a.rs:8\n\nThe other candidate, src/b.rs, is excluded.'
        self.assertTrue(live.check(first, explained, Path('.')))
        self.assertFalse(live.check(near, explained, Path('.')))
        self.assertFalse(live.check(first, 'src/b.rs:8\nnot src/a.rs:8', Path('.')))

    def test_the_fixed_test_must_pass_with_its_tests_untouched(self):
        spec = live.load_tasks()['fix_test']['check']
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp) / 'repo'
            live.build('fix_test', repo)
            self.assertFalse(live.check(spec, '', repo, 'fix_test'))
            source = (repo / 'inventory.py').read_text()
            (repo / 'inventory.py').write_text(source.replace('quantity < threshold', 'quantity <= threshold'))
            self.assertTrue(live.check(spec, '', repo, 'fix_test'))
            (repo / 'test_inventory.py').write_text('')
            self.assertFalse(live.check(spec, '', repo, 'fix_test'))


def built(task):
    temp = tempfile.TemporaryDirectory()
    root = Path(temp.name) / task
    live.build(task, root)
    return temp, root


class NewTaskTests(unittest.TestCase):
    """Each generated task has one right answer, which its check accepts and
    a near miss does not."""

    def answer(self, task, right, wrong):
        spec = live.load_tasks()[task]['check']
        self.assertTrue(live.check(spec, right, Path('.')), right)
        self.assertFalse(live.check(spec, wrong, Path('.')), wrong)

    def fixed(self, task, path, old, new, tests):
        spec = live.load_tasks()[task]['check']
        temp, root = built(task)
        with temp:
            self.assertFalse(live.check(spec, '', root, task))
            source = (root / path).read_text()
            self.assertEqual(source.count(old), 1)
            (root / path).write_text(source.replace(old, new))
            self.assertTrue(live.check(spec, '', root, task))
            (root / tests).write_text((root / tests).read_text() + '\n# edited\n')
            self.assertFalse(live.check(spec, '', root, task))

    def test_every_task_builds_deterministically(self):
        for task in live.load_tasks():
            first, a = built(task)
            second, b = built(task)
            with first, second:
                files = lambda root: {str(p.relative_to(root)): p.read_bytes() for p in sorted(root.rglob('*'))
                                      if p.is_file() and '.git' not in p.parts}
                self.assertEqual(files(a), files(b), task)

    def test_search_symbol(self):
        temp, root = built('search_symbol')
        with temp:
            self.assertEqual(len(list(root.rglob('*.py'))), 300)
            found = [f'{path.relative_to(root)}:{n}' for path in sorted(root.rglob('*.py'))
                     for n, line in enumerate(path.read_text().splitlines(), 1) if line.startswith('def reconcile_ledger(')]
            self.assertEqual(found, ['src/pkg_g/mod_bih.py:15'])
            calls = sum(p.read_text().count('reconcile_ledger(') for p in root.rglob('*.py'))
            self.assertGreater(calls, 1800)
        self.answer('search_symbol', './src/pkg_g/mod_bih.py:15', 'src/pkg_a/mod_ec.py:9')

    def test_git_log_find(self):
        temp, root = built('git_log_find')
        with temp:
            introduced = subprocess.run(['git', 'log', '--format=%s', '-S', 'price + discount'], cwd=root,
                                        capture_output=True, text=True).stdout
            self.assertEqual(introduced.splitlines(), ['Tidy pricing helpers'])
            subjects = subprocess.run(['git', 'log', '--format=%s'], cwd=root, capture_output=True, text=True).stdout.splitlines()
            self.assertEqual(len(subjects), len(set(subjects)))
        self.answer('git_log_find', 'Tidy pricing helpers', 'Tune rates for region bh')

    def test_listing_artifacts(self):
        temp, root = built('listing_artifacts')
        with temp:
            artifacts = {'node_modules', '.venv', 'build', 'target', '__pycache__'}
            every = [p for p in root.rglob('*.py')]
            own = [p for p in every if not artifacts & set(p.relative_to(root).parts)]
            self.assertEqual(len(own), live.load_tasks()['listing_artifacts']['check']['expected'])
            self.assertGreater(len(every), 2 * len(own))
        self.answer('listing_artifacts', '52', str(len(every)))

    def test_large_file_fix(self):
        temp, root = built('large_file_fix')
        with temp:
            self.assertGreater((root / 'billing.py').stat().st_size, 40 * 1024)
        self.fixed('large_file_fix', 'billing.py', 'max(fee, amount * 0.25)', 'min(fee, amount * 0.25)', 'test_billing.py')

    def test_stack_trace(self):
        temp, root = built('stack_trace')
        with temp:
            log = (root / 'logs/service.log').read_text().splitlines()
            causes = [i for i, line in enumerate(log) if line.startswith('Caused by:')]
            self.assertIn('LedgerService.java:214', log[causes[-1] + 1])
            self.assertEqual(sum('LedgerService.java:214' in line for line in log), 1)
            self.assertGreater(sum('NullPointerException' in line for line in log), 2)
        self.answer('stack_trace', 'LedgerService.java:214', 'LedgerService.java:301')

    def test_json_api(self):
        temp, root = built('json_api')
        with temp:
            text = (root / 'api/issues.json').read_text()
            self.assertEqual(text.count('\n'), 0)
            issues = json.loads(text)
            self.assertEqual(len(issues), 200)
            names = lambda i: [l['name'] for l in i['labels']]
            exact = sum(i['state'] == 'open' and 'regression' in names(i) for i in issues)
            loose = sum(i['state'] == 'open' and 'regression' in json.dumps(i) for i in issues)
            self.assertEqual(exact, live.load_tasks()['json_api']['check']['expected'])
            self.assertNotEqual(exact, loose)
        self.answer('json_api', str(exact), str(loose))

    def test_build_warnings(self):
        temp, root = built('build_warnings')
        with temp:
            build = subprocess.run(['python3', 'build.py'], cwd=root, capture_output=True, text=True)
            self.assertEqual(build.returncode, 1)
            lines = build.stderr.splitlines()
            self.assertEqual(sum(l.startswith('warning:') for l in lines), 2000)
            errors = [i for i, l in enumerate(lines) if l.startswith('error[')]
            self.assertEqual(len(errors), 1)
            self.assertIn('src/ledger/settle.rs', lines[errors[0] + 1])
            self.assertEqual(sum('src/ledger/settle.rs' in l for l in lines), 1)
        self.answer('build_warnings', 'src/ledger/settle.rs', 'src/ledger/settled.rs')

    def test_pytest_many(self):
        temp, root = built('pytest_many')
        with temp:
            tests = subprocess.run(['python3', '-m', 'unittest', 'test_rates'], cwd=root, capture_output=True, text=True)
            self.assertIn('Ran 600 tests', tests.stderr)
            self.assertIn('FAILED (failures=2)', tests.stderr)
        self.fixed('pytest_many', 'rates.py', '    if units <= 1000:\n        return 0.10\n    if units <= 5000:',
                   '    if units < 1000:\n        return 0.10\n    if units < 5000:', 'test_rates.py')

    def output(self, root, command):
        """What the command a task imposes prints, both streams."""
        run = subprocess.run(command, cwd=root, capture_output=True, text=True)
        return run, run.stdout + run.stderr

    def assert_view_sized(self, text):
        # Above every exact-size threshold, below the size Claude Code saves
        # to a file: the model is shown a view inline.
        self.assertGreater(len(text.encode()), 2 * 1024)
        self.assertLess(len(text.encode()), 30 * 1024)

    def test_build_warnings_medium(self):
        temp, root = built('build_warnings_medium')
        with temp:
            build, text = self.output(root, ['python3', 'build.py'])
            # Exit 0: Claude Code hands a failed command's result to no hook
            # that can replace it, so only a successful one shows a view.
            self.assertEqual(build.returncode, 0)
            self.assert_view_sized(text)
            lines = text.splitlines()
            self.assertEqual(sum(l.startswith('warning:') for l in lines), 300)
            errors = [i for i, l in enumerate(lines) if l.startswith('error[')]
            self.assertEqual(len(errors), 1)
            self.assertEqual(lines[errors[0] + 1], '  --> src/ledger/settle.rs:88:17')
            self.assertTrue(any('src/ledger/settled.rs' in l for l in lines))
        self.answer('build_warnings_medium', 'src/ledger/settle.rs:88', 'src/ledger/settled.rs:88')
        self.answer('build_warnings_medium', 'src/ledger/settle.rs:88', 'src/mod_bc.rs:88')

    def test_test_report_medium(self):
        temp, root = built('test_report_medium')
        with temp:
            tests, text = self.output(root, ['python3', '-m', 'unittest', '-v', 'test_shipping'])
            self.assert_view_sized(text)
            self.assertIn('Ran 150 tests', text)
            self.assertIn('FAILED (failures=1)', text)
            self.assertIn('FAIL: test_grams_0500', text)
        self.fixed('test_report_medium', 'shipping.py', 'if grams < 500:', 'if grams <= 500:', 'test_shipping.py')

    def test_grep_context(self):
        temp, root = built('grep_context')
        with temp:
            _, text = self.output(root, ['grep', '-rn', '-C2', 'charge_card', 'src'])
            self.assert_view_sized(text)
            matches = [l for l in text.splitlines() if re.match(r'src/m_\w+\.py:\d+:', l)]
            self.assertEqual(len(matches), 200)
            lines = (root / 'src/m_g.py').read_text().splitlines()
            self.assertEqual(lines[54], '    charge_card(o, t)')
            self.assertEqual(lines[53], '    t = o.s - o.refund')
            decoy = (root / 'src/m_d.py').read_text().splitlines()
            self.assertEqual((decoy[29], decoy[30]), ('    t = o.s - o.refund', '    charge_card_legacy(o, t)'))
            self.assertEqual(sum('refund' in p.read_text() for p in (root / 'src').glob('*.py')), 2)
        self.answer('grep_context', 'src/m_g.py:55', 'src/m_d.py:31')
        self.answer('grep_context', 'src/m_g.py:55', 'src/m_g.py:54')

    def test_diff_medium(self):
        temp, root = built('diff_medium')
        with temp:
            _, text = self.output(root, ['git', 'diff'])
            self.assert_view_sized(text)
            self.assertIn('+    return used <= limit', text)
            self.assertIn('+    if limit > x:', text)
            changed = re.findall(r'^diff --git a/(\S+)', text, re.M)
            self.assertEqual(len(changed), 22)
        self.answer('diff_medium', 'src/quota.py:3', 'src/window.py:3')


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

    def test_sixteen_tasks_each_with_a_builder(self):
        tasks = live.load_tasks()
        self.assertEqual(len(tasks), 16)
        with tempfile.TemporaryDirectory() as temp:
            for task in tasks:
                live.build(task, Path(temp) / task)


class SessionTests(unittest.TestCase):
    def test_a_session_keeps_its_raw_transcript_and_never_overwrites_it(self):
        def fake(args, **kwargs):
            if args[0] == 'claude':
                return subprocess.CompletedProcess(args, 0, STREAM, '')
            return subprocess.run(args, **kwargs)
        spec = live.load_tasks()['find_receipt']
        with tempfile.TemporaryDirectory() as temp, patch.object(live.subprocess, 'run', side_effect=fake):
            path = live.transcript_path(Path(temp), 0, 'find_receipt', 'off')
            record = live.session(live.parse_arm('off'), 'find_receipt', spec, 'haiku', 60, False, path)
            self.assertTrue(record['correct'])
            self.assertEqual(path.name, '0-find_receipt-off.jsonl.gz')
            with gzip.open(path, 'rt') as f:
                self.assertEqual(f.read(), STREAM)
            with self.assertRaises(FileExistsError):
                live.session(live.parse_arm('off'), 'find_receipt', spec, 'haiku', 60, False, path)

    def test_an_arm_installs_its_binary_with_no_inherited_scopelet_setting(self):
        calls = []
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {'SCOPELET_EVENTS': '0'}), \
                patch.object(live.subprocess, 'run', side_effect=lambda args, **kw: calls.append((args, kw['env']))):
            env = live.install(live.parse_arm('single=/bin/echo'), Path(temp))
            self.assertEqual(calls[0][0], ['/bin/echo', 'install', '--agent', 'claude'])
            self.assertEqual({k for k in env if k.startswith('SCOPELET_')}, {'SCOPELET_CONFIG_DIR', 'SCOPELET_CACHE_DIR'})
        with tempfile.TemporaryDirectory() as temp, patch.object(live.subprocess, 'run', side_effect=AssertionError):
            env = live.install(live.parse_arm('off'), Path(temp))
            self.assertFalse(any(k.startswith('SCOPELET_') for k in env))


class LiveGuardTests(unittest.TestCase):
    def test_nothing_runs_without_live(self):
        with patch.object(live.subprocess, 'run', side_effect=AssertionError('called')):
            self.assertEqual(live.main(['--arm', 'off']), 2)

    def test_arms_are_off_or_a_name_and_a_binary(self):
        self.assertEqual(live.parse_arm('off'), {'name': 'off', 'binary': None})
        self.assertEqual(live.parse_arm('v4old=/tmp/scopelet-0.11.1'),
                         {'name': 'v4old', 'binary': Path('/tmp/scopelet-0.11.1').resolve()})
        # Versions and grammar parts are gone: a binary shows its own format.
        for bad in ('v4=/tmp/scopelet@4', 'v5g=/tmp/scopelet@5:labels', 'single=', '=/tmp/scopelet', 'a b=/x'):
            with self.assertRaises(Exception):
                live.parse_arm(bad)


if __name__ == '__main__':
    unittest.main()
