#!/usr/bin/env python3
"""Publish accounting and fixture hashes without account metadata or transcripts."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('scopelet_benchmark', ROOT / 'bench/run.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def export(directory):
    report = json.loads((directory / 'report.json').read_text())
    meta = report['meta']
    rows = []
    for run in report['runs']:
        stdout = (directory / run['stdout_file']).read_bytes()
        stderr = (directory / run['stderr_file']).read_bytes()
        usage = harness.normalize_usage(run['agent'], stdout)
        row = {key: run[key] for key in ('run_id', 'agent', 'task', 'arm', 'exit_code', 'timed_out', 'duration_seconds')}
        row.update({
            'passed': run['acceptance']['passed'],
            'native_skill_requested': (directory / run['prompt_file']).read_text().startswith('/scopelet '),
            'usage': usage,
            'tools': harness.tool_usage(run['agent'], stdout, stderr),
            'fixture_sha256': digest(json.dumps(run['fixture_hashes'], sort_keys=True).encode()),
            'stdout_sha256': digest(stdout),
            'stderr_sha256': digest(stderr),
            'binary_sha256': run.get('binary_sha256'),
        })
        rows.append(row)
    return {
        'name': directory.name,
        'planned_runs': meta['planned_runs'],
        'completed_runs': len(rows),
        'binary_sha256': meta.get('binary_sha256'),
        'skill_hashes': meta.get('skill_hashes'),
        'cli_version': meta.get('cli_version'),
        'agent_cli_versions': meta.get('agent_cli_versions'),
        'models': {'codex': 'gpt-5.6-luna', 'claude': 'claude-haiku-4-5-20251001'},
        'runs': rows,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directories', type=Path, nargs='+')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    data = {
        'schema_version': 1,
        'export_harness_sha256': digest((ROOT / 'bench/run.py').read_bytes()),
        'metric': 'logical input plus reported output; cached input included; not dollars',
        'limitations': [
            'One run per cell, fixed arm order, synthetic tasks; no statistical significance claim.',
            'Installed global skill context may be discovered despite user settings overrides.',
            'Adoption counts recognize a conservative subset of shell syntax; raw commands were inspected for highlighted comparisons.',
            'campaign-1 is exploratory: binary and skill were edited during that campaign; it is not an immutable release comparison.',
            'campaign-1, final-20260909 and commands-20260909 used Claude setting-sources empty: project skill discovery was disabled. Claude skill trials are in claude-skill-20260909.',
            'Raw transcripts stay local; published hashes permit checking retained originals.',
        ],
        'campaigns': [export(directory) for directory in args.directories],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(data, indent=2, sort_keys=True)+'\n')
    print(args.out)


if __name__ == '__main__':
    main()
