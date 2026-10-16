#!/usr/bin/env python3
"""Live session benchmark: real Claude Code sessions, measured in tokens.

Each task is a synthetic repository and a prompt. Every arm runs the same
task in a fresh copy of it, with its own temporary `CLAUDE_CONFIG_DIR` and
Scopelet config and cache: `off` installs nothing, any other arm installs
its binary's hooks with `scopelet install --agent claude` and pins its
compact version. Repetitions are paired: each round runs every arm on every
task, in an order that rotates between rounds.

Each run records the session's input, cache creation, cache read and output
tokens, turns, tool calls and cost from `claude -p --output-format
stream-json`, plus a correctness check. Missing usage is recorded as null,
never zero. A run that fails or times out stays in the denominator.

Real model calls happen only with `--live`; without it the script exits
before calling anything.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / 'fixtures' / 'live'
TOOLS = 'Bash,Read,Edit,Write,Grep,Glob'


def load_tasks():
    return json.loads((FIXTURES / 'tasks.json').read_text())


# --- synthetic repositories -------------------------------------------------

def word(i):
    """A word per number, without digits, so lines do not fold by template."""
    return ''.join(chr(ord('a') + int(c)) for c in str(i))


def build(task, root):
    """Create the task's repository under `root` (a fresh directory)."""
    root.mkdir(parents=True)
    if task == 'fix_test':
        for name in ('inventory.py', 'test_inventory.py'):
            shutil.copy(FIXTURES / 'fix_test' / name, root / name)
    elif task == 'find_receipt':
        (root / 'logs').mkdir()
        lines = []
        for i in range(20000):
            lines.append(f'2026-10-01T10:{i // 60 % 60:02}:{i % 60:02}Z INFO worker-{i % 8} '
                         f'processed order {100000 + i} batch {word(i)}')
            if i == 11871:
                lines.append('2026-10-01T13:17:51Z INFO billing issued receipt RCPT-7139-QX for order 7139')
        (root / 'logs/app.log').write_text('\n'.join(lines) + '\n')
    elif task == 'count_failures':
        rows = []
        for i in range(5000):
            suite = ('billing', 'auth', 'search')[i % 3]
            failed = suite == 'billing' and i % 41 == 0
            rows.append(json.dumps({'id': i, 'suite': suite, 'status': 'failed' if failed else 'passed',
                                    'duration_ms': (i * 37) % 900, 'note': word(i)}))
        (root / 'events.jsonl').write_text('\n'.join(rows) + '\n')
    elif task == 'review_diff':
        git = lambda *a: subprocess.run(['git', *a], cwd=root, check=True, capture_output=True,
                                        env={**os.environ, 'GIT_AUTHOR_NAME': 'bench', 'GIT_AUTHOR_EMAIL': 'bench@example.com',
                                             'GIT_COMMITTER_NAME': 'bench', 'GIT_COMMITTER_EMAIL': 'bench@example.com',
                                             'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'})
        git('init', '-q')
        (root / 'src').mkdir()
        auth = [f'def helper_{word(i)}(value):\n    return value + {i}\n\n' for i in range(40)]
        auth.insert(20, 'def is_expired(expires_at, now):\n    return expires_at < now\n\n')
        (root / 'src/auth.py').write_text(''.join(auth))
        lock = lambda v: json.dumps({'name': 'app', 'lockfileVersion': 3, 'packages': {
            f'node_modules/pkg-{word(i)}': {'version': f'1.{i}.{v}', 'resolved': f'https://registry.npmjs.org/pkg-{word(i)}/-/pkg-{word(i)}-1.{i}.{v}.tgz',
                                           'integrity': 'sha512-' + hashlib.sha256(f'{i}-{v}'.encode()).hexdigest()} for i in range(400)}}, indent=2)
        (root / 'package-lock.json').write_text(lock(0))
        git('add', '-A'); git('commit', '-q', '-m', 'initial')
        (root / 'src/auth.py').write_text(''.join(auth).replace('return expires_at < now', 'return expires_at <= now'))
        (root / 'package-lock.json').write_text(lock(1))
        git('add', '-A'); git('commit', '-q', '-m', 'Bump dependencies and tidy auth')
    else:
        raise ValueError(task)


# --- one session ------------------------------------------------------------

def parse_stream(text):
    """Usage, turns, cost, answer and tool calls of a stream-json transcript.

    Missing figures stay None: an unknown usage is never zero.
    """
    tool_calls, tools, result = 0, {}, None
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get('type') == 'assistant':
            for block in event.get('message', {}).get('content', []) or []:
                if isinstance(block, dict) and block.get('type') == 'tool_use':
                    tool_calls += 1
                    name = str(block.get('name'))
                    tools[name] = tools.get(name, 0) + 1
        elif event.get('type') == 'result':
            result = event
    usage = (result or {}).get('usage') or {}
    pick = lambda key: usage.get(key) if isinstance(usage.get(key), int) else None
    record = {
        'input_tokens': pick('input_tokens'),
        'cache_creation_input_tokens': pick('cache_creation_input_tokens'),
        'cache_read_input_tokens': pick('cache_read_input_tokens'),
        'output_tokens': pick('output_tokens'),
        'turns': (result or {}).get('num_turns'),
        'cost_usd': (result or {}).get('total_cost_usd'),
        'tool_calls': tool_calls if result is not None else None,
        'tools': tools,
        'answer': (result or {}).get('result'),
        'session_error': (result or {}).get('is_error') if result is not None else True,
    }
    parts = [record[k] for k in ('input_tokens', 'cache_creation_input_tokens', 'cache_read_input_tokens')]
    record['input_total'] = sum(parts) if all(p is not None for p in parts) else None
    return record


def check(spec, answer, workdir):
    """Whether the session did the task, judged on the repository and answer."""
    kind = spec['type']
    if kind == 'answer_contains':
        return bool(answer) and spec['expected'] in answer
    if kind == 'answer_number':
        return bool(answer) and str(spec['expected']) in re.findall(r'\d+', answer)
    if kind == 'tests_pass':
        unchanged = all((workdir / name).read_bytes() == (FIXTURES / 'fix_test' / name).read_bytes()
                        for name in spec.get('unchanged', []))
        run = subprocess.run(spec['command'], cwd=workdir, capture_output=True, timeout=120)
        return unchanged and run.returncode == 0
    raise ValueError(kind)


def install(arm, scratch):
    """Environment for one session of `arm`: its own Claude and Scopelet dirs."""
    env = dict(os.environ)
    for key in list(env):
        if key.startswith('SCOPELET_') or key in ('CLAUDE_CONFIG_DIR', 'CODEX_HOME', 'OPENCODE_CONFIG_DIR'):
            env.pop(key)
    env['CLAUDE_CONFIG_DIR'] = str(scratch / 'claude')
    (scratch / 'claude').mkdir()
    if arm['binary']:
        env.update(SCOPELET_CONFIG_DIR=str(scratch / 'scopelet'), SCOPELET_CACHE_DIR=str(scratch / 'cache'),
                   SCOPELET_COMPACT_VERSION=arm['version'], CODEX_HOME=str(scratch / 'codex'),
                   OPENCODE_CONFIG_DIR=str(scratch / 'opencode'))
        subprocess.run([str(arm['binary']), 'install', '--agent', 'claude'], env=env, check=True,
                       capture_output=True, timeout=60)
    return env


def session(arm, task, spec, model, timeout, isolate_home):
    with tempfile.TemporaryDirectory(prefix=f'scopelet-live-{arm["name"]}-') as temp:
        scratch = Path(temp)
        workdir = scratch / 'repo'
        build(task, workdir)
        env = install(arm, scratch)
        if isolate_home:
            env['HOME'] = str(scratch / 'home')
            (scratch / 'home').mkdir()
        started = time.monotonic()
        try:
            # The prompt goes on stdin: `--allowedTools` takes every word after it.
            run = subprocess.run(['claude', '-p', '--model', model, '--output-format', 'stream-json', '--verbose',
                                  '--permission-mode', 'acceptEdits', f'--allowedTools={TOOLS}'],
                                 input=spec['prompt'], cwd=workdir, env=env, capture_output=True, text=True,
                                 timeout=timeout)
            record = parse_stream(run.stdout)
            record['exit_code'] = run.returncode
            if run.returncode != 0:
                record['stderr'] = run.stderr[-2000:]
        except subprocess.TimeoutExpired:
            record = parse_stream('')
            record['exit_code'] = None
            record['timed_out'] = True
        record['seconds'] = round(time.monotonic() - started, 1)
        try:
            record['correct'] = record['exit_code'] == 0 and check(spec['check'], record['answer'], workdir)
        except Exception as error:  # a broken check is a failed run, kept in the report
            record['correct'] = False
            record['check_error'] = repr(error)
        stats = scratch / 'cache' / 'events'
        if stats.is_dir():
            record['scopelet_events'] = sum(len(f.read_text().splitlines()) for f in stats.glob('*.jsonl'))
        return {'arm': arm['name'], 'task': task, **record}


# --- report -------------------------------------------------------------------

def median(values):
    values = [v for v in values if v is not None]
    return statistics.median(values) if values else None


def summarize(records, arms, tasks):
    """Per task and arm, and over all tasks per arm. Failures stay counted."""
    def cell(rows):
        return {'runs': len(rows), 'correct': sum(bool(r.get('correct')) for r in rows),
                'with_usage': sum(r.get('input_total') is not None for r in rows),
                **{f'median_{key}': median(r.get(key) for r in rows)
                   for key in ('input_total', 'input_tokens', 'cache_creation_input_tokens', 'cache_read_input_tokens',
                               'output_tokens', 'turns', 'tool_calls', 'cost_usd', 'seconds')}}
    summary = {task: {arm: cell([r for r in records if r['task'] == task and r['arm'] == arm]) for arm in arms}
               for task in tasks}
    summary['all'] = {arm: cell([r for r in records if r['arm'] == arm]) for arm in arms}
    return summary


def parse_arm(text):
    """`off`, or `NAME=BINARY@VERSION` (version 3 or 4)."""
    if text == 'off':
        return {'name': 'off', 'binary': None, 'version': None}
    name, _, rest = text.partition('=')
    binary, _, version = rest.partition('@')
    if not name or not binary or version not in ('1', '2', '3', '4', '5'):
        raise argparse.ArgumentTypeError('expected off or NAME=BINARY@VERSION')
    return {'name': name, 'binary': Path(binary).resolve(), 'version': version}


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--live', action='store_true', help='required: run real, billed model sessions')
    p.add_argument('--arm', action='append', type=parse_arm, help='off, or NAME=BINARY@VERSION; repeatable')
    p.add_argument('--task', action='append', help='task name from fixtures/live/tasks.json; default all')
    p.add_argument('--runs', type=int, default=3, help='paired rounds')
    p.add_argument('--model', default='haiku')
    p.add_argument('--timeout', type=int, default=900, help='seconds per session')
    p.add_argument('--isolate-home', action='store_true',
                   help='also give each session a temporary HOME (needs an API key; OAuth lives in the keychain)')
    p.add_argument('--out', type=Path, help='report path; default bench/results/live-<UTC time>.json')
    args = p.parse_args(argv)
    if not args.live:
        print('live.py runs real, billed Claude Code sessions; pass --live to run them.', file=sys.stderr)
        return 2
    arms = args.arm or [parse_arm('off')]
    names = [a['name'] for a in arms]
    if len(set(names)) != len(names):
        p.error('arm names must be distinct')
    tasks = load_tasks()
    selected = args.task or list(tasks)
    out = args.out or HERE / 'results' / time.strftime('live-%Y%m%dT%H%M%SZ.json', time.gmtime())
    if out.exists():
        p.error(f'{out} exists; completed evidence is never overwritten')
    report = {'kind': 'live Claude Code sessions; tokens from the session result, not estimates',
              'model': args.model, 'runs': args.runs, 'tasks': selected,
              'claude_version': subprocess.run(['claude', '--version'], capture_output=True, text=True).stdout.strip(),
              'arms': [{'name': a['name'], 'version': a['version'],
                        'binary_sha256': hashlib.sha256(a['binary'].read_bytes()).hexdigest() if a['binary'] else None}
                       for a in arms],
              'records': []}
    out.parent.mkdir(parents=True, exist_ok=True)
    for round_ in range(args.runs):
        order = arms[round_ % len(arms):] + arms[:round_ % len(arms)]
        for task in selected:
            for arm in order:
                record = session(arm, task, tasks[task], args.model, args.timeout, args.isolate_home)
                record['round'] = round_
                report['records'].append(record)
                print(f"round {round_} {task} {arm['name']}: correct={record['correct']} "
                      f"input_total={record['input_total']} turns={record['turns']} tools={record['tool_calls']}",
                      flush=True)
                report['summary'] = summarize(report['records'], names, selected)
                out.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['summary']['all'], indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
