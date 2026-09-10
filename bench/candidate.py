#!/usr/bin/env python3
"""Compact-v3 candidate campaign on Claude Code / Haiku. No model calls without --live; at most 18 attempts."""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import sys
import tempfile

import compare
import run as base

MODEL = 'claude-haiku-4-5-20251001'
TASKS = ('task4', 'task2', 'task3')
ARMS = ('native', 'released', 'candidate')
SEED = 20260910
MAX_ATTEMPTS = 18
DECISION_TASKS = ('task4', 'task2')
DECISION_RULE = ('go when every candidate attempt passes and the candidate mean of logical input plus output tokens '
                 '(cache included) over task4 and task2 is at most the released mean; task3 is reported without a criterion.')


def matrix(repetitions=2):
    cases = [dict(task=t, arm=a, repetition=r) for r in range(1, repetitions + 1) for t in TASKS for a in ARMS]
    random.Random(SEED).shuffle(cases)
    return cases


def case_id(case):
    return '-'.join(str(case[k]) for k in ('task', 'arm', 'repetition'))


def session_tokens(row):
    usage = row.get('usage', {})
    values = [usage.get(k) for k in ('logical_input_tokens', 'output_tokens')]
    return sum(values) if all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in values) else None


def summarize(rows):
    result = {}
    for task in TASKS:
        for arm in ARMS:
            selected = [r for r in rows if r.get('task') == task and r.get('arm') == arm]
            tokens = [session_tokens(r) for r in selected if session_tokens(r) is not None]
            seconds = [r['duration_seconds'] for r in selected if r.get('duration_seconds') is not None]
            costs = [r['usage']['total_cost_usd_reported'] for r in selected
                     if r.get('usage', {}).get('total_cost_usd_reported') is not None]
            passes = sum(bool(r.get('passed')) for r in selected)
            result[f'{task}/{arm}'] = {
                'attempts': len(selected), 'passes': passes, 'tokens': tokens,
                'missing_usage': len(selected) - len(tokens),
                'mean_tokens': statistics.mean(tokens) if tokens else None,
                'median_tokens': statistics.median(tokens) if tokens else None,
                'mean_seconds': statistics.mean(seconds) if seconds else None,
                'total_cost_usd_reported': sum(costs) if selected and len(costs) == len(selected) else None}
    return result


def decision(summary):
    """Pre-declared go/no-go; None (undecidable) when any input is missing."""
    reasons = []
    for task in TASKS:
        cell = summary.get(f'{task}/candidate', {})
        if cell.get('attempts', 0) and cell['passes'] < cell['attempts']:
            reasons.append(f'candidate functional failure on {task}')
    means = {}
    for arm in ('released', 'candidate'):
        cells = [summary.get(f'{task}/{arm}', {}) for task in DECISION_TASKS]
        if any(not c.get('attempts') or c.get('missing_usage') or c.get('mean_tokens') is None for c in cells):
            return {'go': None, 'rule': DECISION_RULE, 'reasons': reasons + [f'{arm} tokens incomplete on {DECISION_TASKS}'], 'combined_mean_tokens': means}
        means[arm] = sum(c['mean_tokens'] for c in cells)
    if means['candidate'] > means['released']:
        reasons.append('candidate combined mean tokens above released')
    return {'go': not reasons, 'rule': DECISION_RULE, 'reasons': reasons, 'combined_mean_tokens': means}


def isolated_environment(workspace):
    state = workspace / '.benchmark'
    state.mkdir(mode=0o700)
    for name in ('.ignore', '.gitignore'):
        (state / name).write_text('*\n')
    env = base.purged_environment()
    for key in list(env):
        if key.startswith(('SCOPELET_', 'RTK_', 'HEADROOM_')):
            env.pop(key)
    env.update(SCOPELET_CONFIG_DIR=str(state / 'config'), SCOPELET_CACHE_DIR=str(state / 'cache'))
    return state, env


def treatment_command(case, workspace, state, env, binaries):
    command = base.command_for('claude', MODEL, None, include_hook_events=True) + ['--disable-slash-commands']
    if case['arm'] != 'native':
        install_env = {**env, 'CLAUDE_CONFIG_DIR': str(workspace / '.claude')}
        subprocess.run([str(binaries[case['arm']]), 'install', '--agent', 'claude'], env=install_env, check=True, capture_output=True)
        env['PATH'] = str(state / 'config/bin') + os.pathsep + env.get('PATH', '')
        command += ['--settings', str(workspace / '.claude/settings.json')]
        # Explicit --settings owns these hooks; avoid loading them twice via project settings.
        command[command.index('--setting-sources') + 1] = ''
    return command


def run_case(case, binaries, destination, timeout):
    destination.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix='scopelet-candidate-') as tmp:
        workspace = Path(tmp)
        base.create_fixture(workspace, case['task'])
        hashes = base.fixture_hashes(workspace)
        base._git_init(workspace)
        state, env = isolated_environment(workspace)
        command = treatment_command(case, workspace, state, env, binaries)
        prompt = base.prompt_for('claude', case['task'], 'baseline', Path('scopelet')).replace(' Do not invoke Scopelet.', '')
        (destination / 'prompt.txt').write_text(prompt)
        invocation = compare.invoke(command, prompt, workspace, timeout, env)
        stdout, stderr = invocation.pop('stdout'), invocation.pop('stderr')
        invocation.pop('command', None)
        for name, data in [('stdout.raw', stdout), ('stderr.raw', stderr)]:
            (destination / name).write_bytes(data)
        usage = base.normalize_usage('claude', stdout, MODEL)
        grade = base.grade_workspace(case['task'], workspace, expected_fixture_hashes=hashes)
        calls = compare.ordered_tool_calls('claude', stdout)
        hooks = compare.hook_events(stdout)
        evidence = {'compact_outputs': sum('[scopelet compact-' in c['text'] for c in calls),
                    'compact_v3_outputs': sum('[scopelet compact-v3' in c['text'] for c in calls),
                    'recovery_calls': sum('expand' in c['payload'] and 'scopelet' in c['payload'] for c in calls),
                    'tool_calls': len(calls), 'hooks': hooks}
        row = {**case, **invocation, 'model_requested': MODEL, 'usage': usage, 'acceptance': grade,
               'fixture_hashes': hashes, 'stdout_sha256': base.sha256_bytes(stdout),
               'stderr_sha256': base.sha256_bytes(stderr), 'integration': evidence,
               'quota_rejected': compare.rate_limit_reset(stdout + stderr) is not None}
        if case['arm'] != 'native':
            row['integration_valid'] = bool(evidence['compact_outputs'] or hooks['responses_with_output'])
        else:
            row['integration_valid'] = None
        row['passed'] = bool(grade['passed'] and invocation.get('exit_code') == 0 and not usage.get('model_mismatch')
                             and not usage.get('result_is_error'))
        (destination / 'result.json').write_text(json.dumps(row, indent=2) + '\n')
        shutil.rmtree(state)
        # Installed hook paths are private campaign evidence, never exports.
        shutil.rmtree(workspace / '.claude', ignore_errors=True)
        base._copy_tree_after(workspace, destination / 'workspace_after')
        (destination / 'result.json').write_text(json.dumps(row, indent=2) + '\n')
        return row


def recover_attempt_error(case, destination, error):
    """Keep a completed grade, or recover paid usage when postprocessing fails."""
    result = destination / 'result.json'
    if result.is_file():
        row = json.loads(result.read_text())
        row['archive_error'] = type(error).__name__
        return row
    row = {**case, 'status': 'harness_error', 'error_type': type(error).__name__, 'passed': False}
    raw_path = destination / 'stdout.raw'
    if raw_path.is_file():
        raw = raw_path.read_bytes()
        row['usage'] = base.normalize_usage('claude', raw, MODEL)
        row['recovered_evidence'] = {'stdout_sha256': base.sha256_bytes(raw),
                                     'limitation': 'Usage recovered after postprocessing failure; independent grade unavailable.'}
    return row


def save(out, report):
    report['summary'] = summarize(report['runs'])
    report['decision'] = decision(report['summary'])
    temp = out / 'report.json.tmp'
    temp.write_text(json.dumps(report, indent=2) + '\n')
    temp.replace(out / 'report.json')


def reserve(report, case, limit):
    if len(report['runs']) >= limit:
        raise ValueError('campaign attempt budget exhausted')
    if any(case_id(row) == case_id(case) for row in report['runs']):
        raise ValueError('attempt already exists; never overwrite or retry it')
    report['runs'].append({**case, 'status': 'started'})


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--live', action='store_true')
    p.add_argument('--released', type=Path, help='Frozen released binary (0.3.2, compact-v2 default)')
    p.add_argument('--candidate', type=Path, help='Candidate binary (compact-v3 default)')
    p.add_argument('--repetitions', type=int, default=2)
    p.add_argument('--max-attempts', type=int, default=MAX_ATTEMPTS)
    p.add_argument('--timeout', type=float, default=600)
    args = p.parse_args(argv)
    if not 1 <= args.repetitions <= 2:
        p.error('one or two repetitions')
    if not 1 <= args.max_attempts <= MAX_ATTEMPTS:
        p.error(f'budget must be between 1 and {MAX_ATTEMPTS}')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    cases = matrix(args.repetitions)
    if not args.live:
        report = {'schema_version': 1, 'model': MODEL, 'seed': SEED, 'live': False, 'matrix': cases,
                  'max_attempts': args.max_attempts, 'decision_rule': DECISION_RULE, 'runs': []}
        save(out, report)
        print(json.dumps({'model': MODEL, 'budget': args.max_attempts, 'cells': len(cases), 'rule': DECISION_RULE}, indent=2))
        return 0
    # Prevent concurrent invocations from bypassing the shared attempt ledger.
    with (out / '.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if (out / 'report.json').exists() and json.loads((out / 'report.json').read_text()).get('live'):
            report = json.loads((out / 'report.json').read_text())
            binaries = {k: Path(v) for k, v in report['private_paths'].items()}
            for key, digest in report['binary_sha256'].items():
                if base.sha256_file(binaries[key]) != digest:
                    raise ValueError(f'frozen {key} changed')
        else:
            if not (args.released and args.candidate):
                p.error('--live needs --released and --candidate')
            frozen = out / 'frozen'
            frozen.mkdir(exist_ok=True)
            binaries = {}
            for arm in ('released', 'candidate'):
                binaries[arm] = frozen / arm
                shutil.copy2(getattr(args, arm).resolve(), binaries[arm])
            report = {'schema_version': 1, 'model': MODEL, 'seed': SEED, 'live': True, 'matrix': cases,
                      'max_attempts': args.max_attempts, 'decision_rule': DECISION_RULE, 'runs': [],
                      'private_paths': {k: str(v) for k, v in binaries.items()},
                      'binary_sha256': {k: base.sha256_file(v) for k, v in binaries.items()},
                      'binary_versions': {k: base.cli_version(v) for k, v in binaries.items()},
                      'harness_sha256': {name: base.sha256_file(Path(__file__).with_name(name))
                                         for name in ('candidate.py', 'run.py', 'compare.py')},
                      'host_version': base.agent_cli_version('claude'),
                      'notes': ['Equal prompts and fixture-scoped host configuration; global skills and slash commands disabled.',
                                'Logical input plus output includes cached input and is not billed cost.',
                                'Two repetitions per cell are exploratory and cannot establish significance.',
                                'Failures stay in the sample; missing usage is unknown, never zero; no automatic retry.']}
        limit = min(report['max_attempts'], args.max_attempts)
        save(out, report)
        for case in cases:
            if any(case_id(r) == case_id(case) for r in report['runs']):
                continue
            if len(report['runs']) >= limit:
                break
            reserve(report, case, limit)
            save(out, report)
            try:
                row = run_case(case, binaries, out / 'runs' / case_id(case), args.timeout)
            except Exception as error:
                row = recover_attempt_error(case, out / 'runs' / case_id(case), error)
                print(type(error).__name__, str(error), file=sys.stderr, flush=True)
            report['runs'][-1] = row
            save(out, report)
            print(json.dumps({'id': case_id(case), 'passed': row.get('passed'), 'seconds': row.get('duration_seconds'),
                              'tokens': session_tokens(row), 'cost': row.get('usage', {}).get('total_cost_usd_reported'),
                              'integration': row.get('integration_valid')}), flush=True)
            if row.get('quota_rejected') or row.get('usage', {}).get('usage_missing', True):
                report.setdefault('stops', []).append({'case': case_id(case), 'reason': 'quota or incomplete usage; no automatic retry'})
                save(out, report)
                break
    return 0


if __name__ == '__main__':
    sys.exit(main())
