#!/usr/bin/env python3
"""Next-action preservation: does a compressed view change what the agent does next?

After CoACT: replay a recorded session up to a large tool output, show the
model that output in several forms, and compare the action it takes next. No
trained model is needed, only the transcripts `live.py --transcripts` keeps.

For every transcript of the `off` arm (raw outputs), each Bash or Grep result
is put through the real hook (`scopelet hook claude` on a simulated
`PostToolUse`, after the task prompt as `UserPromptSubmit`, so the command's
profile and the prompt's anchors apply) under each condition: raw, compact-v4,
compact-v5 with its grammar only (`SCOPELET_V5_GRAMMAR=labels,header,tables`)
and compact-v5 whole. A step counts when at least one condition changed the
output, which only happens above its profile's exact-size threshold.

The conversation up to that output is rebuilt (task prompt, assistant turns
without their thinking blocks, earlier results as recorded) and sent to
`POST /v1/messages` with a short agent prompt and Bash, Read, Edit, Grep,
Glob and Write tools, with the shared prefix cached. The model's sampling
cannot be set, so each condition is drawn several times. An action is
decomposed into its tool, program, paths, pattern and line range, or a final
answer judged by the task's own check. A condition's NAP at a step is the
share of its draws equal to the most frequent raw action; raw's own share is
the noise floor. A coarse NAP ignores the pattern and line range, which vary
between draws even on raw output. Paths are taken relative to the session's
directory. A step whose shown text is not the tool response the hook would
receive (a Grep the host cut) is skipped and listed. Input tokens are the
API's exact usage. A failed call is
null with its error, never zero, and stays in the report.

Calls happen only with `--live`; the key is read from `ANTHROPIC_API_KEY`
and never written or printed. The output directory must not exist.
"""
from __future__ import annotations
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

import live

HERE = Path(__file__).resolve().parent
ENDPOINT = 'https://api.anthropic.com/v1/messages'
MODEL = 'claude-haiku-5-5'
DRAWS = 3
# (name, compact version or None for the raw output, SCOPELET_V5_GRAMMAR or None)
CONDITIONS = [('raw', None, None), ('v4', '4', None), ('v5g', '5', 'labels,header,tables'), ('v5', '5', None)]
SYSTEM = ('You are a coding agent working in a repository. Use the tools to complete the user\'s task, '
          'one step at a time. When the task is done, reply with the final answer only.')
TOOLS = [
    {'name': 'Bash', 'description': 'Run a shell command in the repository and return its output.',
     'input_schema': {'type': 'object', 'properties': {'command': {'type': 'string'},
                                                       'description': {'type': 'string'}},
                      'required': ['command']}},
    {'name': 'Read', 'description': 'Read a file, optionally from a line offset and for a number of lines.',
     'input_schema': {'type': 'object', 'properties': {'file_path': {'type': 'string'}, 'offset': {'type': 'integer'},
                                                       'limit': {'type': 'integer'}},
                      'required': ['file_path']}},
    {'name': 'Edit', 'description': 'Replace an exact string in a file.',
     'input_schema': {'type': 'object', 'properties': {'file_path': {'type': 'string'}, 'old_string': {'type': 'string'},
                                                       'new_string': {'type': 'string'}, 'replace_all': {'type': 'boolean'}},
                      'required': ['file_path', 'old_string', 'new_string']}},
    {'name': 'Grep', 'description': 'Search file contents with a regular expression.',
     'input_schema': {'type': 'object', 'properties': {'pattern': {'type': 'string'}, 'path': {'type': 'string'},
                                                       'glob': {'type': 'string'}, 'output_mode': {'type': 'string'},
                                                       '-n': {'type': 'boolean'}, '-i': {'type': 'boolean'}},
                      'required': ['pattern']}},
    {'name': 'Glob', 'description': 'List files matching a glob pattern.',
     'input_schema': {'type': 'object', 'properties': {'pattern': {'type': 'string'}, 'path': {'type': 'string'}},
                      'required': ['pattern']}},
    {'name': 'Write', 'description': 'Write a file.',
     'input_schema': {'type': 'object', 'properties': {'file_path': {'type': 'string'}, 'content': {'type': 'string'}},
                      'required': ['file_path', 'content']}},
]
TOOL_NAMES = {t['name'] for t in TOOLS}


# --- transcripts --------------------------------------------------------------

def parse_name(path):
    """(round, task, arm) of `ROUND-TASK-ARM.jsonl.gz`."""
    stem = Path(path).name.removesuffix('.jsonl.gz')
    round_, _, rest = stem.partition('-')
    task, _, arm = rest.rpartition('-')
    return int(round_), task, arm


def text_of(content):
    """A tool_result's content as the model reads it."""
    if isinstance(content, str):
        return content
    return ''.join(b.get('text', '') for b in content or [] if isinstance(b, dict))


def rebuild(events, prompt):
    """The API messages of a stream-json transcript, and its replayable steps.

    Assistant events sharing a message id merge into one message; thinking
    blocks are dropped (they are bound to the session that produced them).
    Consecutive user events merge, so parallel results stay in one message.
    A step is a Bash or Grep result with its raw tool response.
    """
    messages = [{'role': 'user', 'content': prompt}]
    steps, last_id, uses = [], None, {}
    for event in events:
        kind = event.get('type')
        message = event.get('message') or {}
        if kind == 'assistant':
            blocks = []
            for block in message.get('content') or []:
                if block.get('type') == 'text' and block.get('text'):
                    blocks.append({'type': 'text', 'text': block['text']})
                elif block.get('type') == 'tool_use':
                    blocks.append({'type': 'tool_use', 'id': block['id'], 'name': block['name'], 'input': block['input']})
                    uses[block['id']] = block
            if not blocks:
                continue
            if messages[-1]['role'] == 'assistant' and message.get('id') == last_id:
                messages[-1]['content'].extend(blocks)
            else:
                messages.append({'role': 'assistant', 'content': blocks})
            last_id = message.get('id')
        elif kind == 'user' and isinstance(message.get('content'), list):
            results = [b for b in message['content'] if isinstance(b, dict) and b.get('type') == 'tool_result']
            if not results:
                continue
            if messages[-1]['role'] != 'user':
                messages.append({'role': 'user', 'content': []})
            for block in results:
                messages[-1]['content'].append({'type': 'tool_result', 'tool_use_id': block['tool_use_id'],
                                                'content': text_of(block.get('content')),
                                                **({'is_error': True} if block.get('is_error') else {})})
                use = uses.get(block['tool_use_id'])
                response = event.get('tool_use_result')
                if use and use['name'] in ('Bash', 'Grep') and isinstance(response, dict):
                    steps.append({'message': len(messages) - 1, 'block': len(messages[-1]['content']) - 1,
                                  'tool': use['name'], 'input': use['input'], 'response': response})
            last_id = None
    # A step needs the action that follows it.
    return messages, [s for s in steps if s['message'] + 1 < len(messages)]


# --- views through the real hook -----------------------------------------------

def composed(tool, response):
    """The text a tool response puts in the model's tool_result."""
    if tool == 'Grep':
        return response.get('content') if isinstance(response.get('content'), str) else None
    if not isinstance(response.get('stdout'), str):
        return None
    return '\n'.join(x for x in (response['stdout'].rstrip('\n'), (response.get('stderr') or '').rstrip('\n')) if x)


def hook_view(binary, step, version, parts, prompt, run=subprocess.run):
    """The tool_result text a session of this condition would have shown."""
    with tempfile.TemporaryDirectory(prefix='scopelet-nap-') as temp:
        env = {k: v for k, v in os.environ.items() if not k.startswith('SCOPELET_')}
        env.update(SCOPELET_CONFIG_DIR=str(Path(temp) / 'config'), SCOPELET_CACHE_DIR=str(Path(temp) / 'cache'),
                   SCOPELET_COMPACT_VERSION=version, SCOPELET_EVENTS='0')
        if parts is not None:
            env['SCOPELET_V5_GRAMMAR'] = parts
        session = str(uuid.uuid4())
        hook = lambda event: run([str(binary), 'hook', 'claude'], input=json.dumps(event), capture_output=True,
                                 text=True, env=env, timeout=60)
        hook({'hook_event_name': 'UserPromptSubmit', 'session_id': session, 'prompt': prompt})
        out = hook({'hook_event_name': 'PostToolUse', 'session_id': session, 'tool_name': step['tool'],
                    'tool_input': step['input'], 'tool_response': step['response']})
        if out.returncode != 0:
            raise RuntimeError(f'hook exit {out.returncode}: {out.stderr[-300:]}')
        try:
            updated = json.loads(out.stdout)['hookSpecificOutput']['updatedToolOutput']
        except (json.JSONDecodeError, KeyError, TypeError):
            return None
        return composed(step['tool'], {**step['response'], **updated})


def views(binary, step, raw_text, prompt):
    """Each condition's tool_result text; raw is the recorded one."""
    original = composed(step['tool'], step['response'])
    result = {}
    for name, version, parts in CONDITIONS:
        if version is None:
            result[name] = raw_text
            continue
        view = hook_view(binary, step, version, parts, prompt)
        if view is None or original is None:
            result[name] = raw_text
        elif original in raw_text:
            result[name] = raw_text.replace(original, view, 1)
        else:
            result[name] = view
    return result


# --- actions ------------------------------------------------------------------

SEARCHERS = {'grep', 'egrep', 'fgrep', 'rg', 'ag', 'ack'}
PATH = re.compile(r'(^|/)[\w.-]+\.[A-Za-z0-9]{1,6}$|/')


def words(command):
    try:
        return shlex.split(command)
    except ValueError:
        return command.split()


def relative(path, cwd):
    """A path as seen from the session's directory: absolute paths under it
    and `./` prefixes name the same file as the bare relative path."""
    path = path.rstrip('/')
    if cwd:
        for root in {cwd.rstrip('/'), '/private' + cwd.rstrip('/')}:
            if path.startswith(root + '/'):
                path = path[len(root) + 1:]
    return path.removeprefix('./') or '.'


def bash_action(command, cwd=None):
    """Program, paths, pattern and line range of a shell command."""
    segments = [s for s in re.split(r'\s*(?:\|\||&&|;|\|)\s*', command) if s.strip()]
    program, pattern, span = None, None, None
    for segment in segments:
        tokens = [t for t in words(segment) if not re.fullmatch(r'[A-Z_][A-Z0-9_]*=.*', t)]
        if not tokens or tokens[0] in ('cd', 'echo', 'true', 'set'):
            continue
        name = Path(tokens[0]).name
        if program is None:
            program = name if name != 'git' or len(tokens) < 2 else f'git {tokens[1]}'
        args = [t for t in tokens[1:] if not t.startswith('-')]
        if name in SEARCHERS and pattern is None and args:
            pattern = args[0]
        if name == 'git' and pattern is None:
            for flag in ('-S', '-G', '--grep'):
                if flag in tokens[:-1]:
                    pattern = tokens[tokens.index(flag) + 1]
        if name == 'sed' and span is None:
            m = re.search(r"(\d+)\s*,\s*(\d+)\s*p", segment)
            span = f'{m.group(1)}-{m.group(2)}' if m else None
        if name in ('head', 'tail') and span is None:
            m = re.search(r'-n\s*\+?(\d+)|-(\d+)\b', segment)
            span = f'{name}:{m.group(1) or m.group(2)}' if m else name
    paths = sorted({relative(t, cwd) for s in segments for t in words(s)[1:]
                    if not t.startswith('-') and not t.startswith('http') and t != pattern and PATH.search(t)})
    return program, paths, pattern, span


def action(content, check=None, cwd=None):
    """A comparable form of the assistant's next step: its first tool call,
    or its final answer judged by `check` (None when that cannot be judged).
    Paths are relative to the session's directory `cwd`."""
    for block in content:
        if block.get('type') != 'tool_use':
            continue
        tool, data = block.get('name'), block.get('input') or {}
        if tool == 'Bash':
            program, paths, pattern, span = bash_action(str(data.get('command', '')), cwd)
        else:
            program = None
            paths = sorted(relative(str(data[k]), cwd) for k in ('file_path', 'path') if data.get(k))
            pattern = data.get('pattern')
            span = f'{data.get("offset")}+{data.get("limit")}' if data.get('offset') or data.get('limit') else None
        return {'kind': 'tool', 'tool': tool, 'program': program, 'paths': paths, 'pattern': pattern, 'range': span}
    text = ''.join(b.get('text', '') for b in content if b.get('type') == 'text')
    return {'kind': 'final', 'correct': check(text) if check else None}


def key(act, coarse=False):
    """An action's identity. `coarse` ignores the search pattern and line
    range, which vary from draw to draw even on the raw output."""
    if act is None:
        return None
    if coarse:
        act = {k: v for k, v in act.items() if k not in ('pattern', 'range')}
    return json.dumps(act, sort_keys=True)


def nap(draws, reference, coarse=False):
    """Share of the successful draws equal to `reference`, or None."""
    known = [d['action'] for d in draws if d.get('action') is not None]
    return sum(key(a, coarse) == reference for a in known) / len(known) if known else None


# --- the API ------------------------------------------------------------------

class ApiClient:
    """One Messages call; rate limits and overload are retried."""

    def __init__(self, key, model=MODEL, retries=6):
        self.key, self.model, self.retries = key, model, retries

    def __call__(self, messages):
        body = json.dumps({'model': self.model, 'max_tokens': 4096, 'system': SYSTEM, 'tools': TOOLS,
                           'messages': messages}).encode()
        for attempt in range(self.retries + 1):
            request = urllib.request.Request(ENDPOINT, data=body, method='POST', headers={
                'x-api-key': self.key, 'anthropic-version': '2023-06-01', 'content-type': 'application/json'})
            try:
                with urllib.request.urlopen(request, timeout=300) as response:
                    return json.load(response)
            except urllib.error.HTTPError as error:
                if error.code not in (429, 500, 502, 503, 529) or attempt == self.retries:
                    raise RuntimeError(f'HTTP {error.code}: {error.read().decode(errors="replace")[:300]}') from None
                time.sleep(float(error.headers.get('retry-after') or 2 ** attempt))
            except (urllib.error.URLError, TimeoutError) as error:
                if attempt == self.retries:
                    raise RuntimeError(f'network: {error}') from None
                time.sleep(2 ** attempt)
        raise AssertionError('unreachable')


def replay(messages, step, text):
    """The conversation up to `step`, its result shown as `text`, with the
    prefix every condition shares marked for caching."""
    prefix = json.loads(json.dumps(messages[:step['message']]))
    prefix[-1]['content'][-1]['cache_control'] = {'type': 'ephemeral'}
    last = json.loads(json.dumps(messages[step['message']]))
    last['content'][step['block']]['content'] = text
    return prefix + [last]


USAGE = ('input_tokens', 'cache_creation_input_tokens', 'cache_read_input_tokens', 'output_tokens')


def draw(client, conversation, check, cwd=None):
    try:
        response = client(conversation)
    except Exception as error:  # noqa: BLE001 - a failure is reported, not fatal
        return {'action': None, 'error': str(error), **{k: None for k in USAGE}, 'input_total': None}
    usage = response.get('usage') or {}
    pick = {k: usage.get(k) if isinstance(usage.get(k), int) else None for k in USAGE}
    total = [pick[k] for k in USAGE[:3]]
    record = {**pick, 'input_total': sum(total) if None not in total else None,
              'stop_reason': response.get('stop_reason')}
    if response.get('stop_reason') in ('refusal', 'max_tokens'):
        return {**record, 'action': None, 'error': f'stop_reason {response.get("stop_reason")}'}
    return {**record, 'action': action(response.get('content') or [], check, cwd), 'error': None}


def measure_step(client, messages, step, texts, check, draws, cwd=None):
    """Every draw of every condition; raw's first draw writes the cache."""
    conversations = {name: replay(messages, step, texts[name]) for name, _, _ in CONDITIONS}
    results = {name: [] for name in conversations}
    results['raw'].append(draw(client, conversations['raw'], check, cwd))
    for i in range(draws):
        for name in conversations:
            if name == 'raw' and i == 0:
                continue
            results[name].append(draw(client, conversations[name], check, cwd))
    return results


def score(results, coarse=False):
    """NAP per condition against raw's most frequent action, and raw's own."""
    actions = [key(d['action'], coarse) for d in results['raw'] if d.get('action') is not None]
    modal = Counter(actions).most_common(1)[0][0] if actions else None
    return modal, {name: (nap(draws, modal, coarse) if modal is not None else None) for name, draws in results.items()}


def summarize(steps):
    names = [name for name, _, _ in CONDITIONS]
    scored = [s for s in steps if all(s['nap'][n] is not None for n in names)]
    mean = lambda values: sum(values) / len(values) if values else None
    summary = {'steps': len(steps), 'steps_scored': len(scored), 'conditions': {}}
    for name in names:
        draws = [d for s in steps for d in s['draws'][name]]
        totals = [d['input_total'] for d in draws if d['input_total'] is not None]
        summary['conditions'][name] = {
            'mean_nap': mean([s['nap'][name] for s in scored]),
            'mean_nap_coarse': mean([s['nap_coarse'][name] for s in scored if s['nap_coarse'][name] is not None]),
            'view_bytes': sum(s['view_bytes'][name] for s in scored),
            'mean_input_total': mean(totals), 'draws': len(draws),
            'errors': sum(d['error'] is not None for d in draws),
        }
    summary['by_task'] = {}
    for task in sorted({s['task'] for s in scored}):
        rows = [s for s in scored if s['task'] == task]
        summary['by_task'][task] = {'steps': len(rows), **{n: mean([s['nap'][n] for s in rows]) for n in names},
                                    'coarse': {n: mean([s['nap_coarse'][n] for s in rows]) for n in names}}
    return summary


def run(binary, transcripts, client, out, arm='off', draws=DRAWS, workers=4):
    """Replay every step of the `arm` transcripts; write `out/report.json`."""
    out.mkdir(parents=True)  # never an existing directory
    tasks = live.load_tasks()
    report = {'kind': 'next-action preservation; tokens from the API usage, not estimates', 'model': getattr(client, 'model', None),
              'draws': draws, 'conditions': [{'name': n, 'version': v, 'parts': p} for n, v, p in CONDITIONS],
              'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(), 'steps': [], 'skipped': []}
    jobs = []
    for path in sorted(Path(transcripts).glob(f'*-{arm}.jsonl.gz')):
        round_, task, _ = parse_name(path)
        if task not in tasks:
            report['skipped'].append({'transcript': path.name, 'reason': 'unknown task'})
            continue
        prompt = tasks[task]['prompt']
        spec = tasks[task]['check']
        check = None if spec['type'] == 'tests_pass' else (lambda text, spec=spec: live.check(spec, text, Path('.')))
        with gzip.open(path, 'rt', encoding='utf-8') as f:
            events = [json.loads(line) for line in f if line.strip().startswith('{')]
        cwd = next((e.get('cwd') for e in events if e.get('type') == 'system' and e.get('cwd')), None)
        messages, steps = rebuild(events, prompt)
        for index, step in enumerate(steps):
            raw_text = messages[step['message']]['content'][step['block']]['content']
            original = composed(step['tool'], step['response'])
            if step['response'].get('persistedOutputPath') or step['response'].get('persistedOutputSize'):
                # The host saved it and showed a preview; the hook passes
                # such a result through, so every arm shows the same preview.
                report['skipped'].append({'transcript': path.name, 'step': index, 'tool': step['tool'],
                                          'reason': 'persisted by the host: every arm shows its preview'})
                continue
            if original is None or original not in raw_text:
                # The host showed something else than the response the hook
                # receives (a Grep cut to its head limit): not comparable.
                report['skipped'].append({'transcript': path.name, 'step': index, 'tool': step['tool'],
                                          'reason': 'shown text is not the tool response'})
                continue
            texts = views(binary, step, raw_text, prompt)
            if all(texts[n] == raw_text for n, _, _ in CONDITIONS):
                continue
            jobs.append((path.name, round_, task, index, messages, step, texts, check, cwd))

    def one(job):
        name, round_, task, index, messages, step, texts, check, cwd = job
        results = measure_step(client, messages, step, texts, check, draws, cwd)
        modal, scores = score(results)
        _, coarse = score(results, coarse=True)
        command = step['input'].get('command') if step['tool'] == 'Bash' else step['input'].get('pattern')
        return {'transcript': name, 'round': round_, 'task': task, 'step': index, 'tool': step['tool'],
                'command': command, 'view_bytes': {n: len(t.encode()) for n, t in texts.items()},
                'raw_action': json.loads(modal) if modal else None, 'nap': scores, 'nap_coarse': coarse,
                'draws': results}

    with ThreadPoolExecutor(max_workers=workers) as pool:
        for row in pool.map(one, jobs):
            report['steps'].append(row)
            print(f"{row['transcript']} step {row['step']} {row['tool']}: " +
                  ' '.join(f'{n}={row["nap"][n]}' for n, _, _ in CONDITIONS), flush=True)
            report['summary'] = summarize(report['steps'])
            (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    report['summary'] = summarize(report['steps'])
    (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--live', action='store_true', help='required: run real, billed model calls')
    p.add_argument('--transcripts', type=Path, required=True, help='directory written by live.py --transcripts')
    p.add_argument('--scopelet', type=Path, default=HERE.parent / 'target/release/scopelet')
    p.add_argument('--arm', default='off', help='transcripts of this arm (raw outputs); default off')
    p.add_argument('--model', default=MODEL)
    p.add_argument('--draws', type=int, default=DRAWS)
    p.add_argument('--workers', type=int, default=4)
    p.add_argument('--out', type=Path, required=True, help='new directory; completed evidence is never overwritten')
    args = p.parse_args(argv)
    if not args.live:
        print('nap.py makes real, billed model calls; pass --live to run it.', file=sys.stderr)
        return 2
    key = os.environ.get('ANTHROPIC_API_KEY')
    if not key:
        print('ANTHROPIC_API_KEY is not set.', file=sys.stderr)
        return 2
    if args.out.exists():
        p.error(f'{args.out} exists; completed evidence is never overwritten')
    report = run(args.scopelet.resolve(), args.transcripts, ApiClient(key, args.model), args.out, args.arm,
                 args.draws, args.workers)
    print(json.dumps(report['summary'], indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
