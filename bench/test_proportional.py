import json
from pathlib import Path
import tempfile
import unittest
import proportional as p
import run as base
import export_proportional


class ProportionalTests(unittest.TestCase):
    def test_matrix_is_bounded_balanced_and_reproducible(self):
        cases=p.matrix(2)
        self.assertEqual(len(cases),18)
        self.assertEqual(cases,p.matrix(2))
        self.assertEqual(len({(c['task'],c['arm'],c['repetition']) for c in cases}),18)

    def test_grades_original_correct_and_semantically_wrong_small_edits(self):
        for task in p.TASKS:
            with self.subTest(task=task),tempfile.TemporaryDirectory() as d:
                root=Path(d);p.fixture(root,task);hashes=base.fixture_hashes(root)
                self.assertFalse(p.grade(root,task,hashes))
                if task=='python':
                    path=root/'src/helpers.py'
                    path.write_text("def normalize_identifier(value):\n    return '_'.join(value.strip().lower().split())\n")
                elif task=='javascript':
                    (root/'options.mjs').write_text('export function option(value, fallback) { return value ?? fallback; }\n')
                else:
                    data=json.loads((root/'config.json').read_text());data['retries']=0
                    (root/'config.json').write_text(json.dumps(data))
                self.assertTrue(p.grade(root,task,hashes))
                if task=='json':
                    data['quota']=False;(root/'config.json').write_text(json.dumps(data))
                    self.assertFalse(p.grade(root,task,hashes))
                if task=='javascript':
                    (root/'check.mjs').write_text("console.log('forged success');\n")
                    self.assertFalse(p.grade(root,task,hashes))

    def test_missing_usage_and_failures_remain_in_summary(self):
        cell=p.summarize([{'task':'python','arm':'candidate','passed':False}])['python/candidate']
        self.assertEqual(cell,{'attempts':1,'passes':0,'tokens':[],'median_tokens':None,'mean_tokens':None,'missing_usage':1})

    def test_export_keeps_failure_without_raw_private_invocation(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            (root/'report.json').write_text(json.dumps({'model_requested':'gpt-5.6-luna','runs':[{'task':'python','arm':'candidate','repetition':1,'status':'harness_error','error':'/private/operator/path','command':['private command'],'passed':False}]}))
            result=export_proportional.export(root)
            self.assertFalse(result['runs'][0]['passed'])
            self.assertEqual(result['summary']['python/candidate']['attempts'],1)
            self.assertNotIn('/private/operator',json.dumps(result))
            self.assertNotIn('command',result['runs'][0])
