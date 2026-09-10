import json
from pathlib import Path
import tempfile
import unittest
import sqlite3
from unittest.mock import patch

import current


class CurrentTests(unittest.TestCase):
    def test_postprocessing_error_keeps_usage_without_inventing_grade(self):
        with tempfile.TemporaryDirectory() as temp:
            destination = Path(temp)
            case = {'phase': 'main', 'agent': 'codex', 'arm': 'rtk', 'task': 'task4', 'repetition': 1}
            (destination / 'stdout.raw').write_text(json.dumps({'type': 'turn.completed', 'usage': {'input_tokens': 100, 'output_tokens': 10}}))
            row = current.recover_attempt_error(case, destination, RuntimeError())
            self.assertEqual(current.session_tokens(row), 110)
            self.assertFalse(row['passed'])
            self.assertEqual(row['status'], 'harness_error')
            (destination / 'result.json').write_text(json.dumps({**case, 'passed': True, 'acceptance': {'passed': True}}))
            row = current.recover_attempt_error(case, destination, OSError())
            self.assertTrue(row['passed'])
            self.assertEqual(row['archive_error'], 'OSError')

    def test_optional_rtk_telemetry_failure_does_not_lose_session(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'rtk.db'
            path.touch()
            with patch.object(current.sqlite3, 'connect', side_effect=sqlite3.OperationalError('unavailable')):
                self.assertEqual(current.rtk_counts(path), {'unavailable': 'OperationalError'})

    def test_rtk_telemetry_escapes_uri_characters_in_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'rtk?#.db'
            with sqlite3.connect(path) as db:
                db.execute('CREATE TABLE commands (id INTEGER)')
                db.execute('INSERT INTO commands VALUES (1)')
            self.assertEqual(current.rtk_counts(path), {'commands': 1})

    def test_budget_counts_interrupted_attempts_and_rejects_duplicates(self):
        report = {'runs': []}
        case = current.matrix('main')[0]
        current.reserve(report, case, 1)
        self.assertEqual(report['runs'][0]['status'], 'started')
        with self.assertRaises(ValueError):
            current.reserve(report, current.matrix('main')[1], 1)
        with self.assertRaises(ValueError):
            current.reserve(report, case, 60)

    def test_matrix_is_balanced_deterministic_and_within_shared_budget(self):
        main = current.matrix('main')
        self.assertEqual(main, current.matrix('main'))
        self.assertEqual(len({current.case_id(c) for c in main}), 48)
        self.assertEqual(len(current.matrix('preflight')) + len(main) +
                         len(current.matrix('followup', dict.fromkeys(current.MODELS, 'task4'))), 60)

    def test_incomplete_usage_and_failed_runs_remain_visible(self):
        rows = [{**current.matrix('main')[0], 'passed': False, 'usage':
                 {'logical_input_tokens': None, 'output_tokens': 10, 'usage_missing': False}}]
        cell = next(iter(current.summarize(rows).values()))
        self.assertEqual(cell['missing_usage'], 1)
        self.assertEqual(cell['passes'], 0)
        self.assertIsNone(cell['mean_tokens'])
        self.assertIsNone(cell['cost_per_pass_usd_reported'])

    def test_cost_per_success_keeps_failed_attempt_spend(self):
        case = current.matrix('main')[0]
        rows = [{**case, 'passed': passed, 'usage': {'logical_input_tokens': 100,
                 'output_tokens': 10, 'total_cost_usd_reported': 1}} for passed in (True, False)]
        cell = next(iter(current.summarize(rows).values()))
        self.assertEqual(cell['cost_per_pass_usd_reported'], 2)
        self.assertEqual(cell['attempts'], 2)

    def test_dry_run_does_not_create_output_or_launch_processes(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / 'missing'
            with patch('sys.argv', ['current.py', '--out', str(out)]), patch.object(current, 'run_case') as invoke, patch('builtins.print'):
                current.main()
            invoke.assert_not_called()
            self.assertFalse(out.exists())

    def test_claude_scopelet_installs_in_project_and_loads_hooks_once(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            state = workspace / 'state'
            paths = {'scopelet': Path('/fixture/scopelet')}
            env = {'PATH': '/usr/bin', 'CLAUDE_CONFIG_DIR': '/original/auth'}
            with patch.object(current.subprocess, 'run') as install:
                command = current.treatment_command({'agent': 'claude', 'arm': 'scopelet'}, workspace, state, env, paths)
            self.assertEqual(install.call_args.kwargs['env']['CLAUDE_CONFIG_DIR'], str(workspace / '.claude'))
            self.assertEqual(env['CLAUDE_CONFIG_DIR'], '/original/auth')
            self.assertEqual(command[command.index('--setting-sources') + 1], '')
            self.assertIn('--disable-slash-commands', command)
