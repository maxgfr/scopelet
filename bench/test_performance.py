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

if __name__=='__main__':unittest.main()
