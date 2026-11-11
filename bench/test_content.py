import json
from pathlib import Path
import os
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch
import content


class Hung:
    """A process that does not finish before its timeout."""
    pid = 2 ** 22 + 7
    returncode = None

    def __init__(self):
        self.waited = False

    def communicate(self, input=None, timeout=None):
        if timeout is not None and not self.waited:
            self.waited = True
            raise subprocess.TimeoutExpired('scopelet', timeout, output=b'partial')
        return b'partial', b''


class ContentTests(unittest.TestCase):
    def test_recovery_uses_source_blob_not_serialized_artifact(self):
        original = b'original\r\n'
        artifact = 'artifact:' + 'a' * 64
        blob = 'blob:' + 'b' * 64
        manifest = json.dumps({'snapshots': [{'blob': blob}]}).encode()
        with patch.object(content, 'bounded', side_effect=[subprocess.CompletedProcess([], 0, manifest), subprocess.CompletedProcess([], 0, original)]) as run:
            exact, recovered = content.recovery(Path('/bin/scopelet'), Path('/cache'), artifact.encode(), original)
        self.assertTrue(exact)
        self.assertEqual(recovered, original)
        self.assertEqual(run.call_args_list[-1].args[0][-2:], [blob, '--raw'])

    def test_read_failure_cannot_claim_roundtrip(self):
        with patch.object(content, 'bounded', return_value=subprocess.CompletedProcess([], 2, b'')):
            exact, recovered = content.recovery(Path('/bin/scopelet'), Path('/cache'), ('artifact:' + 'a' * 64).encode(), b'original')
        self.assertFalse(exact)
        self.assertIsNone(recovered)

    def test_untouched_output_counts_as_roundtrip_without_an_artifact(self):
        with patch.object(content, 'bounded') as run:
            exact, recovered = content.recovery(Path('/bin/scopelet'), Path('/cache'), b'same', b'same')
        self.assertTrue(exact)
        self.assertIsNone(recovered)
        run.assert_not_called()

    def test_only_the_giant_line_is_expected_after_recovery(self):
        self.assertEqual(content.RECOVERY_ONLY, {'giant_unicode_line'})
        self.assertTrue(content.RECOVERY_ONLY <= set(content.fixtures()))

    def test_file_fixtures_join_the_synthetic_corpus_without_shadowing_it(self):
        files = content.file_fixtures()
        synthetic = content.synthetic_fixtures()
        self.assertGreaterEqual(len(files), 15)
        self.assertFalse(set(files) & set(synthetic))
        merged = content.fixtures()
        self.assertEqual(set(merged), set(files) | set(synthetic))
        for name, spec in files.items():
            self.assertEqual(merged[name], (spec['bytes'], spec['facts']))
            self.assertTrue(spec['facts'] or spec['pending_facts'], name)
            self.assertIn(spec['profile'], {None, 'tests', 'search', 'git', 'file-read', 'logs'}, name)

    def test_file_fixtures_are_compressed_with_their_profile(self):
        self.assertEqual(content.profile_args('rust_large'), ['--profile', 'file-read'])
        self.assertEqual(content.profile_args('cargo_test'), ['--profile', 'tests'])
        self.assertEqual(content.profile_args('kubectl_json'), [])
        self.assertEqual(content.profile_args('diagnostics'), [])

    def test_block_notation_facts_map_to_their_source_lines(self):
        self.assertEqual(content.source_form('\n }\n \n'), '\n}\n\n')
        self.assertEqual(content.source_form('error: x'), 'error: x')

    def test_every_fixture_file_has_a_manifest_entry(self):
        stems = {path.stem for path in content.FIXTURES.glob('*.txt')}
        self.assertEqual(stems, set(content.file_fixtures()))

    def test_pending_facts_are_reported_apart_from_enforced_facts(self):
        for name, facts in content.pending_facts().items():
            self.assertTrue(facts)
            self.assertFalse(set(facts) & set(content.file_fixtures()[name]['facts']), name)

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

    def test_a_hung_binary_is_a_failed_run_kept_in_the_report(self):
        started = []
        real = subprocess.run

        def version(command, **kwargs):
            # The binary's version; anything else (`platform` asks `uname`) runs.
            if '--version' in command:
                return subprocess.CompletedProcess(command, 0, 'scopelet 0.0.0\n', '')
            return real(command, **kwargs)
        real_popen = subprocess.Popen

        def popen(command, **kwargs):
            # The binary hangs; anything else really runs.
            if 'compress' in command or 'expand' in command:
                started.append(command)
                return Hung()
            return real_popen(command, **kwargs)
        with tempfile.TemporaryDirectory() as temp, patch('builtins.print'), \
                patch.object(content, 'fixtures', return_value={'tiny': (b'x' * 64, ['x'])}), \
                patch.object(content.subprocess, 'run', side_effect=version), \
                patch.object(content.subprocess, 'Popen', side_effect=popen), \
                patch.object(content.os, 'killpg') as killed:
            binary = Path(temp) / 'binary'
            binary.write_bytes(b'frozen')
            report = content.measure(binary, Path(temp) / 'out', 3, 30)
            # Two states, one hung run each: not 2 x 33, and each one killed.
            self.assertEqual(sum('compress' in command for command in started), 2)
            self.assertEqual(killed.call_count, 2)
            self.assertTrue((Path(temp) / 'out' / 'report.json').exists())
        # A hang is recorded once per state, then that state stops: the next
        # runs would only hang as long again.
        for state in ('cold', 'warm'):
            self.assertEqual(report['cases']['tiny']['states'][state]['failures'], 1)
            self.assertTrue(report['cases']['tiny']['states'][state]['samples'][0]['timed_out'])
        self.assertIn('tiny/cold: 1 non-zero exit(s)', content.failures(report))
        # A recovery that hangs cannot claim a round trip either.
        with patch.object(content.subprocess, 'Popen', side_effect=lambda command, **kw: Hung()), \
                patch.object(content.os, 'killpg'):
            exact, recovered = content.recovery(Path('/bin/scopelet'), Path('/cache'), ('artifact:' + 'a' * 64).encode(), b'o')
        self.assertFalse(exact)

    def test_a_timeout_kills_the_whole_process_group(self):
        # What hangs under `/usr/bin/time` is its child: the shell's
        # background sleep stands for it.
        with tempfile.TemporaryDirectory() as temp:
            child = Path(temp) / 'child'
            result = content.bounded(['/bin/sh', '-c', f'sleep 60 & echo $! > {child}; wait'], 1)
            self.assertEqual(result.returncode, 124)
            self.assertTrue(result.timed_out)
            pid = int(child.read_text())
            for _ in range(50):
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                os.kill(pid, 9)
                self.fail('the hung child outlived the timeout')
        # An interrupt of the probe takes its run with it.
        with tempfile.TemporaryDirectory() as temp:
            child = Path(temp) / 'child'

            def interrupted(self, *args, **kwargs):
                while not child.exists() or not child.read_text().strip():
                    time.sleep(0.01)
                raise KeyboardInterrupt
            with patch.object(subprocess.Popen, 'communicate', interrupted), self.assertRaises(KeyboardInterrupt):
                content.bounded(['/bin/sh', '-c', f'echo $$ > {child}; exec sleep 60'], 120)
            pid = int(child.read_text())
            for _ in range(50):
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                os.kill(pid, 9)
                self.fail('the run outlived the interrupt')
        ok = content.bounded(['/bin/cat'], 5, input=b'bytes')
        self.assertEqual((ok.returncode, ok.stdout, ok.timed_out), (0, b'bytes', False))

    def test_summary_keeps_exit_failures(self):
        value = content.summarize([{'seconds': .5, 'exit_code': 7}, {'seconds': .1, 'exit_code': 0}])
        self.assertEqual(value['failures'], 1)
        self.assertEqual(value['median_seconds'], .3)
        self.assertEqual(value['p95_seconds'], .5)


if __name__ == '__main__':
    unittest.main()
