#!/usr/bin/env python3
"""Export inspected current-campaign evidence, excluding private paths and text."""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys

import current
import compare
import run as base
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from export_comparison import USAGE_FIELDS, private_paths


def export(root):
    report = json.loads((root / 'report.json').read_text())
    output = {key: report[key] for key in ('schema_version', 'models', 'seed', 'max_attempts', 'binary_sha256',
                                         'provenance', 'host_versions', 'harness_sha256', 'unmeasured')}
    output['notes'] = [
        'Exploratory automatic integration comparison; two repetitions per main cell, no significance claim.',
        'Logical input includes cached tokens and is not billed cost. Reported USD is a provider list-basis estimate, not an invoice.',
        'Failed attempts remain in totals. Missing telemetry never implies zero usage or no activation.',
        'Codex serving model identity is unknown when the provider omits it; requested model is recorded.',
        'The follow-up binary may match the release; in that case it measures variability, not an optimization.',
    ]
    output['stops'] = report.get('stops', [])
    output['harness_snapshots'] = report.get('harness_snapshots', {})
    output['exporter_sha256'] = {name: base.sha256_file(Path(__file__).with_name(name))
                                for name in ('export_current.py', 'current.py', 'compare.py', 'run.py')}
    output['followup_tasks'] = report.get('followup_tasks', {})
    rows = []
    for row in report['runs']:
        raw_path = root / 'runs' / current.case_id(row) / 'stdout.raw'
        recovered = row.get('recovered_evidence', {})
        expected = row.get('stdout_sha256', recovered.get('stdout_sha256'))
        if expected is not None and (not raw_path.is_file() or base.sha256_file(raw_path) != expected):
            raise ValueError('retained transcript hash mismatch: ' + current.case_id(row))
        selected = {k: row[k] for k in ('phase', 'agent', 'arm', 'task', 'repetition', 'status', 'error_type', 'archive_error', 'passed',
                                      'duration_seconds', 'exit_code', 'timed_out', 'model_requested', 'effort_requested',
                                      'fixture_hashes', 'stdout_sha256', 'stderr_sha256', 'integration', 'integration_valid', 'proxy') if k in row}
        selected['usage'] = {key: current.usage_for(row).get(key) for key in USAGE_FIELDS}
        if recovered:
            selected['usage_recovered_from_transcript'] = True
            selected['stdout_sha256'] = recovered['stdout_sha256']
            selected['limitation'] = recovered['limitation']
        grade = row.get('acceptance', {})
        selected['acceptance'] = {k: grade.get(k) for k in ('grade', 'passed', 'exit_code', 'timed_out')}
        workspace = raw_path.parent / 'workspace_after'
        if row['task'] == 'task2' and workspace.is_dir():
            recheck = base.grade_workspace('task2', workspace, expected_fixture_hashes=row.get('fixture_hashes'))
            selected['acceptance_recheck'] = {k: recheck.get(k) for k in ('grade', 'passed', 'exit_code', 'timed_out')}
            selected['passed'] = bool(row.get('passed') and recheck['passed'])
        # Replay retained traces with the corrected parser; do not rewrite historical rows.
        tools = compare.adoption(row['agent'], raw_path.read_bytes(), row['task']) if expected is not None else row.get('tools', {})
        selected['checks_sequence'] = {'verified': tools.get('checks_sequence_verified'), 'checks_runs': tools.get('checks_runs', [])}
        selected['adoption'] = {k: tools.get(k) for k in ('rtk_invocations', 'rtk_recall_invocations',
                                                        'checks_command_invocations', 'scopelet_expand_invocations',
                                                        'headroom_retrieve_calls', 'tool_error_count')}
        rows.append(selected)
    output['runs'] = rows
    output['summary'] = current.summarize(rows)
    output['attempts'] = len(rows)
    if private_paths(output):
        raise ValueError('private path detected in export')
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('campaign', type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    value = export(args.campaign)
    args.out.write_text(json.dumps(value, indent=2) + '\n')


if __name__ == '__main__':
    main()
