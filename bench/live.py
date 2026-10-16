#!/usr/bin/env python3
"""Live session benchmark: real Claude Code sessions, measured in billed cost.

Each task is a synthetic repository and a prompt. Every arm runs the same
task in a fresh copy of it, with its own temporary `CLAUDE_CONFIG_DIR` and
Scopelet config and cache: `off` installs nothing, any other arm
(`NAME=BINARY`) installs its binary's hooks with `scopelet install --agent
claude`, in that binary's default format. Repetitions are paired: each round
runs every arm on every task, in an order that rotates between rounds.

Each run records the session's input, cache creation, cache read and output
tokens, turns, tool calls, cost and the model actually served from `claude -p
--output-format stream-json`, the number of Scopelet views the model was
actually shown (`views_shown`, counted in that stream), plus a correctness
check. Missing usage is
recorded as null, never zero. A run that fails or times out stays in the
denominator. The summary judges arms by billed cost per successful task:
every run's cost over the successes, failures included, and, against a
baseline arm, paired deltas per (task, round) with a 95% interval from a
bootstrap over tasks.

Real model calls happen only with `--live`; without it the script exits
before calling anything.
"""
from __future__ import annotations
import argparse
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import random
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
# Bootstrap draws and seed: fixed, so a report is reproducible from its records.
DRAWS = 10_000
SEED = 7139


def load_tasks():
    return json.loads((FIXTURES / 'tasks.json').read_text())


# --- synthetic repositories -------------------------------------------------

def word(i):
    """A word per number, without digits, so lines do not fold by template."""
    return ''.join(chr(ord('a') + int(c)) for c in str(i))


def git_in(root):
    """`git` in `root` with a fixed identity and clock, ignoring user config."""
    def git(*args, when=0):
        date = f'2026-10-01T{when // 60:02}:{when % 60:02}:00Z'
        subprocess.run(['git', *args], cwd=root, check=True, capture_output=True,
                       env={**os.environ, 'GIT_AUTHOR_NAME': 'bench', 'GIT_AUTHOR_EMAIL': 'bench@example.com',
                            'GIT_COMMITTER_NAME': 'bench', 'GIT_COMMITTER_EMAIL': 'bench@example.com',
                            'GIT_AUTHOR_DATE': date, 'GIT_COMMITTER_DATE': date,
                            'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'})
    return git


# search_symbol: the one file and line that defines the symbol.
SYMBOL_FILE = 187


def build_search_symbol(root):
    """300 modules calling `reconcile_ledger`; one defines it, one a lookalike."""
    for i in range(300):
        package = root / 'src' / f'pkg_{word(i // 30)}'
        package.mkdir(parents=True, exist_ok=True)
        lines = [f'"""Batch steps {word(i)}."""', '']
        for k in range(6):
            lines += [f'def step_{word(i)}_{word(k)}(batch):',
                      f'    return reconcile_ledger(batch, "{word(i * 6 + k)}")', '']
            if i == SYMBOL_FILE and k == 3:
                lines += ['def reconcile_ledger(batch, tag):',
                          '    """Post a batch to the ledger under `tag`."""',
                          '    return [(tag, entry) for entry in batch]', '']
            if i == 42 and k == 1:
                lines += ['def reconcile_ledger_legacy(batch):',
                          '    return reconcile_ledger(batch, "legacy")', '']
        (package / f'mod_{word(i)}.py').write_text('\n'.join(lines))


def build_git_log_find(root):
    """30 commits; one with a misleading subject flips a sign in `apply_discount`."""
    git = git_in(root)
    git('init', '-q')
    functions = [f'def rate_{word(i)}(price):\n    return round(price * {1 + i / 100:.2f}, 2)\n\n' for i in range(40)]
    functions.insert(20, 'def apply_discount(price, discount):\n    return price - discount\n\n')
    (root / 'data').mkdir()
    for c in range(30):
        # Every commit retunes one rate; from commit 17 on the discount is added.
        i = c % 40
        functions[i if i < 20 else i + 1] = f'def rate_{word(i)}(price):\n    return round(price * {1 + (i + c) / 100:.2f}, 2)\n\n'
        source = ''.join(functions)
        if c >= 17:
            source = source.replace('return price - discount', 'return price + discount')
        subject = 'Tidy pricing helpers' if c == 17 else f'Tune rates for region {word(c)}'
        (root / 'pricing.py').write_text(source)
        rows = [f'{word(r)},{(r * 7 + c * 13) % 997},{word(r + c)}' for r in range(120)]
        (root / 'data/rates.csv').write_text('region,rate,tag\n' + '\n'.join(rows) + '\n')
        git('add', '-A')
        git('commit', '-q', '-m', subject, when=c)


def listing_artifacts_layout():
    """Relative paths of the listing_artifacts repository, and its answer."""
    paths, sources = [], 0
    for p in range(4):
        package = f'src/app_{word(p)}'
        for name in ['__init__'] + [f'mod_{word(p * 10 + k)}' for k in range(9)]:
            paths.append(f'{package}/{name}.py')
            paths.append(f'{package}/__pycache__/{name}.cpython-312.pyc')
            paths.append(f'build/lib/app_{word(p)}/{name}.py')
            sources += 1
    for k in range(11):
        paths.append(f'tests/test_{word(k)}.py')
        sources += 1
    paths.append('scripts/release.py')
    sources += 1
    for k in range(150):
        paths.append(f'node_modules/pkg-{word(k)}/index.js')
        if k % 6 == 0:
            paths.append(f'node_modules/pkg-{word(k)}/gyp/pylib/gen_{word(k)}.py')
    for k in range(120):
        paths.append(f'.venv/lib/python3.12/site-packages/dep_{word(k)}/core.py')
    for k in range(200):
        paths.append(f'target/debug/deps/lib_{word(k)}.rlib')
    return paths, sources


def build_listing_artifacts(root):
    paths, _ = listing_artifacts_layout()
    for path in paths:
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(f'# {path}\n')


def build_large_file_fix(root):
    """A 50 KiB billing module whose `compute_late_fee` caps with max, not min."""
    parts = ['"""Billing rules: one small function per rule."""\n\n\n']
    for i in range(260):
        parts.append(f'def rule_{word(i)}(amount, ratio=0.{i % 9 + 1}):\n'
                     f'    """Rule {word(i)}: scale amount by ratio, then round to cents."""\n'
                     f'    scaled = amount * ratio + {i} / 100\n'
                     f'    return round(scaled, 2)\n\n\n')
        if i == 151:
            parts.append('def compute_late_fee(days_late, amount):\n'
                         '    """1.5% of amount per day late, capped at a quarter of the amount."""\n'
                         '    if days_late <= 0:\n'
                         '        return 0.0\n'
                         '    fee = amount * 0.015 * days_late\n'
                         '    return round(max(fee, amount * 0.25), 2)\n\n\n')
    (root / 'billing.py').write_text(''.join(parts))
    (root / 'test_billing.py').write_text(
        'import unittest\n\nimport billing\n\n\n'
        'class LateFeeTests(unittest.TestCase):\n'
        '    def test_on_time_costs_nothing(self):\n'
        '        self.assertEqual(billing.compute_late_fee(0, 200), 0.0)\n\n'
        '    def test_fee_grows_per_day(self):\n'
        '        self.assertEqual(billing.compute_late_fee(2, 200), 6.0)\n\n'
        '    def test_fee_is_capped(self):\n'
        '        self.assertEqual(billing.compute_late_fee(40, 200), 50.0)\n\n'
        '    def test_other_rules_unchanged(self):\n'
        '        self.assertEqual(billing.rule_a(100), 10.0)\n\n\n'
        "if __name__ == '__main__':\n    unittest.main()\n")


def build_stack_trace(root):
    """A Java service log: handled warnings with traces, then one nested failure."""
    (root / 'logs').mkdir()
    lines = []
    for i in range(3000):
        second = f'2026-10-01T09:{i // 60 % 60:02}:{i % 60:02}.{i % 1000:03}Z'
        lines.append(f'{second} INFO  [worker-{i % 6}] c.a.b.SettlementJob - settled batch {word(i)} entries={i % 50}')
        if i % 400 == 13:
            lines += [f'{second} WARN  [worker-{i % 6}] c.a.b.AccountCache - refresh retried',
                      'java.lang.NullPointerException: Cannot read field "region" because "entry" is null',
                      '\tat com.acme.billing.AccountCache.refresh(AccountCache.java:57)',
                      '\tat com.acme.billing.AccountCache.lambda$warm$0(AccountCache.java:31)',
                      '\tat java.base/java.util.concurrent.ThreadPoolExecutor.runWorker(ThreadPoolExecutor.java:1144)']
        if i == 2217:
            lines += [f'{second} ERROR [worker-3] c.a.b.SettlementJob - settlement failed for batch {word(i)}',
                      'java.lang.RuntimeException: settlement failed',
                      '\tat com.acme.billing.SettlementJob.run(SettlementJob.java:88)',
                      '\tat com.acme.billing.Scheduler.tick(Scheduler.java:142)',
                      'Caused by: com.acme.billing.LedgerException: could not post entry 7139',
                      '\tat com.acme.billing.LedgerService.post(LedgerService.java:301)',
                      '\tat com.acme.billing.SettlementJob.run(SettlementJob.java:81)',
                      '\t... 1 more',
                      'Caused by: java.lang.NullPointerException: Cannot invoke "Account.getBalance()" because "account" is null',
                      '\tat com.acme.billing.LedgerService.balanceOf(LedgerService.java:214)',
                      '\tat com.acme.billing.LedgerService.post(LedgerService.java:296)',
                      '\t... 2 more']
    (root / 'logs/service.log').write_text('\n'.join(lines) + '\n')


def json_api_issues():
    labels = ['bug', 'regression', 'regression-test', 'docs', 'ui']
    issues = []
    for i in range(200):
        chosen = [labels[j] for j in range(5) if (i * (j + 3)) % (j + 4) == 0]
        issues.append({'id': 900000 + i, 'number': i + 1, 'title': f'Issue {word(i)}' + (' (regression)' if i % 11 == 0 else ''),
                       'state': 'closed' if i % 3 == 0 else 'open',
                       'labels': [{'id': 100 + labels.index(n), 'name': n, 'color': 'ededed'} for n in chosen],
                       'user': {'login': f'user-{word(i % 17)}'}, 'comments': i % 9,
                       'created_at': f'2026-09-{i % 28 + 1:02}T10:00:00Z',
                       'body': f'Steps {word(i)}: open the page, then {"see the regression" if i % 5 == 0 else "check it"}.'})
    return issues


def build_json_api(root):
    (root / 'api').mkdir()
    (root / 'api/issues.json').write_text(json.dumps(json_api_issues(), separators=(',', ':')))


def warnings_build(root, count, error_at):
    """A rustc-like build that prints `count` warnings, each with its `-->`
    location, and one error after warning `error_at`, and fails."""
    (root / 'build.py').write_text(
        'import sys\n\n'
        'def word(i):\n    return "".join(chr(97 + int(c)) for c in str(i))\n\n'
        f'for i in range({count}):\n'
        '    print(f"warning: unused variable `x_{word(i)}`", file=sys.stderr)\n'
        '    module = "src/ledger/settled.rs" if i % 97 == 0 else f"src/mod_{word(i % 40)}.rs"\n'
        '    print(f"  --> {module}:{i % 300 + 1}:9", file=sys.stderr)\n'
        f'    if i == {error_at}:\n'
        '        print("error[E0308]: mismatched types", file=sys.stderr)\n'
        '        print("  --> src/ledger/settle.rs:88:17", file=sys.stderr)\n'
        f'print("error: could not compile `ledger` (lib) due to 1 previous error; {count} warnings emitted", file=sys.stderr)\n'
        'sys.exit(1)\n')


def build_build_warnings(root):
    """A build that prints 2,000 warnings and one error, and fails."""
    warnings_build(root, 2000, 1311)


def build_build_warnings_medium(root):
    """The same build with 300 warnings: about 18 KB, shown inline as a view."""
    warnings_build(root, 300, 211)


def build_pytest_many(root):
    """600 unittest cases, two failing at the tier boundaries of `tiered_rate`."""
    (root / 'rates.py').write_text(
        '"""Unit price by volume tier."""\n\n\n'
        'def tiered_rate(units):\n'
        '    """0.10 below 1000 units, 0.08 below 5000, else 0.05."""\n'
        '    if units <= 1000:\n'
        '        return 0.10\n'
        '    if units <= 5000:\n'
        '        return 0.08\n'
        '    return 0.05\n')
    (root / 'test_rates.py').write_text(
        'import unittest\n\nimport rates\n\n\n'
        'class TierTests(unittest.TestCase):\n    pass\n\n\n'
        'for _units in range(0, 6000, 10):\n'
        '    def _case(self, units=_units):\n'
        '        expected = 0.10 if units < 1000 else 0.08 if units < 5000 else 0.05\n'
        '        self.assertEqual(rates.tiered_rate(units), expected)\n'
        '    setattr(TierTests, f"test_units_{_units:04}", _case)\n\n\n'
        "if __name__ == '__main__':\n    unittest.main()\n")


def build_test_report_medium(root):
    """150 verbose unittest cases, one failing at the first weight tier of
    `shipping_cost`: about 11 KB of test report."""
    (root / 'shipping.py').write_text(
        '"""Shipping cost by parcel weight."""\n\n\n'
        'def shipping_cost(grams):\n'
        '    """5.00 up to 500 g, 8.00 up to 2000 g, else 12.00."""\n'
        '    if grams < 500:\n'
        '        return 5.00\n'
        '    if grams <= 2000:\n'
        '        return 8.00\n'
        '    return 12.00\n')
    (root / 'test_shipping.py').write_text(
        'import unittest\n\nimport shipping\n\n\n'
        'class ShippingTests(unittest.TestCase):\n    pass\n\n\n'
        'for _grams in range(0, 3000, 20):\n'
        '    def _case(self, grams=_grams):\n'
        '        expected = 5.00 if grams <= 500 else 8.00 if grams <= 2000 else 12.00\n'
        '        self.assertEqual(shipping.shipping_cost(grams), expected)\n'
        '    setattr(ShippingTests, f"test_grams_{_grams:04}", _case)\n\n\n'
        "if __name__ == '__main__':\n    unittest.main()\n")


# grep_context: the one charge_card call whose total subtracts a refund, and
# the charge_card_legacy lookalike that does too, as (module, function).
GREP_FILES, GREP_FUNCTIONS = 10, 20
GREP_ANSWER, GREP_DECOY = (6, 13), (3, 7)


def build_grep_context(root):
    """200 `charge_card` calls in 10 modules, each two lines below the total
    it charges: only `grep -C2` context shows which total holds a refund."""
    (root / 'src').mkdir()
    for f in range(GREP_FILES):
        lines = []
        for k in range(GREP_FUNCTIONS):
            refund = (f, k) in (GREP_ANSWER, GREP_DECOY)
            lines += [f'def pay_{word(f * GREP_FUNCTIONS + k)}(o):',
                      '    t = o.s - o.refund' if refund else f'    t = o.s + o.tax_{word(k)}',
                      f'    charge_card{"_legacy" if (f, k) == GREP_DECOY else ""}(o, t)', '']
        (root / 'src' / f'm_{word(f)}.py').write_text('\n'.join(lines))


def build_diff_medium(root):
    """An uncommitted change of about 15 KB: docstrings reworded in 20
    modules, an equivalent comparison rewritten in window.py, and one
    behavior change in quota.py."""
    vcs = git_in(root)
    vcs('init', '-q')
    (root / 'src').mkdir()

    def module(i, revised):
        parts = [f'"""Helpers for area {word(i)}."""\n\n\n']
        for k in range(6):
            doc = (f'Return the {word(k)} adjustment for area {word(i)}, rounded to cents.' if revised and k % 2 == 0
                   else f'Compute the {word(k)} adjustment of area {word(i)}.')
            parts.append(f'def adjust_{word(i)}_{word(k)}(value):\n    """{doc}"""\n'
                         f'    return round(value * {1 + (i + k) / 100:.2f}, 2)\n\n\n')
        return ''.join(parts)

    def quota(revised):
        return ('def within_quota(used, limit):\n    """Whether usage stays under its limit."""\n'
                f'    return used {"<=" if revised else "<"} limit\n')

    def window(revised):
        return ('def in_window(x, limit):\n    """Whether x falls before the window closes."""\n'
                f'    if {"limit > x" if revised else "x < limit"}:\n        return True\n    return False\n')

    def write(revised):
        for i in range(20):
            (root / 'src' / f'area_{word(i)}.py').write_text(module(i, revised))
        (root / 'src/quota.py').write_text(quota(revised))
        (root / 'src/window.py').write_text(window(revised))
    write(False)
    vcs('add', '-A'); vcs('commit', '-q', '-m', 'initial')
    write(True)


BUILDERS = {
    'search_symbol': build_search_symbol,
    'git_log_find': build_git_log_find,
    'listing_artifacts': build_listing_artifacts,
    'large_file_fix': build_large_file_fix,
    'stack_trace': build_stack_trace,
    'json_api': build_json_api,
    'build_warnings': build_build_warnings,
    'pytest_many': build_pytest_many,
    'build_warnings_medium': build_build_warnings_medium,
    'test_report_medium': build_test_report_medium,
    'grep_context': build_grep_context,
    'diff_medium': build_diff_medium,
}


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
        git = git_in(root)
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
        git('add', '-A'); git('commit', '-q', '-m', 'Bump dependencies and tidy auth', when=1)
    elif task in BUILDERS:
        BUILDERS[task](root)
    else:
        raise ValueError(task)


# --- one session ------------------------------------------------------------

# A Scopelet view or repeat reference at a line start, in any format: the
# single format's `[scopelet v5 `, and `v4` or `compact-vN` from earlier ones.
VIEW = re.compile(r'(?m)^\[scopelet (?:v\d+|compact-v\d+) ')


def views_in(content):
    """Scopelet views in a tool_result's content, as the model reads it."""
    if isinstance(content, list):
        content = ''.join(b.get('text', '') for b in content if isinstance(b, dict))
    return len(VIEW.findall(content)) if isinstance(content, str) else 0


def parse_stream(text):
    """Usage, turns, cost, answer, tool calls, views shown and served models
    of a stream-json transcript.

    Missing figures stay None: an unknown usage is never zero.
    """
    tool_calls, tools, result, models, views = 0, {}, None, set(), 0
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        if event.get('type') == 'assistant':
            message = event.get('message') or {}
            if isinstance(message.get('model'), str):
                models.add(message['model'])
            for block in message.get('content', []) or []:
                if isinstance(block, dict) and block.get('type') == 'tool_use':
                    tool_calls += 1
                    name = str(block.get('name'))
                    tools[name] = tools.get(name, 0) + 1
        elif event.get('type') == 'user':
            content = (event.get('message') or {}).get('content')
            for block in content if isinstance(content, list) else []:
                if isinstance(block, dict) and block.get('type') == 'tool_result':
                    views += views_in(block.get('content'))
        elif event.get('type') == 'result':
            result = event
    usage = (result or {}).get('usage') or {}
    pick = lambda key: usage.get(key) if isinstance(usage.get(key), int) else None
    if isinstance((result or {}).get('modelUsage'), dict):
        models.update(result['modelUsage'])
    record = {
        'input_tokens': pick('input_tokens'),
        'cache_creation_input_tokens': pick('cache_creation_input_tokens'),
        'cache_read_input_tokens': pick('cache_read_input_tokens'),
        'output_tokens': pick('output_tokens'),
        'turns': (result or {}).get('num_turns'),
        'cost_usd': (result or {}).get('total_cost_usd'),
        'tool_calls': tool_calls if result is not None else None,
        'tools': tools,
        # What the stream shows, even when it ends without a result.
        'views_shown': views,
        'models_served': sorted(models) or None,
        'answer': (result or {}).get('result'),
        'session_error': (result or {}).get('is_error') if result is not None else True,
    }
    parts = [record[k] for k in ('input_tokens', 'cache_creation_input_tokens', 'cache_read_input_tokens')]
    record['input_total'] = sum(parts) if all(p is not None for p in parts) else None
    return record


def check(spec, answer, workdir, task=None):
    """Whether the session did the task, judged on the repository and answer.

    `tests_pass` compares the files it names with a fresh build of `task`.
    """
    kind = spec['type']
    if kind == 'answer_contains':
        # `forbidden`: near misses the answer must not name.
        return (bool(answer) and spec['expected'] in answer
                and not any(wrong in answer for wrong in spec.get('forbidden', [])))
    if kind == 'answer_number':
        return bool(answer) and str(spec['expected']) in re.findall(r'\d+', answer)
    if kind == 'tests_pass':
        with tempfile.TemporaryDirectory() as temp:
            reference = Path(temp) / 'reference'
            build(task, reference)
            unchanged = all((workdir / name).read_bytes() == (reference / name).read_bytes()
                            for name in spec.get('unchanged', []))
            # Bytecode the session cached is ignored: a same-size edit in the
            # same second would otherwise run the old code.
            run = subprocess.run(spec['command'], cwd=workdir, capture_output=True, timeout=120,
                                 env={**os.environ, 'PYTHONPYCACHEPREFIX': str(Path(temp) / 'pycache')})
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
                   CODEX_HOME=str(scratch / 'codex'), OPENCODE_CONFIG_DIR=str(scratch / 'opencode'))
        subprocess.run([str(arm['binary']), 'install', '--agent', 'claude'], env=env, check=True,
                       capture_output=True, timeout=60)
    return env


def transcript_path(directory, round_, task, arm):
    return directory / f'{round_}-{task}-{arm}.jsonl.gz'


def session(arm, task, spec, model, timeout, isolate_home, transcript=None):
    """One session; its raw stream-json goes gzipped to `transcript` if given."""
    with tempfile.TemporaryDirectory(prefix=f'scopelet-live-{arm["name"]}-') as temp:
        scratch = Path(temp)
        workdir = scratch / 'repo'
        build(task, workdir)
        env = install(arm, scratch)
        if isolate_home:
            env['HOME'] = str(scratch / 'home')
            (scratch / 'home').mkdir()
        started = time.monotonic()
        stdout = ''
        try:
            # The prompt goes on stdin: `--allowedTools` takes every word after it.
            run = subprocess.run(['claude', '-p', '--model', model, '--output-format', 'stream-json', '--verbose',
                                  '--permission-mode', 'acceptEdits', f'--allowedTools={TOOLS}'],
                                 input=spec['prompt'], cwd=workdir, env=env, capture_output=True, text=True,
                                 timeout=timeout)
            stdout = run.stdout
            record = parse_stream(run.stdout)
            record['exit_code'] = run.returncode
            if run.returncode != 0:
                record['stderr'] = run.stderr[-2000:]
        except subprocess.TimeoutExpired as expired:
            stdout = expired.stdout.decode(errors='replace') if isinstance(expired.stdout, bytes) else (expired.stdout or '')
            record = parse_stream('')
            record['exit_code'] = None
            record['timed_out'] = True
        record['seconds'] = round(time.monotonic() - started, 1)
        if transcript is not None:
            with gzip.open(transcript, 'xt', encoding='utf-8') as out:
                out.write(stdout)
        try:
            record['correct'] = record['exit_code'] == 0 and check(spec['check'], record['answer'], workdir, task)
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


def cost_per_success(rows):
    """Billed cost of every run with a known cost, failures included, over the
    successes among them: (value or None without a success, runs of unknown cost)."""
    known = [r for r in rows if r.get('cost_usd') is not None]
    wins = sum(bool(r.get('correct')) for r in known)
    return (sum(r['cost_usd'] for r in known) / wins if wins else None), len(rows) - len(known)


def interval(values):
    """The 2.5th and 97.5th percentiles of `values` (nearest rank)."""
    values = sorted(values)
    if not values:
        return None
    rank = lambda q: values[min(len(values) - 1, max(0, math.ceil(q * len(values)) - 1))]
    return [rank(0.025), rank(0.975)]


def bootstrap(groups, statistic, draws=DRAWS, seed=SEED):
    """A 95% interval of `statistic` over `groups` resampled with replacement.

    Draws where the statistic is undefined are counted, not imputed.
    """
    rng = random.Random(seed)
    values, undefined = [], 0
    for _ in range(draws):
        value = statistic([groups[rng.randrange(len(groups))] for _ in groups])
        if value is None:
            undefined += 1
        else:
            values.append(value)
    return {'ci95': interval(values), 'draws': draws, 'undefined_draws': undefined}


METRICS = ('cost_usd', 'input_total', 'turns')


def paired(records, arm, baseline, tasks, draws=DRAWS):
    """`arm` against `baseline` on the (task, round) pairs both ran.

    Per metric, the mean of the pair deltas over the pairs where both are
    known; cost per success compares the pairs whose costs are both known.
    Intervals resample tasks, keeping each task's pairs together.
    """
    by_key = {}
    for r in records:
        if r['arm'] in (arm, baseline):
            by_key.setdefault((r['task'], r.get('round')), {})[r['arm']] = r
    groups = {}
    for (task, round_), pair in sorted(by_key.items(), key=lambda kv: (kv[0][0], kv[0][1] if kv[0][1] is not None else -1)):
        if arm in pair and baseline in pair and task in tasks:
            groups.setdefault(task, []).append((round_, pair[arm], pair[baseline]))
    groups = [groups[t] for t in tasks if t in groups]
    pairs = [p for g in groups for p in g]

    def deltas(sample, key):
        return [a[key] - b[key] for g in sample for _, a, b in g if a.get(key) is not None and b.get(key) is not None]

    def mean_delta(sample, key):
        values = deltas(sample, key)
        return sum(values) / len(values) if values else None

    def cps_delta(sample):
        both = [(a, b) for g in sample for _, a, b in g if a.get('cost_usd') is not None and b.get('cost_usd') is not None]
        ours, _ = cost_per_success([a for a, _ in both])
        theirs, _ = cost_per_success([b for _, b in both])
        return None if ours is None or theirs is None else ours - theirs

    result = {
        'baseline': baseline, 'pairs': len(pairs), 'tasks': len(groups),
        'correct': {arm: sum(bool(a.get('correct')) for _, a, _ in pairs),
                    baseline: sum(bool(b.get('correct')) for _, _, b in pairs)},
        'cost_per_success_delta': {'value': cps_delta(groups)},
        'mean_delta': {},
        'detail': [{'task': a['task'], 'round': round_,
                    **{key: (a[key] - b[key] if a.get(key) is not None and b.get(key) is not None else None)
                       for key in METRICS},
                    'correct': [bool(a.get('correct')), bool(b.get('correct'))]}
                   for round_, a, b in pairs],
    }
    for key in METRICS:
        known = len(deltas(groups, key))
        result['mean_delta'][key] = {'value': mean_delta(groups, key), 'pairs_known': known,
                                     'pairs_unknown': len(pairs) - known}
    if draws and groups:
        result['cost_per_success_delta'].update(bootstrap(groups, cps_delta, draws))
        for key in METRICS:
            result['mean_delta'][key].update(bootstrap(groups, lambda s, key=key: mean_delta(s, key), draws))
    return result


def summarize(records, arms, tasks, baseline=None, draws=DRAWS):
    """Per task and arm, and over all tasks per arm. Failures stay counted.

    With a `baseline` arm, `paired` compares every other arm with it; `draws`
    of 0 skips the bootstrap (progress reports between sessions).
    """
    def cell(rows):
        cps, unknown = cost_per_success(rows)
        known = [r['cost_usd'] for r in rows if r.get('cost_usd') is not None]
        return {'runs': len(rows), 'correct': sum(bool(r.get('correct')) for r in rows),
                'with_usage': sum(r.get('input_total') is not None for r in rows),
                'total_cost_usd': sum(known) if known else None, 'cost_unknown': unknown,
                'cost_per_success': cps,
                **{f'median_{key}': median(r.get(key) for r in rows)
                   for key in ('input_total', 'input_tokens', 'cache_creation_input_tokens', 'cache_read_input_tokens',
                               'output_tokens', 'turns', 'tool_calls', 'cost_usd', 'seconds')}}
    summary = {task: {arm: cell([r for r in records if r['task'] == task and r['arm'] == arm]) for arm in arms}
               for task in tasks}
    summary['all'] = {arm: cell([r for r in records if r['arm'] == arm]) for arm in arms}
    if baseline in arms:
        summary['paired'] = {arm: paired(records, arm, baseline, tasks, draws) for arm in arms if arm != baseline}
    return summary


def parse_arm(text):
    """`off`, or `NAME=BINARY`: that binary's hooks in its default format."""
    if text == 'off':
        return {'name': 'off', 'binary': None}
    name, _, binary = text.partition('=')
    if not re.fullmatch(r'[A-Za-z0-9_.]+', name) or not binary or '@' in binary:
        raise argparse.ArgumentTypeError('expected off or NAME=BINARY')
    return {'name': name, 'binary': Path(binary).resolve()}


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--live', action='store_true', help='required: run real, billed model sessions')
    p.add_argument('--arm', action='append', type=parse_arm, help='off, or NAME=BINARY; repeatable')
    p.add_argument('--task', action='append', help='task name from fixtures/live/tasks.json; default all')
    p.add_argument('--runs', type=int, default=3, help='paired rounds')
    p.add_argument('--model', default='haiku')
    p.add_argument('--baseline', default='v4old', help='arm the others are paired against (default v4old)')
    p.add_argument('--transcripts', type=Path, help='directory for each session\'s gzipped stream-json')
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
    if args.transcripts:
        args.transcripts.mkdir(parents=True, exist_ok=True)
    report = {'kind': 'live Claude Code sessions; tokens and cost from the session result, not estimates',
              'model': args.model, 'runs': args.runs, 'tasks': selected, 'baseline': args.baseline,
              'claude_version': subprocess.run(['claude', '--version'], capture_output=True, text=True).stdout.strip(),
              'arms': [{'name': a['name'],
                        'binary_sha256': hashlib.sha256(a['binary'].read_bytes()).hexdigest() if a['binary'] else None}
                       for a in arms],
              'records': []}
    out.parent.mkdir(parents=True, exist_ok=True)
    for round_ in range(args.runs):
        order = arms[round_ % len(arms):] + arms[:round_ % len(arms)]
        for task in selected:
            for arm in order:
                transcript = transcript_path(args.transcripts, round_, task, arm['name']) if args.transcripts else None
                record = session(arm, task, tasks[task], args.model, args.timeout, args.isolate_home, transcript)
                record['round'] = round_
                report['records'].append(record)
                print(f"round {round_} {task} {arm['name']}: correct={record['correct']} cost={record['cost_usd']} "
                      f"input_total={record['input_total']} turns={record['turns']} tools={record['tool_calls']} "
                      f"views={record['views_shown']}",
                      flush=True)
                report['summary'] = summarize(report['records'], names, selected, args.baseline, draws=0)
                out.write_text(json.dumps(report, indent=2) + '\n')
    report['summary'] = summarize(report['records'], names, selected, args.baseline)
    out.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'all': report['summary']['all'],
                      'paired': {k: {kk: vv for kk, vv in v.items() if kk != 'detail'}
                                 for k, v in report['summary'].get('paired', {}).items()}}, indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
