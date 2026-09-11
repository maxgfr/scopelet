import unittest
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

    def test_machine_reads_as_a_platform_name(self):
        name = publish.machine()
        self.assertEqual(len(name.split()), 2)


if __name__ == '__main__':
    unittest.main()
