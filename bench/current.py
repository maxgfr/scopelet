#!/usr/bin/env python3
"""Current automatic integrations. One append-only campaign, at most 60 attempts."""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import random
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile

import compare
import run as base

MODELS = {'codex': 'gpt-5.6-luna', 'claude': 'claude-haiku-4-5-20251001'}
TASKS = ('task3', 'task4', 'task2')
ARMS = ('native', 'scopelet', 'rtk', 'headroom')
SEED = 20260910


def matrix(phase, followup=None):
    if phase == 'preflight':
        return [dict(agent=a, arm=b, task='task4', repetition=0, phase=phase)
                for a in MODELS for b in ('scopelet', 'headroom')]
    if phase == 'main':
        cases = [dict(agent=a, arm=b, task=t, repetition=r, phase=phase)
                 for a in MODELS for b in ARMS for t in TASKS for r in (1, 2)]
    else:
        cases = [dict(agent=a, arm=b, task=followup[a], repetition=r, phase=phase)
                 for a in MODELS for b in ('scopelet', 'candidate') for r in (1, 2)]
    random.Random(SEED).shuffle(cases)
    return cases


def case_id(case):
    return '-'.join(str(case[k]) for k in ('phase', 'agent', 'arm', 'task', 'repetition'))


def usage_for(row):
    return row.get('usage', row.get('recovered_evidence', {}).get('usage', {}))


def session_tokens(row):
    usage = usage_for(row)
    values = [usage.get(k) for k in ('logical_input_tokens', 'output_tokens')]
    return sum(values) if all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in values) else None


def summarize(rows):
    cells = {}
    for row in rows:
        key = '/'.join(row[k] for k in ('phase', 'agent', 'task', 'arm'))
        cells.setdefault(key, []).append(row)
    result = {}
    for key, selected in cells.items():
        tokens = [session_tokens(r) for r in selected if session_tokens(r) is not None]
        seconds = [r['duration_seconds'] for r in selected if r.get('duration_seconds') is not None]
        costs = [usage_for(r)['total_cost_usd_reported'] for r in selected if usage_for(r).get('total_cost_usd_reported') is not None]
        passes = sum(r.get('passed', False) for r in selected)
        result[key] = {'attempts': len(selected), 'passes': passes, 'tokens': tokens,
                       'missing_usage': len(selected) - len(tokens),
                       'mean_tokens': statistics.mean(tokens) if tokens else None,
                       'median_tokens': statistics.median(tokens) if tokens else None,
                       'min_tokens': min(tokens) if tokens else None, 'max_tokens': max(tokens) if tokens else None,
                       'mean_seconds': statistics.mean(seconds) if seconds else None,
                       'total_cost_usd_reported': sum(costs) if len(costs) == len(selected) else None,
                       'cost_per_pass_usd_reported': sum(costs) / passes if passes and len(costs) == len(selected) else None}
    return result


def selected_followups(rows):
    """Predeclared priority: functional failures, then greatest token overhead."""
    cells = summarize(rows)
    result = {}
    for agent in MODELS:
        def score(task):
            native = cells.get(f'main/{agent}/{task}/native', {})
            scoped = cells.get(f'main/{agent}/{task}/scopelet', {})
            failures = scoped.get('attempts', 0) - scoped.get('passes', 0)
            n, s = native.get('mean_tokens'), scoped.get('mean_tokens')
            return failures, s / n if s is not None and n else float('-inf')
        result[agent] = max(TASKS, key=score)
    return result


def isolated_environment(workspace):
    state = workspace / '.benchmark'
    state.mkdir(mode=0o700)
    for name in ('.ignore', '.gitignore'):
        (state / name).write_text('*\n')
    original_codex = Path(os.environ.get('CODEX_HOME', Path.home() / '.codex'))
    codex_home = state / 'codex'
    codex_home.mkdir(mode=0o700)
    for name in ('auth.json', 'models_cache.json'):
        source = original_codex / name
        if source.is_file():
            shutil.copyfile(source, codex_home / name)
            (codex_home / name).chmod(0o600)
    env = base.purged_environment()
    for key in list(env):
        if key.startswith(('SCOPELET_', 'RTK_', 'HEADROOM_')):
            env.pop(key)
    env.update(CODEX_HOME=str(codex_home), SCOPELET_CONFIG_DIR=str(state / 'config'),
               SCOPELET_CACHE_DIR=str(state / 'cache'), RTK_DB_PATH=str(state / 'rtk.db'),
               RTK_RECALL_DB=str(state / 'recall.db'), RTK_TELEMETRY_DISABLED='1',
               RTK_HOOK_AUDIT='0', HEADROOM_BEACON='off', HEADROOM_TELEMETRY='off')
    return state, env


def treatment_command(case, workspace, state, env, paths):
    agent, arm = case['agent'], case['arm']
    if agent == 'codex':
        command = base.command_for(agent) + ['--dangerously-bypass-hook-trust']
        disabled = compare.global_skill_paths()
        if disabled:
            command += ['-c', 'skills.config=[' + ','.join('{path=' + json.dumps(str(p)) + ',enabled=false}' for p in disabled) + ']']
    else:
        command = base.command_for(agent, MODELS[agent], None, include_hook_events=True)
        command += ['--disable-slash-commands']
    if arm in ('scopelet', 'candidate'):
        binary = paths[arm]
        install_env = {**env, 'CLAUDE_CONFIG_DIR': str(workspace / '.claude')}
        subprocess.run([str(binary), 'install', '--agent', agent], env=install_env, check=True, capture_output=True)
        env['PATH'] = str(state / 'config/bin') + os.pathsep + env.get('PATH', '')
        if agent == 'claude':
            command += ['--settings', str(workspace / '.claude/settings.json')]
            # Explicit --settings owns these hooks; avoid loading them twice via project settings.
            command[command.index('--setting-sources') + 1] = ''
    if arm == 'rtk':
        env['PATH'] = str(paths['rtk'].parent) + os.pathsep + env.get('PATH', '')
        if agent == 'codex':
            (workspace / 'AGENTS.md').write_bytes(paths['awareness'].read_bytes())
        else:
            command += ['--settings', compare.rtk_hook_settings(paths['rtk'])]
    if arm == 'headroom':
        command = [sys.executable, str(Path(__file__).with_name('headroom_proxy.py').resolve()),
                   '--headroom', str(paths['headroom'])] + command
    return command


def rtk_counts(path):
    if not path.is_file():
        return {}
    try:
        with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True) as db:
            names = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            return {name: db.execute('SELECT COUNT(*) FROM "' + name + '"').fetchone()[0]
                    for name in ('commands', 'hook_events') if name in names}
    except sqlite3.Error as error:
        # Optional telemetry must never discard a completed model session or its grade.
        return {'unavailable': type(error).__name__}


def run_case(case, paths, destination, timeout):
    destination.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix='scopelet-current-') as tmp:
        workspace = Path(tmp)
        base.create_fixture(workspace, case['task'])
        hashes = base.fixture_hashes(workspace)
        base._git_init(workspace)
        state, env = isolated_environment(workspace)
        command = treatment_command(case, workspace, state, env, paths)
        prompt = base.prompt_for(case['agent'], case['task'], 'baseline', Path('scopelet')).replace(' Do not invoke Scopelet.', '')
        (destination / 'prompt.txt').write_text(prompt)
        invocation = compare.invoke(command, prompt, workspace, timeout, env)
        stdout, stderr = invocation.pop('stdout'), invocation.pop('stderr')
        invocation.pop('command', None)
        for name, data in [('stdout.raw', stdout), ('stderr.raw', stderr)]:
            (destination / name).write_bytes(data)
        usage = base.normalize_usage(case['agent'], stdout, MODELS[case['agent']])
        grade = base.grade_workspace(case['task'], workspace, expected_fixture_hashes=hashes)
        calls = compare.ordered_tool_calls(case['agent'], stdout)
        hooks = compare.hook_events(stdout)
        evidence = {'compact_outputs': sum('[scopelet compact-' in c['text'] for c in calls),
                    'recovery_calls': sum('expand' in c['payload'] and 'scopelet' in c['payload'] for c in calls),
                    'tool_calls': len(calls), 'hooks': hooks, 'rtk_database_rows': rtk_counts(state / 'rtk.db')}
        row = {**case, **invocation, 'model_requested': MODELS[case['agent']],
               'effort_requested': 'low' if case['agent'] == 'codex' else None,
               'usage': usage, 'acceptance': grade, 'fixture_hashes': hashes,
               'stdout_sha256': base.sha256_bytes(stdout), 'stderr_sha256': base.sha256_bytes(stderr),
               'integration': evidence, 'tools': compare.adoption(case['agent'], stdout, case['task']),
               'quota_rejected': compare.rate_limit_reset(stdout + stderr) is not None}
        if case['arm'] == 'headroom':
            summary = workspace / '.headroom-comparison/summary.json'
            row['proxy'] = json.loads(summary.read_text()) if summary.is_file() else {}
            row['integration_valid'] = row['proxy'].get('routing_verified', False)
        elif case['arm'] in ('scopelet', 'candidate'):
            row['integration_valid'] = bool(evidence['compact_outputs'] or hooks['responses_with_output']) if case['phase'] == 'preflight' else None
        else:
            row['integration_valid'] = None
        row['passed'] = bool(grade['passed'] and invocation.get('exit_code') == 0 and not usage.get('model_mismatch')
                             and not usage.get('result_is_error') and row.get('integration_valid') is not False)
        (destination / 'result.json').write_text(json.dumps(row, indent=2) + '\n')
        shutil.rmtree(state)
        # Installed hook paths and local proxy data are private campaign evidence, never exports.
        shutil.rmtree(workspace / '.claude', ignore_errors=True)
        if (workspace / '.headroom-comparison').exists():
            shutil.move(str(workspace / '.headroom-comparison'), destination / 'proxy-private')
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
        row['recovered_evidence'] = {'usage': base.normalize_usage(case['agent'], raw, MODELS[case['agent']]),
                                     'stdout_sha256': base.sha256_bytes(raw),
                                     'limitation': 'Usage recovered after postprocessing failure; independent grade unavailable.'}
    return row


def save(out, report):
    report['summary'] = summarize(report['runs'])
    temp = out / 'report.json.tmp'
    temp.write_text(json.dumps(report, indent=2) + '\n')
    temp.replace(out / 'report.json')


def reserve(report, case, limit):
    if len(report['runs']) >= limit:
        raise ValueError('campaign attempt budget exhausted')
    if any(case_id(row) == case_id(case) for row in report['runs']):
        raise ValueError('attempt already exists; never overwrite or retry it')
    report['runs'].append({**case, 'status': 'started'})


def freeze(out, args):
    frozen = out / 'frozen'
    frozen.mkdir()
    paths = {}
    for name in ('scopelet', 'rtk', 'awareness'):
        source = getattr(args, name)
        if source is None or not source.is_file():
            raise ValueError(f'provide --{name} as an existing file')
        paths[name] = frozen / name
        shutil.copy2(source.resolve(), paths[name])
    paths['headroom'] = args.headroom.resolve()
    return paths


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--phase', choices=('preflight', 'main', 'followup'), default='preflight')
    p.add_argument('--live', action='store_true')
    for name in ('scopelet', 'rtk', 'awareness', 'headroom', 'candidate'):
        p.add_argument('--' + name, type=Path)
    p.add_argument('--max-attempts', type=int, default=60)
    p.add_argument('--timeout', type=float, default=600)
    p.add_argument('--provenance', type=Path, help='Verified release/package provenance JSON')
    args = p.parse_args()
    if not 1 <= args.max_attempts <= 60:
        p.error('budget must be between 1 and 60')
    if not args.live:
        print(json.dumps({'models': MODELS, 'budget': args.max_attempts, 'matrix': matrix(args.phase, dict.fromkeys(MODELS, 'task4'))}, indent=2))
        return
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    # Prevent concurrent invocations from bypassing the shared attempt ledger.
    with (out / '.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if (out / 'report.json').exists():
            report = json.loads((out / 'report.json').read_text())
            paths = {k: Path(v) for k, v in report['private_paths'].items()}
            for key, digest in report['binary_sha256'].items():
                if base.sha256_file(paths[key]) != digest:
                    raise ValueError(f'frozen {key} changed')
        else:
            if args.phase != 'preflight' or not args.headroom:
                p.error('start with preflight and all installation paths')
            paths = freeze(out, args)
            report = {'schema_version': 1, 'models': MODELS, 'seed': SEED, 'runs': [], 'max_attempts': args.max_attempts,
                      'private_paths': {k: str(v) for k, v in paths.items()},
                      'binary_sha256': {k: base.sha256_file(v) for k, v in paths.items()},
                      'provenance': json.loads(args.provenance.read_text()) if args.provenance else {},
                      'host_versions': {a: base.agent_cli_version(a) for a in MODELS},
                      'unmeasured': {'codex/headroom': 'Portable adapter rejects Codex: authenticated routing has not been established.'}}
        limit = min(report['max_attempts'], args.max_attempts)
        if args.phase == 'followup':
            if 'candidate' not in paths:
                paths['candidate'] = out / 'frozen/candidate'
                shutil.copy2((args.candidate or paths['scopelet']).resolve(), paths['candidate'])
                report['private_paths']['candidate'] = str(paths['candidate'])
                report['binary_sha256']['candidate'] = base.sha256_file(paths['candidate'])
            report['followup_tasks'] = selected_followups(report['runs'])
        harness = {name: base.sha256_file(Path(__file__).with_name(name))
                   for name in ('current.py', 'run.py', 'compare.py', 'headroom_proxy.py')}
        report.setdefault('harness_sha256', {})[args.phase] = harness
        snapshot = base.sha256_bytes(json.dumps(harness, sort_keys=True).encode())
        saved_harness = out / 'frozen' / 'harness' / snapshot
        if not saved_harness.exists():
            saved_harness.mkdir(parents=True)
            for name in harness:
                shutil.copyfile(Path(__file__).with_name(name), saved_harness / name)
        report.setdefault('harness_snapshots', {})[snapshot] = harness
        save(out, report)
        for case in matrix(args.phase, report.get('followup_tasks')):
            if any(case_id(r) == case_id(case) for r in report['runs']):
                continue
            if f"{case['agent']}/{case['arm']}" in report['unmeasured']:
                continue
            if args.phase != 'preflight':
                preflight = [r for r in report['runs'] if r['phase'] == 'preflight' and r['agent'] == case['agent']
                             and r['arm'] == ('headroom' if case['arm'] == 'headroom' else 'scopelet')]
                if not preflight or not preflight[0].get('passed') or preflight[0].get('usage', {}).get('usage_missing', True):
                    continue
            if len(report['runs']) >= limit:
                break
            reserve(report, case, limit)
            save(out, report)
            try:
                row = run_case(case, paths, out / 'runs' / case_id(case), args.timeout)
            except Exception as error:
                row = recover_attempt_error(case, out / 'runs' / case_id(case), error)
                print(type(error).__name__, str(error), file=sys.stderr, flush=True)
            report['runs'][-1] = row
            save(out, report)
            print(json.dumps({'id': case_id(case), 'passed': row.get('passed'), 'seconds': row.get('duration_seconds'),
                              'tokens': session_tokens(row), 'integration': row.get('integration_valid')}), flush=True)
            if row.get('quota_rejected') or row.get('usage', {}).get('usage_missing', True):
                report.setdefault('stops', []).append({'case': case_id(case), 'reason': 'quota or incomplete usage; no automatic retry'})
                save(out, report)
                break


if __name__ == '__main__':
    main()
