import json
from pathlib import Path
import tempfile
import unittest
import performance
import performance_live as live
import run as base


class PerformanceTests(unittest.TestCase):
    def test_live_matrix_has_exactly_thirty_unique_balanced_cells(self):
        matrix=live.matrix()
        self.assertEqual(len(matrix),30)
        self.assertEqual(matrix,live.matrix())
        self.assertEqual(len({(c['task'],c['arm'],c['repetition']) for c in matrix}),30)
        self.assertEqual(live.MODEL,'gpt-5.6-luna')

    def test_unknown_usage_is_not_zero_and_failures_are_retained(self):
        cells=live.summarize([{'task':'small','arm':'candidate','passed':False}])
        self.assertEqual(cells['small/candidate']['attempts'],1)
        self.assertEqual(cells['small/candidate']['missing_usage'],1)
        self.assertIsNone(cells['small/candidate']['mean_tokens'])
        self.assertEqual(cells['small/candidate']['passes'],0)

    def test_recovery_grader_checks_exact_types_and_immutable_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);live.fixture(root,'recovery');hashes=base.fixture_hashes(root)
            self.assertFalse(live.grade(root,'recovery',hashes))
            answer=root/'answer.json';answer.write_text(json.dumps({'amount':47,'currency':'EUR'}))
            self.assertTrue(live.grade(root,'recovery',hashes))
            answer.write_text(json.dumps({'amount':47.0,'currency':'EUR'}))
            self.assertFalse(live.grade(root,'recovery',hashes))
            answer.write_text(json.dumps({'amount':47,'currency':'EUR'}));(root/'evidence.txt').write_text('forged')
            self.assertFalse(live.grade(root,'recovery',hashes))

    def test_diagnostics_grader_accepts_fix_but_rejects_modified_checks(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);live.fixture(root,'diagnostics');hashes=base.fixture_hashes(root)
            self.assertFalse(live.grade(root,'diagnostics',hashes))
            (root/'src/worker.py').write_text('def worker_count(items, limit=None):\n    return list(items) if limit is None else list(items[:limit])\n')
            self.assertTrue(live.grade(root,'diagnostics',hashes))
            (root/'checks.py').write_text('print("passed")')
            self.assertFalse(live.grade(root,'diagnostics',hashes))

    def test_offline_summary_keeps_missing_rss_and_exit_failure(self):
        value=performance.summarize([{'seconds':.01,'rss_bytes':None,'exit_code':7}])
        self.assertEqual(value['failures'],1)
        self.assertIsNone(value['median_rss_bytes'])


class RolloutTests(unittest.TestCase):
    def test_incomplete_or_failed_campaign_cannot_enable_default(self):
        from export_performance import rollout
        rows=[{'task':task,'arm':arm,'passed':True,'exit_code':0,'usage':{'usage_missing':False,'logical_input_tokens':tokens,'output_tokens':1}} for task in live.TASKS for arm,tokens in [('baseline',100),('candidate',70)]]
        self.assertFalse(rollout(rows,False)['eligible_for_v2_default'])
        self.assertTrue(rollout(rows,True)['eligible_for_v2_default'])
        rows[-1]['passed']=False
        self.assertFalse(rollout(rows,True)['eligible_for_v2_default'])


class VersionComparisonTests(unittest.TestCase):
    def test_v2_baseline_compares_v2_output_and_retains_v1_measurements(self):
        from argparse import Namespace
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'binary'; binary.write_bytes(b'frozen')
            options = Namespace(out=root / 'out', baseline=binary, candidate=binary, baseline_version=2,
                                warmups=0, repetitions=1, stress=False, cases='small')
            def fixture(path, stress):
                path.mkdir(); source = path / 'small'; source.write_bytes(b'fixture')
                return {'small': (['compress'], source)}
            def measure(binary, args, source, cache, version):
                raw = str(version).encode()
                return {'seconds': 0.01, 'rss_bytes': 1, 'exit_code': 0, 'stdout_sha256': performance.sha(raw), 'output_bytes': len(raw)}, raw
            with patch.object(performance, 'fixtures', side_effect=fixture), patch.object(performance, 'measure', side_effect=measure), patch('builtins.print'):
                report = performance.run(options)
            self.assertEqual(report['comparison_arm'], 'candidate_v2')
            self.assertEqual(report['output_mismatches'], [])
            self.assertIn('candidate', report['cases']['small/cold'])

    def test_candidate_version_is_measured_only_by_the_candidate_arm(self):
        from argparse import Namespace
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'binary'; binary.write_bytes(b'frozen')
            options = Namespace(out=root / 'out', baseline=binary, candidate=binary, baseline_version=2, candidate_version=3,
                                warmups=0, repetitions=1, stress=False, cases='small')
            seen = []
            def fixture(path, stress):
                path.mkdir(); source = path / 'small'; source.write_bytes(b'fixture')
                return {'small': (['compress'], source)}
            def measure(binary, args, source, cache, version):
                seen.append(version); raw = str(version).encode()
                return {'seconds': 0.01, 'rss_bytes': 1, 'exit_code': 0, 'stdout_sha256': performance.sha(raw), 'output_bytes': len(raw)}, raw
            with patch.object(performance, 'fixtures', side_effect=fixture), patch.object(performance, 'measure', side_effect=measure), patch('builtins.print'):
                report = performance.run(options)
            self.assertEqual(sorted(seen), [2, 2, 2, 2, 3, 3])
            self.assertEqual(report['candidate_compact_version'], 3)
            self.assertEqual(report['output_mismatches'], [])

    def test_v2_output_mismatch_fails_command(self):
        from unittest.mock import patch
        with patch('sys.argv', ['performance.py', '--baseline', 'a', '--candidate', 'b', '--out', 'unused']), \
                patch.object(performance, 'run', return_value={'output_mismatches': [{'case': 'small'}], 'v1_mismatches': [], 'cases': {}}):
            with self.assertRaises(SystemExit) as error: performance.main()
            self.assertEqual(error.exception.code, 1)

if __name__=='__main__':unittest.main()
