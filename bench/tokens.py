#!/usr/bin/env python3
"""Token probe: what a compact view costs in real Claude tokens.

For every content fixture (`content.py`: the synthetic cases and the real
outputs of `fixtures/`), a cargo run over twelve test binaries, a listing with
`node_modules` and a large source file, this compresses the output as the
hooks would (`scopelet compress --profile`) with the binary under test
(`single`) and, with `--baseline BINARY`, with a reference binary in its
default format (`baseline`, such as the 0.11.1 release), then counts the raw
output and every view with `POST /v1/messages/count_tokens`, the exact and
free count of the model's own tokenizer. A count is net: the tokens of one
user message holding the text, less the frame around it, measured as a
message holding `.` (one token; the API refuses an empty message). A count
that fails is null, never zero, and the report keeps the error; sums cover
the fixtures every compared view counted.

Calls happen only with `--live`; the key is read from `ANTHROPIC_API_KEY`
and never written or printed. No model generates anything.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

import content

HERE = Path(__file__).resolve().parent
MODELS = ['claude-opus-5-5', 'claude-haiku-5-5']
ENDPOINT = 'https://api.anthropic.com/v1/messages/count_tokens'
# The smallest message the API accepts: its count less one is the frame.
BASELINE = '.'


def workspace_run():
    """A cargo test run over twelve test binaries, one failing: a test view
    with a totals line, which no content fixture has."""
    parts = []
    for binary in range(12):
        failed = binary == 7
        tests = ''.join(f'test m{binary}::case_{i} ... {"FAILED" if failed and i == 3 else "ok"}\n' for i in range(30))
        status = 'FAILED' if failed else 'ok'
        parts.append(f'     Running tests/suite_{binary}.rs (target/debug/deps/suite_{binary}-0123456789abcdef)\n\n'
                     f'running 30 tests\n{tests}\ntest result: {status}. {29 if failed else 30} passed; '
                     f'{1 if failed else 0} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.0{binary}s\n\n')
    return ''.join(parts).encode(), ['totals runs=12 passed=359 failed=1 ignored=0']


def word(i):
    return ''.join(chr(ord('a') + int(c)) for c in str(i))


def listing():
    """`find .` in a project with dependencies and caches."""
    lines = ['.', './src', './node_modules']
    for i in range(400):
        lines.append(f'./src/module_{word(i)}.py')
        if i % 2 == 0:
            lines += [f'./node_modules/pkg-{word(i)}/index.js', f'./node_modules/pkg-{word(i)}/package.json']
    lines += [f'./src/__pycache__/module_{word(i)}.cpython-312.pyc' for i in range(40)]
    return ('\n'.join(lines) + '\n').encode(), ['./src/module_a.py', './node_modules/pkg-a/index.js']


def large_source():
    """A 48 KiB Python module read whole."""
    parts = ['"""Request handlers."""\n\n\nclass Router:\n    pass\n\n\n']
    for i in range(90):
        parts.append(f'def handler_{word(i)}(request, context):\n')
        parts += [f'    value_{k} = request.get("{word(i * 7 + k)}") or context.default_{k}\n' for k in range(9)]
        parts.append('    return value_0\n\n\n')
    return ''.join(parts).encode(), ['class Router:', 'def handler_a(request, context):']


def cases():
    """The content fixtures, the workspace run, a listing and a large source
    file, with their profiles."""
    return {**content.fixtures(), 'cargo_workspace': workspace_run(),
            'listing': listing(), 'large_source': large_source()}


PROFILES = {'cargo_workspace': ['--profile', 'tests'], 'listing': ['--profile', 'search'],
            'large_source': ['--profile', 'file-read']}


def arms(binary, baseline=None):
    """(name, binary) of every compared view: the reference binary's when
    given, then the tested binary's."""
    return [*([('baseline', Path(baseline))] if baseline else []), ('single', Path(binary))]


def view(binary, cache, raw, name):
    """The view `binary` makes of `raw`, in its default format."""
    env = {k: v for k, v in os.environ.items() if not k.startswith('SCOPELET_')}
    env['SCOPELET_EVENTS'] = '0'
    result = subprocess.run([str(binary), '--cache-dir', str(cache), 'compress',
                             *PROFILES.get(name, content.profile_args(name))], input=raw, capture_output=True, env=env, timeout=60)
    if result.returncode != 0:
        raise RuntimeError(f'{name} {binary}: exit {result.returncode}: {result.stderr.decode(errors="replace")}')
    return result.stdout


class ApiCounter:
    """Counts with the Messages API; one request per distinct (model, text)."""

    def __init__(self, key, retries=5):
        self.key = key
        self.retries = retries

    def __call__(self, model, text):
        body = json.dumps({'model': model, 'messages': [{'role': 'user', 'content': text}]}).encode()
        for attempt in range(self.retries + 1):
            request = urllib.request.Request(ENDPOINT, data=body, method='POST', headers={
                'x-api-key': self.key, 'anthropic-version': '2023-06-01', 'content-type': 'application/json'})
            try:
                with urllib.request.urlopen(request, timeout=60) as response:
                    return json.load(response)['input_tokens']
            except urllib.error.HTTPError as error:
                # Rate limits and overload are retried; anything else is the answer.
                if error.code not in (429, 500, 502, 503, 529) or attempt == self.retries:
                    detail = error.read().decode(errors='replace')[:300]
                    raise RuntimeError(f'HTTP {error.code}: {detail}') from None
                time.sleep(float(error.headers.get('retry-after') or 2 ** attempt))
        raise AssertionError('unreachable')


def counted(counter, model, text, memo):
    """Net tokens of `text`, or (None, error); identical texts are counted once."""
    key = (model, hashlib.sha256(text.encode()).hexdigest())
    if key not in memo:
        try:
            memo[key] = (counter(model, text), None)
        except Exception as error:  # noqa: BLE001 - a failure is reported, not fatal
            memo[key] = (None, str(error))
    return memo[key]


def compared(fixtures, complete, model, name, reference, totals):
    """What `name` saves against `reference` over the `complete` fixtures."""
    worse = sorted(n for n, f in fixtures.items()
                   if f in complete and f['tokens'][model][name] > f['tokens'][model][reference])
    return {'saved_tokens': totals[reference] - totals[name],
            'saved_percent': round(100 * (totals[reference] - totals[name]) / totals[reference], 2),
            'fixtures_worse': worse}


def summarize(fixtures, model, names):
    """Totals over the fixtures every view counted; the tested view against
    the raw output and, when measured, against the baseline view."""
    complete = [f for f in fixtures.values() if all(f['tokens'][model].get(n) is not None for n in ['raw', *names])]
    totals = {n: sum(f['tokens'][model][n] for f in complete) for n in ['raw', *names]}
    summary = {'fixtures_counted': len(complete), 'fixtures_missing': len(fixtures) - len(complete), 'tokens': totals}
    for reference in ('raw', 'baseline'):
        if reference in totals and totals[reference] and 'single' in totals:
            summary[f'single_vs_{reference}'] = compared(fixtures, complete, model, 'single', reference, totals)
    return summary


def measure(binary, counter, models, out, baseline=None):
    """Write `out` (refusing an existing file) and return the report."""
    if out.exists():
        raise FileExistsError(f'{out} exists; completed evidence is never overwritten')
    compared_arms = arms(binary, baseline)
    names = [name for name, _ in compared_arms]
    report = {'kind': 'exact token counts from POST /v1/messages/count_tokens, net of the message frame',
              'binary_sha256': {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in compared_arms},
              'models': models, 'arms': names, 'fixtures': {}}
    memo = {}
    baselines = {}
    for model in models:
        count, error = counted(counter, model, BASELINE, memo)
        baselines[model] = None if count is None else count - 1
        report.setdefault('frame_tokens', {})[model] = baselines[model]
        if error:
            report.setdefault('errors', []).append({'model': model, 'baseline': error})
    with tempfile.TemporaryDirectory(prefix='scopelet-tokens-') as cache:
        for name, (raw, facts) in cases().items():
            views = {'raw': raw}
            for arm, path in compared_arms:
                # Each binary its own cache: one cache never holds the other's artifacts.
                views[arm] = view(path, Path(cache) / arm, raw, name)
            entry = {'bytes': {k: len(v) for k, v in views.items()},
                     'facts_visible': {arm: all(f.encode() in views[arm] or content.source_form(f).encode() in views[arm]
                                                for f in facts) for arm in names},
                     'tokens': {}, 'errors': {}}
            for model in models:
                entry['tokens'][model] = {}
                for arm, data in views.items():
                    text = data.decode('utf-8', errors='replace')
                    # An empty output costs nothing, and the API refuses it.
                    count, error = (0, None) if not text else counted(counter, model, text, memo)
                    if count and baselines[model] is None:
                        count, error = None, error or 'frame unknown'
                    elif count:
                        count -= baselines[model]
                    entry['tokens'][model][arm] = count
                    if error:
                        entry['errors'][f'{model}/{arm}'] = error
            if not entry['errors']:
                del entry['errors']
            report['fixtures'][name] = entry
            print(f'{name}: ' + ', '.join(f'{arm}={entry["tokens"][models[0]][arm]}' for arm in ['raw', *names]),
                  flush=True)
    report['summary'] = {model: summarize(report['fixtures'], model, names) for model in models}
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, indent=2) + '\n')
    return report


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--live', action='store_true', help='required: call the token counting API')
    p.add_argument('--scopelet', type=Path, default=HERE.parent / 'target/release/scopelet',
                   help='binary under test: the `single` view')
    p.add_argument('--baseline', type=Path, help='reference binary (such as the 0.11.1 release): the `baseline` view')
    p.add_argument('--model', action='append', help=f'repeatable; default {" and ".join(MODELS)}')
    p.add_argument('--out', type=Path, default=HERE / 'results' / 'tokens.json')
    args = p.parse_args(argv)
    if not args.live:
        print('tokens.py calls the token counting API; pass --live to run it.', file=sys.stderr)
        return 2
    key = os.environ.get('ANTHROPIC_API_KEY')
    if not key:
        print('ANTHROPIC_API_KEY is not set.', file=sys.stderr)
        return 2
    if args.out.exists():
        p.error(f'{args.out} exists; completed evidence is never overwritten')
    report = measure(args.scopelet.resolve(), ApiCounter(key), args.model or MODELS, args.out,
                     args.baseline.resolve() if args.baseline else None)
    print(json.dumps(report['summary'], indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
