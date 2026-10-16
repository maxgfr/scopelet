import gzip
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import nap

BINARY = Path(__file__).resolve().parent.parent / 'target' / 'release' / 'scopelet'


def build_output():
    lines = []
    for i in range(400):
        lines += [f'warning: unused variable `x_{nap.live.word(i)}`', f'  --> src/mod_{nap.live.word(i % 40)}.rs:{i % 300 + 1}:9']
        if i == 311:
            lines += ['error[E0308]: mismatched types', '  --> src/ledger/settle.rs:88:17']
    return '\n'.join(lines) + '\n'


def transcript(stdout):
    """A stream-json session: one large build output, then a search, then an answer."""
    def assistant(mid, block):
        return {'type': 'assistant', 'message': {'id': mid, 'model': 'claude-haiku-5-5', 'content': [block]}}

    def result(tid, out):
        return {'type': 'user', 'message': {'role': 'user', 'content': [{'tool_use_id': tid, 'type': 'tool_result', 'content': out}]},
                'tool_use_result': {'stdout': out, 'stderr': '', 'interrupted': False, 'isImage': False}}
    return [
        {'type': 'system', 'subtype': 'init', 'tools': ['Bash']},
        assistant('m1', {'type': 'thinking', 'thinking': '', 'signature': 'sig'}),
        assistant('m1', {'type': 'text', 'text': 'Running the build.'}),
        assistant('m1', {'type': 'tool_use', 'id': 't1', 'name': 'Bash', 'input': {'command': 'python3 build.py'}, 'caller': {'type': 'direct'}}),
        result('t1', stdout),
        assistant('m2', {'type': 'tool_use', 'id': 't2', 'name': 'Bash', 'input': {'command': "grep -n -A1 '^error' build.log"}}),
        result('t2', '1:error[E0308]: mismatched types\n2:  --> src/ledger/settle.rs:88:17\n'),
        assistant('m3', {'type': 'text', 'text': 'src/ledger/settle.rs'}),
        {'type': 'result', 'is_error': False, 'result': 'src/ledger/settle.rs'},
    ]


class RebuildTests(unittest.TestCase):
    def test_a_transcript_becomes_messages_and_steps(self):
        messages, steps = nap.rebuild(transcript(build_output()), 'Run the build.')
        self.assertEqual([m['role'] for m in messages], ['user', 'assistant', 'user', 'assistant', 'user', 'assistant'])
        self.assertEqual(messages[0]['content'], 'Run the build.')
        # One message per id, thinking dropped, only the fields the API takes.
        self.assertEqual(messages[1]['content'], [
            {'type': 'text', 'text': 'Running the build.'},
            {'type': 'tool_use', 'id': 't1', 'name': 'Bash', 'input': {'command': 'python3 build.py'}}])
        self.assertEqual(len(steps), 2)
        self.assertEqual((steps[0]['message'], steps[0]['block'], steps[0]['tool']), (2, 0, 'Bash'))
        self.assertEqual(steps[0]['response']['stdout'], build_output())

    def test_parallel_results_share_one_message(self):
        events = transcript('x')
        events.insert(4, {'type': 'assistant', 'message': {'id': 'm1', 'content': [
            {'type': 'tool_use', 'id': 't9', 'name': 'Grep', 'input': {'pattern': 'error'}}]}})
        events.insert(6, {'type': 'user', 'message': {'role': 'user', 'content': [
            {'tool_use_id': 't9', 'type': 'tool_result', 'content': [{'type': 'text', 'text': 'a:1:error'}]}]},
            'tool_use_result': {'mode': 'content', 'content': 'a:1:error', 'numLines': 1}})
        messages, steps = nap.rebuild(events, 'p')
        self.assertEqual(len(messages[2]['content']), 2)
        self.assertEqual(messages[2]['content'][1]['content'], 'a:1:error')
        self.assertEqual([s['tool'] for s in steps], ['Bash', 'Grep', 'Bash'])

    def test_names_give_round_task_and_arm(self):
        self.assertEqual(nap.parse_name('/x/2-build_warnings-off.jsonl.gz'), (2, 'build_warnings', 'off'))

    def test_the_replayed_prefix_is_cached_and_the_step_replaced(self):
        messages, steps = nap.rebuild(transcript(build_output()), 'p')
        conversation = nap.replay(messages, steps[0], 'VIEW')
        self.assertEqual(len(conversation), 3)
        self.assertEqual(conversation[1]['content'][-1]['cache_control'], {'type': 'ephemeral'})
        self.assertEqual(conversation[2]['content'][0]['content'], 'VIEW')
        self.assertNotIn('cache_control', messages[1]['content'][-1])
        self.assertEqual(messages[2]['content'][0]['content'], build_output())


class ActionTests(unittest.TestCase):
    def test_shell_commands_decompose(self):
        self.assertEqual(nap.bash_action("cd repo && grep -n -A8 '^error' /tmp/out.txt | head -n 40"),
                         ('grep', ['/tmp/out.txt'], '^error', 'head:40'))
        self.assertEqual(nap.bash_action("sed -n '80,96p' src/ledger/settle.rs"),
                         ('sed', ['src/ledger/settle.rs'], None, '80-96'))
        self.assertEqual(nap.bash_action("git log -p -S 'price + discount' -- pricing.py"),
                         ('git log', ['pricing.py'], 'price + discount', None))
        self.assertEqual(nap.bash_action('LC_ALL=C python3 -m unittest -v test_rates')[0], 'python3')

    def test_tool_calls_and_answers_compare_by_their_parts(self):
        read = nap.action([{'type': 'thinking', 'thinking': ''},
                           {'type': 'tool_use', 'name': 'Read', 'input': {'file_path': './billing.py', 'offset': 900, 'limit': 40}}])
        self.assertEqual(read, {'kind': 'tool', 'tool': 'Read', 'program': None, 'paths': ['billing.py'],
                                'pattern': None, 'range': '900+40'})
        check = lambda text: 'settle.rs' in text
        self.assertEqual(nap.action([{'type': 'text', 'text': 'src/ledger/settle.rs'}], check), {'kind': 'final', 'correct': True})
        self.assertEqual(nap.action([{'type': 'text', 'text': 'Done.'}]), {'kind': 'final', 'correct': None})

    def test_paths_are_relative_to_the_session_directory(self):
        cwd = '/var/folders/x/T/repo'
        grep = lambda path: [{'type': 'tool_use', 'name': 'Grep', 'input': {'pattern': 'x', 'path': path}}]
        same = {nap.key(nap.action(grep(p), cwd=cwd)) for p in
                (f'{cwd}/logs/a.log', f'/private{cwd}/logs/a.log', './logs/a.log', 'logs/a.log')}
        self.assertEqual(len(same), 1)
        self.assertEqual(nap.bash_action(f'grep -n x {cwd}/logs/a.log', cwd)[1], ['logs/a.log'])

    def test_coarse_nap_ignores_pattern_and_range(self):
        call = lambda pattern: {'action': {'kind': 'tool', 'tool': 'Grep', 'program': None, 'paths': ['a.log'],
                                           'pattern': pattern, 'range': None}}
        results = {'raw': [call('ERROR'), call('ERROR|WARN'), call('ERROR')], 'v4': [call('Caused')] * 3,
                   'v5g': [call('ERROR')] * 3, 'v5': [{'action': {'kind': 'final', 'correct': True}}] * 3}
        _, strict = nap.score(results)
        _, coarse = nap.score(results, coarse=True)
        self.assertEqual((strict['v4'], coarse['v4']), (0, 1))
        self.assertEqual((strict['raw'], coarse['raw']), (2 / 3, 1))
        self.assertEqual(coarse['v5'], 0)

    def test_nap_is_the_share_of_draws_equal_to_the_modal_raw_action(self):
        a, b = {'kind': 'final', 'correct': True}, {'kind': 'final', 'correct': False}
        results = {'raw': [{'action': a}, {'action': a}, {'action': b}],
                   'v4': [{'action': a}, {'action': b}, {'action': None, 'error': 'HTTP 500'}],
                   'v5g': [{'action': b}] * 3, 'v5': [{'action': None}] * 3}
        modal, scores = nap.score(results)
        self.assertEqual(json.loads(modal), a)
        self.assertAlmostEqual(scores['raw'], 2 / 3)
        self.assertEqual(scores['v4'], 0.5)  # the failed draw is unknown, not a miss
        self.assertEqual(scores['v5g'], 0)
        self.assertIsNone(scores['v5'])


def fake_client(seen):
    def client(messages):
        seen.append(messages)
        text = messages[-1]['content'][0]['content']
        if 'boom' in text:
            raise RuntimeError('HTTP 400: bad')
        # A model that answers from what it sees, else searches.
        content = ([{'type': 'text', 'text': 'src/ledger/settle.rs'}] if 'error[E0308]' in text else
                   [{'type': 'tool_use', 'id': 'x', 'name': 'Bash', 'input': {'command': 'grep -n error build.log'}}])
        return {'stop_reason': 'end_turn', 'content': content,
                'usage': {'input_tokens': len(text) // 4, 'cache_creation_input_tokens': 0,
                          'cache_read_input_tokens': 1000, 'output_tokens': 20}}
    return client


@unittest.skipUnless(BINARY.exists(), 'needs target/release/scopelet')
class RunTests(unittest.TestCase):
    def write(self, folder, name, events):
        with gzip.open(Path(folder) / name, 'wt') as f:
            f.write('\n'.join(json.dumps(e) for e in events))

    def test_a_run_scores_each_compressed_step_and_never_overwrites(self):
        with tempfile.TemporaryDirectory() as temp:
            self.write(temp, '0-build_warnings-off.jsonl.gz', transcript(build_output()))
            self.write(temp, '0-build_warnings-v4.jsonl.gz', transcript(build_output()))
            seen = []
            out = Path(temp) / 'nap'
            report = nap.run(BINARY, temp, fake_client(seen), out, draws=2, workers=1)
            # The large build output is a step; the 2-line grep result is not.
            self.assertEqual(len(report['steps']), 1)
            step = report['steps'][0]
            self.assertLess(step['view_bytes']['v4'], step['view_bytes']['raw'])
            self.assertLess(step['view_bytes']['v5'], step['view_bytes']['v4'])
            self.assertEqual(len(seen), 4 * 2)
            # The error line survives every view, so every condition answers alike.
            self.assertEqual(step['raw_action'], {'kind': 'final', 'correct': True})
            self.assertEqual(report['summary']['conditions']['v5']['mean_nap'], 1.0)
            self.assertEqual(report['summary']['conditions']['raw']['errors'], 0)
            self.assertTrue((out / 'report.json').exists())
            with self.assertRaises(FileExistsError):
                nap.run(BINARY, temp, fake_client([]), out)

    def test_a_result_the_host_cut_is_skipped_not_compared(self):
        events = transcript(build_output())
        # Claude Code showed a head of the output, the response holds it all.
        events[4]['message']['content'][0]['content'] = build_output()[:1500]
        with tempfile.TemporaryDirectory() as temp:
            self.write(temp, '0-build_warnings-off.jsonl.gz', events)
            report = nap.run(BINARY, temp, fake_client([]), Path(temp) / 'nap', draws=1, workers=1)
            self.assertEqual(report['steps'], [])
            self.assertEqual(report['skipped'][0]['reason'], 'shown text is not the tool response')

    def test_a_result_the_host_persisted_is_skipped(self):
        events = transcript(build_output())
        events[4]['message']['content'][0]['content'] = '<persisted-output>\nPreview (first 2KB):\n...'
        events[4]['tool_use_result'].update(persistedOutputPath='/x/tool-results/a.txt', persistedOutputSize=40000)
        with tempfile.TemporaryDirectory() as temp:
            self.write(temp, '0-build_warnings-off.jsonl.gz', events)
            report = nap.run(BINARY, temp, fake_client([]), Path(temp) / 'nap', draws=1, workers=1)
            self.assertEqual(report['steps'], [])
            self.assertTrue(report['skipped'][0]['reason'].startswith('persisted by the host'))

    def test_failed_calls_are_null_with_their_error(self):
        events = transcript(build_output().replace('mismatched types', 'mismatched types boom'))
        with tempfile.TemporaryDirectory() as temp:
            self.write(temp, '0-build_warnings-off.jsonl.gz', events)
            report = nap.run(BINARY, temp, fake_client([]), Path(temp) / 'nap', draws=1, workers=1)
            draws = report['steps'][0]['draws']['raw']
            self.assertIsNone(draws[0]['action'])
            self.assertIsNone(draws[0]['input_total'])
            self.assertIn('400', draws[0]['error'])
            self.assertEqual(report['summary']['conditions']['raw']['errors'], 1)


class GuardTests(unittest.TestCase):
    def test_nothing_runs_without_live(self):
        with patch.object(nap, 'run', side_effect=AssertionError('called')):
            self.assertEqual(nap.main(['--transcripts', '.', '--out', '/nonexistent/nap']), 2)

    def test_nothing_runs_without_a_key(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(nap, 'run', side_effect=AssertionError('called')):
            self.assertEqual(nap.main(['--live', '--transcripts', '.', '--out', '/nonexistent/nap']), 2)


if __name__ == '__main__':
    unittest.main()
