import json
from pathlib import Path
import tempfile
import unittest
import current
import export_current
import run


class ExportCurrentTests(unittest.TestCase):
    def report(self):
        return dict(schema_version=1, models=current.MODELS, seed=1, max_attempts=60,
                    binary_sha256={}, provenance={}, host_versions={}, harness_sha256={}, unmeasured={})

    def test_corrupt_transcript_is_rejected_and_private_text_is_not_exported(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            row = {**current.matrix('main')[0], 'stdout_sha256': run.sha256_bytes(b'original'),
                   'acceptance': {'stdout': 'private text /Users/example', 'passed': True}, 'passed': True}
            report = {**self.report(), 'runs': [row], 'private_paths': {'scopelet': '/Users/example'}}
            (root / 'report.json').write_text(json.dumps(report))
            raw = root / 'runs' / current.case_id(row) / 'stdout.raw'
            raw.parent.mkdir(parents=True)
            raw.write_bytes(b'changed')
            with self.assertRaises(ValueError): export_current.export(root)
            raw.write_bytes(b'original')
            result = export_current.export(root)
            self.assertNotIn('/Users/example', json.dumps(result))
            self.assertNotIn('private text', json.dumps(result))

    def test_recovered_usage_preserves_harness_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            row = {**current.matrix('main')[0], 'status': 'harness_error', 'passed': False,
                   'recovered_evidence': {'usage': {'logical_input_tokens': 100, 'output_tokens': 5},
                                          'stdout_sha256': run.sha256_bytes(b'raw'), 'limitation': 'grade unavailable'}}
            (root / 'report.json').write_text(json.dumps({**self.report(), 'runs': [row]}))
            raw = root / 'runs' / current.case_id(row) / 'stdout.raw'
            raw.parent.mkdir(parents=True); raw.write_bytes(b'raw')
            result = export_current.export(root)
            self.assertFalse(result['runs'][0]['passed'])
            self.assertEqual(next(iter(result['summary'].values()))['mean_tokens'], 105)
            self.assertTrue(result['runs'][0]['usage_recovered_from_transcript'])
