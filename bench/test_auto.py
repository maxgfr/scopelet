import unittest
import auto
class AutoPilotTests(unittest.TestCase):
    def test_balanced_matrix_and_same_prompt(self):
        rows=auto.matrix();self.assertEqual(len(rows),60)
        for arm in auto.ARMS:self.assertEqual(sum(r['arm']==arm for r in rows),12)
        self.assertNotIn('Do not invoke Scopelet',auto.prompt('task4'))
        self.assertNotIn('/scopelet',auto.prompt('task4'))
    def test_unknown_usage_is_not_zero_and_failure_remains(self):
        cells=auto.summary([{'task':'task3','arm':'native','acceptance':{'grade':'fail'},'usage':{'usage_missing':True}},
                           {'task':'task3','arm':'default','usage':{'usage_missing':False,'logical_input_tokens':10,'output_tokens':2}}])
        self.assertIsNone(cells['task3/native']['mean_tokens']);self.assertEqual(cells['task3/native']['attempts'],1)
        self.assertIsNone(cells['task3/default']['change_vs_native_percent'])

class ExportAuditTests(unittest.TestCase):
    def test_typed_completed_events_only(self):
        import export_auto
        import json
        events = [
            {'type': 'item.started', 'item': {'type': 'command_execution', 'command': 'scopelet run --auto', 'aggregated_output': '[scopelet compact-v1 artifact:a]'}},
            {'type': 'item.completed', 'item': {'type': 'agent_message', 'text': 'scopelet run --auto'}},
            {'type': 'item.completed', 'item': {'type': 'command_execution', 'command': 'scopelet run --auto -- cat a', 'aggregated_output': '[scopelet compact-v1 artifact:a]\n.scopelet/cache/a'}},
        ]
        result = export_auto.audit('\n'.join(json.dumps(e) for e in events).encode())
        self.assertEqual(result['automatic_commands'], 1)
        self.assertEqual(result['compact_results'], 1)
        self.assertEqual(result['cache_paths_in_tool_results'], 1)

    def test_smoke_subset_does_not_expand_campaign(self):
        self.assertEqual(len(auto.matrix(1, tasks=['task4'], arms=['default', 'caveman'])), 2)

if __name__ == '__main__':
    unittest.main()
