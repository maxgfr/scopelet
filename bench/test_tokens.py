import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import tokens

BINARY = Path(__file__).resolve().parent.parent / 'target' / 'release' / 'scopelet'


def bytes_counter(model, text):
    """A stand-in tokenizer: one token per four bytes or part of four, plus a frame of 7."""
    return 7 - (-len(text.encode()) // 4)


class TokensTests(unittest.TestCase):
    def test_refuses_to_run_without_live(self):
        self.assertEqual(tokens.main(['--out', '/nonexistent/tokens.json']), 2)

    def test_refuses_without_a_key(self):
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(tokens.main(['--live', '--out', '/nonexistent/tokens.json']), 2)

    def test_counts_are_net_and_failures_are_unknown_not_zero(self):
        memo = {}
        def flaky(model, text):
            if 'boom' in text:
                raise RuntimeError('HTTP 400: bad')
            return bytes_counter(model, text)
        self.assertEqual(tokens.counted(flaky, 'm', 'abcd' * 10, memo), (17, None))
        # Counted once: the memo answers again.
        self.assertEqual(tokens.counted(lambda m, t: 1 / 0, 'm', 'abcd' * 10, memo), (17, None))
        count, error = tokens.counted(flaky, 'm', 'boom', memo)
        self.assertIsNone(count)
        self.assertIn('400', error)

    def test_a_failed_frame_leaves_every_count_unknown(self):
        def no_frame(model, text):
            if text == tokens.BASELINE:
                raise RuntimeError('HTTP 500: down')
            return bytes_counter(model, text)
        with tempfile.TemporaryDirectory() as tmp:
            if not BINARY.exists():
                self.skipTest('needs target/release/scopelet')
            report = tokens.measure(BINARY, no_frame, ['m'], Path(tmp) / 't.json')
            counts = report['fixtures']['diagnostics']['tokens']['m']
            self.assertTrue(all(v is None for v in counts.values()), counts)
            self.assertEqual(report['summary']['m']['fixtures_counted'], 0)

    def test_summary_compares_only_fully_counted_fixtures(self):
        fixtures = {
            'a': {'tokens': {'m': {'raw': 100, 'v4': 40, 'v5': 30}}},
            'b': {'tokens': {'m': {'raw': 100, 'v4': 10, 'v5': 12}}},
            'c': {'tokens': {'m': {'raw': 100, 'v4': None, 'v5': 5}}},
        }
        summary = tokens.summarize(fixtures, 'm', ['v4', 'v5'])
        self.assertEqual(summary['fixtures_counted'], 2)
        self.assertEqual(summary['fixtures_missing'], 1)
        self.assertEqual(summary['tokens'], {'raw': 200, 'v4': 50, 'v5': 42})
        self.assertEqual(summary['vs_v4']['v5'], {'saved_tokens': 8, 'saved_percent': 16.0, 'fixtures_worse': ['b']})

    @unittest.skipUnless(BINARY.exists(), 'needs target/release/scopelet')
    def test_measure_counts_every_arm_and_never_overwrites(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / 'tokens.json'
            report = tokens.measure(BINARY, bytes_counter, ['m'], out)
            self.assertTrue(out.exists())
            for name, fixture in report['fixtures'].items():
                self.assertEqual(set(fixture['tokens']['m']), {'raw', *(a for a, _, _ in tokens.arms())}, name)
                self.assertTrue(all(fixture['facts_visible'].values()) or name in tokens.content.RECOVERY_ONLY, name)
            self.assertGreater(report['summary']['m']['vs_v4']['v5']['saved_tokens'], 0)
            # Net of the frame: the stand-in's frame is 7 tokens, `.` one more.
            self.assertEqual(report['frame_tokens'], {'m': 7})
            small = report['fixtures']['small']
            self.assertEqual(small['tokens']['m']['raw'], small['bytes']['raw'] // 4)
            with self.assertRaises(FileExistsError):
                tokens.measure(BINARY, bytes_counter, ['m'], out)


if __name__ == '__main__':
    unittest.main()
