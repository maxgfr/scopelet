#!/usr/bin/env python3
"""Token probe: what each compact grammar costs in real Claude tokens.

For every content fixture (`content.py`: the synthetic cases and the real
outputs of `fixtures/`) and a cargo run over twelve test binaries, this compresses the output as the hooks would
(`scopelet compress --profile`) in compact-v4, in compact-v5 and in v5 with
each part of its grammar alone (`SCOPELET_V5_GRAMMAR`), then counts the raw
output and every view with `POST /v1/messages/count_tokens`, the exact and
free count of the model's own tokenizer. A count is net: the tokens of one
user message holding the text, less the frame around it, measured as a
message holding `.` (one token; the API refuses an empty message). A count
that fails is null, never zero, and the report keeps the
error; sums cover the fixtures every compared grammar counted.

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
# The parts of the v5 grammar `SCOPELET_V5_GRAMMAR` can keep alone.
PARTS = ['labels', 'header', 'tables']
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


def cases():
    """The content fixtures, and the workspace run with its profile."""
    return {**content.fixtures(), 'cargo_workspace': workspace_run()}


PROFILES = {'cargo_workspace': ['--profile', 'tests']}


def arms():
    """(name, compact version, SCOPELET_V5_GRAMMAR or None for all parts)."""
    return [('v4', '4', None), ('v5', '5', None)] + [(f'v5_{part}', '5', part) for part in PARTS]


def view(binary, cache, raw, name, version, grammar):
    env = {**os.environ, 'SCOPELET_EVENTS': '0'}
    env.pop('SCOPELET_V5_GRAMMAR', None)
    if grammar is not None:
        env['SCOPELET_V5_GRAMMAR'] = grammar
    result = subprocess.run([str(binary), '--cache-dir', str(cache), '--compact-version', version, 'compress',
                             *PROFILES.get(name, content.profile_args(name))], input=raw, capture_output=True, env=env, timeout=60)
    if result.returncode != 0:
        raise RuntimeError(f'{name} v{version}: exit {result.returncode}: {result.stderr.decode(errors="replace")}')
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


def summarize(fixtures, model, names):
    """Totals over the fixtures every grammar counted, and v4 to each arm."""
    complete = [f for f in fixtures.values() if all(f['tokens'][model].get(n) is not None for n in ['raw', *names])]
    totals = {n: sum(f['tokens'][model][n] for f in complete) for n in ['raw', *names]}
    summary = {'fixtures_counted': len(complete), 'fixtures_missing': len(fixtures) - len(complete),
               'tokens': totals, 'vs_v4': {}}
    for name in names:
        if name == 'v4' or not totals['v4']:
            continue
        worse = sorted(n for n, f in fixtures.items() if f in complete and f['tokens'][model][name] > f['tokens'][model]['v4'])
        summary['vs_v4'][name] = {'saved_tokens': totals['v4'] - totals[name],
                                  'saved_percent': round(100 * (totals['v4'] - totals[name]) / totals['v4'], 2),
                                  'fixtures_worse': worse}
    return summary


def measure(binary, counter, models, out):
    """Write `out` (refusing an existing file) and return the report."""
    if out.exists():
        raise FileExistsError(f'{out} exists; completed evidence is never overwritten')
    names = [name for name, _, _ in arms()]
    report = {'kind': 'exact token counts from POST /v1/messages/count_tokens, net of the message frame',
              'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
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
            for arm, version, grammar in arms():
                views[arm] = view(binary, cache, raw, name, version, grammar)
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
    p.add_argument('--scopelet', type=Path, default=HERE.parent / 'target/release/scopelet')
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
    report = measure(args.scopelet.resolve(), ApiCounter(key), args.model or MODELS, args.out)
    print(json.dumps(report['summary'], indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
