from argparse import Namespace
import argparse
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import performance


def fake_fixture(path, stress):
    path.mkdir(); source = path / 'small'; source.write_bytes(b'fixture')
    return {'small': (['compress'], source)}


def sample(raw):
    return {'seconds': 0.01, 'rss_bytes': 1, 'exit_code': 0, 'stdout_sha256': performance.sha(raw), 'output_bytes': len(raw)}, raw


class SummaryTests(unittest.TestCase):
    def test_offline_summary_keeps_missing_rss_and_exit_failure(self):
        value = performance.summarize([{'seconds': .01, 'rss_bytes': None, 'exit_code': 7}])
        self.assertEqual(value['failures'], 1)
        self.assertIsNone(value['median_rss_bytes'])

    def test_arm_parsing_accepts_named_and_bare_paths(self):
        self.assertEqual(performance.parse_arm('release=/tmp/a'), ('release', Path('/tmp/a')))
        self.assertEqual(performance.parse_arm('/tmp/scopelet'), ('scopelet', Path('/tmp/scopelet')))
        with self.assertRaises(argparse.ArgumentTypeError):
            performance.parse_arm('=path')


class RunTests(unittest.TestCase):
    def run_arms(self, arms, outputs):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'binary'; binary.write_bytes(b'frozen')
            options = Namespace(out=root / 'out', binary=[(name, binary) for name in arms], warmups=0, repetitions=2,
                                stress=False, cases='small')
            seen = []
            def measure(binary, args, source, cache):
                seen.append(binary.name)
                return sample(outputs[binary.name])
            with patch.object(performance, 'fixtures', side_effect=fake_fixture), patch.object(performance, 'measure', side_effect=measure), \
                    patch.object(performance, 'binary_version', return_value='scopelet 0.0.0'), patch('builtins.print'):
                return performance.run(options), seen

    def test_single_arm_measures_every_case_and_state(self):
        report, seen = self.run_arms(['release'], {'release': b'view'})
        self.assertEqual(report['arms'], ['release'])
        self.assertEqual(report['output_mismatches'], [])
        self.assertEqual(sorted(report['cases']), ['small/cold', 'small/warm'])
        self.assertEqual(report['cases']['small/cold']['release']['summary']['samples'], 2)
        self.assertEqual(report['binary_version'], {'release': 'scopelet 0.0.0'})

    def test_arms_alternate_order_and_identical_output_passes(self):
        report, seen = self.run_arms(['baseline', 'candidate'], {'baseline': b'view', 'candidate': b'view'})
        self.assertEqual(seen[:4], ['baseline', 'candidate', 'candidate', 'baseline'])
        self.assertEqual(report['output_mismatches'], [])

    def test_output_mismatch_between_arms_is_recorded(self):
        report, _ = self.run_arms(['baseline', 'candidate'], {'baseline': b'view', 'candidate': b'other'})
        self.assertEqual(len(report['output_mismatches']), 4)
        self.assertEqual(report['output_mismatches'][0]['case'], 'small/cold')
        self.assertEqual(sorted(report['output_mismatches'][0]['stdout_sha256']), ['baseline', 'candidate'])

    def test_output_mismatch_fails_command(self):
        with patch('sys.argv', ['performance.py', '--binary', 'a=/a', '--out', 'unused']), \
                patch.object(performance, 'run', return_value={'output_mismatches': [{'case': 'small'}], 'cases': {}}):
            with self.assertRaises(SystemExit) as error: performance.main()
            self.assertEqual(error.exception.code, 1)

    def test_duplicate_arm_names_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'binary'; binary.write_bytes(b'frozen')
            options = Namespace(out=root / 'out', binary=[('a', binary), ('a', binary)], warmups=0, repetitions=1, stress=False, cases=None)
            with self.assertRaises(ValueError):
                performance.run(options)


if __name__ == '__main__':
    unittest.main()
