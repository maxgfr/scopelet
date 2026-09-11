import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch
import content


class ContentTests(unittest.TestCase):
    def test_recovery_uses_source_blob_not_serialized_artifact(self):
        original = b'original\r\n'
        artifact = 'artifact:' + 'a' * 64
        blob = 'blob:' + 'b' * 64
        manifest = json.dumps({'snapshots': [{'blob': blob}]}).encode()
        with patch.object(content.subprocess, 'run', side_effect=[subprocess.CompletedProcess([], 0, manifest), subprocess.CompletedProcess([], 0, original)]) as run:
            exact, recovered = content.recovery(Path('/bin/scopelet'), Path('/cache'), artifact.encode(), original)
        self.assertTrue(exact)
        self.assertEqual(recovered, original)
        self.assertEqual(run.call_args_list[-1].args[0][-2:], [blob, '--raw'])

    def test_read_failure_cannot_claim_roundtrip(self):
        with patch.object(content.subprocess, 'run', return_value=subprocess.CompletedProcess([], 2, b'')):
            exact, recovered = content.recovery(Path('/bin/scopelet'), Path('/cache'), ('artifact:' + 'a' * 64).encode(), b'original')
        self.assertFalse(exact)
        self.assertIsNone(recovered)

    def test_untouched_output_counts_as_roundtrip_without_an_artifact(self):
        with patch.object(content.subprocess, 'run') as run:
            exact, recovered = content.recovery(Path('/bin/scopelet'), Path('/cache'), b'same', b'same')
        self.assertTrue(exact)
        self.assertIsNone(recovered)
        run.assert_not_called()

    def test_only_the_giant_line_is_expected_after_recovery(self):
        self.assertEqual(content.RECOVERY_ONLY, {'giant_unicode_line'})
        self.assertTrue(content.RECOVERY_ONLY <= set(content.fixtures()))

    def test_failures_name_every_lost_fact_exit_and_roundtrip(self):
        cell = {'failures': 0, 'original_byte_roundtrip_verified': True, 'facts_visible': [True], 'facts_available_after_recovery': [True]}
        report = {'cases': {
            'ok': {'facts_expected': 'in view', 'states': {'cold': cell}},
            'hidden': {'facts_expected': 'in view', 'states': {'cold': {**cell, 'facts_visible': [False]}}},
            'after': {'facts_expected': 'after recovery', 'states': {'cold': {**cell, 'facts_visible': [False]}}},
            'broken': {'facts_expected': 'in view', 'states': {'warm': {**cell, 'failures': 2, 'original_byte_roundtrip_verified': False,
                                                                        'facts_available_after_recovery': [False]}}},
        }}
        problems = content.failures(report)
        self.assertEqual([p.split(':')[0] for p in problems], ['hidden/cold', 'broken/warm', 'broken/warm', 'broken/warm'])
        self.assertIn('non-zero exit', problems[1])

    def test_summary_keeps_exit_failures(self):
        value = content.summarize([{'seconds': .5, 'exit_code': 7}, {'seconds': .1, 'exit_code': 0}])
        self.assertEqual(value['failures'], 1)
        self.assertEqual(value['median_seconds'], .3)
        self.assertEqual(value['p95_seconds'], .5)


if __name__ == '__main__':
    unittest.main()
