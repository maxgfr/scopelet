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

    def test_only_the_giant_line_is_expected_after_recovery(self):
        self.assertEqual(content.RECOVERY_ONLY, {'giant_unicode_line'})
        self.assertTrue(content.RECOVERY_ONLY <= set(content.fixtures()))
