import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import publish


def reports():
    content_report = {'cases': {'diagnostics': {'states': {'cold': {'median_seconds': 0.0053}}}}}
    performance_report = {'arms': ['release'], 'cases': {
        'log/cold': {'release': {'summary': {'median_seconds': 0.0151}}},
        'repo/cold': {'release': {'summary': {'median_seconds': 0.058}}},
        'limit_32m/cold': {'release': {'summary': {'median_seconds': 0.1611}}},
    }}
    return content_report, performance_report


class PublishTests(unittest.TestCase):
    def test_medians_quote_cold_milliseconds_for_every_row(self):
        values = publish.medians(*reports())
        self.assertEqual([label for label, _ in publish.ROWS], list(values))
        self.assertAlmostEqual(values['Compress a 136 KB log'], 5.3)
        self.assertAlmostEqual(values['Compress a 32 MiB stream'], 161.1)

    def test_table_names_version_and_machine_and_bolds_fast_rows(self):
        block = publish.table(publish.medians(*reports()), '0.5.2', 'macOS arm64', 30)
        self.assertTrue(block.startswith(publish.START) and block.endswith(publish.END))
        self.assertIn('Scopelet\n0.5.2', block)
        self.assertIn('macOS arm64', block)
        self.assertIn('| Compress a 3 MB log | **15 ms** |', block)
        self.assertIn('| Compress a 32 MiB stream | 161 ms |', block)

    def test_rewrite_replaces_only_the_marked_block(self):
        readme = f'before\n{publish.START}\nold\n{publish.END}\nafter\n'
        self.assertEqual(publish.rewrite(readme, f'{publish.START}\nnew\n{publish.END}'),
                         f'before\n{publish.START}\nnew\n{publish.END}\nafter\n')
        with self.assertRaises(ValueError):
            publish.rewrite('no markers', 'block')

    def test_publishing_replaces_only_the_offline_reports(self):
        with tempfile.TemporaryDirectory() as temp:
            runs, results = Path(temp) / 'runs', Path(temp) / 'results'
            for probe in ('content', 'performance'):
                (runs / probe).mkdir(parents=True)
                (runs / probe / 'report.json').write_text(f'new {probe}')
            results.mkdir()
            (results / 'content.json').write_text('old content')
            (results / 'live-20261104-120-haiku.json').write_text('live')
            publish.install_reports(runs, results)
            self.assertEqual((results / 'content.json').read_text(), 'new content')
            self.assertEqual((results / 'performance.json').read_text(), 'new performance')
            self.assertEqual((results / 'live-20261104-120-haiku.json').read_text(), 'live')

    def test_a_readme_without_markers_stops_before_anything_is_measured_or_installed(self):
        measured = AssertionError('a probe ran before the README was checked')
        with tempfile.TemporaryDirectory() as temp:
            readme = Path(temp) / 'README.md'
            readme.write_text('no speed table here\n')
            results = Path(temp) / 'results'
            with patch.object(publish, 'RESULTS', results), \
                    patch.object(publish.performance, 'binary_version', side_effect=measured), \
                    patch.object(publish.content, 'measure', side_effect=measured), \
                    patch.object(publish.performance, 'run', side_effect=measured), \
                    self.assertRaises(ValueError):
                publish.main(['--scopelet', '/bin/echo', '--readme', str(readme)])
            self.assertFalse(results.exists())
            self.assertEqual(readme.read_text(), 'no speed table here\n')

    def test_zero_repetitions_are_refused_like_the_probes_do(self):
        with patch.object(publish.content, 'measure', side_effect=AssertionError('measured')), \
                patch.object(sys, 'stderr', io.StringIO()), self.assertRaises(SystemExit):
            publish.main(['--scopelet', '/bin/echo', '--repetitions', '0'])

    def test_machine_reads_as_a_platform_name(self):
        name = publish.machine()
        self.assertEqual(len(name.split()), 2)


if __name__ == '__main__':
    unittest.main()
