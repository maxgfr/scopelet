#!/usr/bin/env python3
"""Export an inspected candidate campaign without private paths or raw transcripts."""
import argparse
import json
from pathlib import Path
import candidate

REPORT_KEYS = ('schema_version', 'model', 'seed', 'live', 'matrix', 'max_attempts', 'decision_rule', 'binary_sha256',
               'binary_versions', 'harness_sha256', 'host_version', 'notes', 'stops')
RUN_KEYS = ('task', 'arm', 'repetition', 'status', 'exit_code', 'timed_out', 'duration_seconds', 'model_requested',
            'usage', 'acceptance', 'passed', 'fixture_hashes', 'stdout_sha256', 'stderr_sha256', 'integration',
            'integration_valid', 'quota_rejected', 'archive_error', 'error_type', 'recovered_evidence')


def export(source):
    original = json.loads((source / 'report.json').read_text())
    report = {k: original[k] for k in REPORT_KEYS if k in original}
    report['runs'] = [{k: row[k] for k in RUN_KEYS if k in row} for row in original.get('runs', [])]
    report['summary'] = candidate.summarize(report['runs'])
    report['decision'] = candidate.decision(report['summary'])
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(export(args.source), indent=2) + '\n')


if __name__ == '__main__':
    main()
