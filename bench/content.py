#!/usr/bin/env python3
"""Offline shared-content probes. No provider calls; Headroom ML is disabled."""
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
import tempfile
import time


def fixtures():
    noise = ''.join(f'progress {i:04d}: ' + 'unchanged ' * 12 + '\n' for i in range(1000))
    rows = [{'id': i, 'status': 'failed' if i == 643 else 'passed',
             'message': 'error: audit-7139 amount=47' if i == 643 else 'stable ' * 12,
             'exact': 900719925474099312345, 'nullable': None} for i in range(1000)]
    return {
        'small': (b'No error: operation completed.\r\n', ['No error: operation completed.']),
        'diagnostics': ((noise + 'error: audit-7139 amount=47\nwarning: do NOT disable validation\n').encode(),
                        ['error: audit-7139 amount=47', 'warning: do NOT disable validation']),
        'repeated_errors': (('start\n' + 'error: retry failed\nworking\n' * 1000 + 'fatal error: rare root cause\nend\n').encode(),
                            ['fatal error: rare root cause']),
        'json': (json.dumps(rows, separators=(',', ':')).encode(), ['error: audit-7139 amount=47', '900719925474099312345']),
        'jsonl': ('\n'.join(json.dumps(r, separators=(',', ':')) for r in rows).encode(),
                  ['error: audit-7139 amount=47', '900719925474099312345']),
        'hidden_receipt': ((noise[:len(noise)//2] + 'receipt audit-7139 amount=47 EUR' + ' padding' * 20 + '\n' + noise[len(noise)//2:]).encode(),
                           ['receipt audit-7139 amount=47 EUR']),
        'giant_unicode_line': (('é' * 6000).encode(), ['é' * 6000]),
        'crlf': ((noise.replace('\n', '\r\n') + 'error: original CRLF survives\r\n').encode(),
                 ['error: original CRLF survives']),
    }


def recovery(binary, cache, output, original):
    match = re.search(rb'artifact:[a-f0-9]{64}', output)
    if not match:
        return output == original, None
    manifest = subprocess.run([str(binary), '--cache-dir', str(cache), 'expand', match[0].decode(), '--manifest'],
                             capture_output=True, timeout=30)
    if manifest.returncode != 0:
        return False, None
    snapshots = json.loads(manifest.stdout)['snapshots']
    if len(snapshots) != 1:
        return False, None
    result = subprocess.run([str(binary), '--cache-dir', str(cache), 'expand', snapshots[0]['blob'], '--raw'],
                            capture_output=True, timeout=30)
    return result.returncode == 0 and result.stdout == original, result.stdout


def measure(args):
    # Import only after explicit environment isolation; the library can create local stores.
    from headroom import compress
    from headroom.cache.compression_store import get_compression_store
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'kind': 'offline shared content, not session tokens or API cost', 'repetitions': args.repetitions,
              'warmups': args.warmups, 'headroom_mode': 'public compress API; protect_recent=0, protect_analysis_context=false, kompress_model=disabled',
              'rtk_mode': 'read default full content; test wrapper separately on synthetic failed command',
              'timing_note': 'Scopelet/RTK elapsed includes CLI startup; Headroom uses an imported library. Do not rank these as equivalent engine latency.',
              'cache_note': 'cold clears tool application state; OS cache and loaded Headroom pipeline are not reset',
              'binary_sha256': {k: hashlib.sha256(getattr(args, k).read_bytes()).hexdigest() for k in ('scopelet', 'rtk')}, 'cases': {}}
    with tempfile.TemporaryDirectory(prefix='scopelet-content-') as tmp:
        root = Path(tmp)
        for name, (original, facts) in fixtures().items():
            source = root / (name + ('.json' if name == 'json' else '.txt'))
            source.write_bytes(original)
            report['cases'][name] = {'input_bytes': len(original), 'sha256': hashlib.sha256(original).hexdigest(), 'facts': facts if name != 'giant_unicode_line' else ['entire giant line'], 'arms': {}}
            for state in ('cold', 'warm'):
                for arm in ('scopelet', 'rtk', 'headroom'):
                    cache = root / f'{arm}-{state}'
                    samples = []
                    output = b''
                    transforms = []
                    for rep in range(-args.warmups, args.repetitions):
                        if state == 'cold':
                            shutil.rmtree(cache, ignore_errors=True)
                            if arm == 'headroom':
                                get_compression_store().clear()
                        cache.mkdir(exist_ok=True)
                        env = {**os.environ, 'SCOPELET_CACHE_DIR': str(cache), 'RTK_DB_PATH': str(cache / 'rtk.db'),
                               'RTK_RECALL_DB': str(cache / 'recall.db'), 'RTK_TELEMETRY_DISABLED': '1', 'RTK_HOOK_AUDIT': '0'}
                        started = time.perf_counter()
                        if arm == 'headroom':
                            messages = [{'role': 'user', 'content': 'Find the failing diagnostic or requested receipt and preserve exact values.'},
                                        {'role': 'assistant', 'tool_calls': [{'id': 'bench', 'type': 'function', 'function': {'name': 'read_file', 'arguments': '{}'}}]},
                                        {'role': 'tool', 'tool_call_id': 'bench', 'content': original.decode()}]
                            result = compress(messages, model='gpt-4o', protect_recent=0, protect_analysis_context=False, kompress_model='disabled')
                            output = result.messages[-1]['content'].encode()
                            exit_code = 0
                            transforms = result.transforms_applied
                        else:
                            command = [str(args.scopelet), '--cache-dir', str(cache), 'compress'] if arm == 'scopelet' else [str(args.rtk), 'read', str(source)]
                            result = subprocess.run(command, input=original if arm == 'scopelet' else None, env=env, capture_output=True, timeout=60)
                            output, exit_code = result.stdout, result.returncode
                        elapsed = time.perf_counter() - started
                        if rep >= 0:
                            samples.append({'seconds': elapsed, 'output_bytes': len(output), 'exit_code': exit_code,
                                            'sha256': hashlib.sha256(output).hexdigest()})
                    visible = [fact in output.decode(errors='replace') for fact in facts]
                    recovered = None
                    if arm == 'scopelet':
                        exact, recovered = recovery(args.scopelet, cache, output, original)
                    elif arm == 'rtk':
                        exact = output == original
                    else:
                        # Only claim original recovery when a real CCR entry returns the original.
                        markers = re.findall(r'<<ccr:([a-f0-9]{12,24})[^>]*>>', output.decode())
                        entries = [get_compression_store().retrieve(m) for m in markers]
                        recovered = next((e.original_content.encode() for e in entries if e and isinstance(e.original_content, str) and e.original_content.encode() == original), None)
                        exact = output == original or recovered == original
                    times = sorted(s['seconds'] for s in samples)
                    report['cases'][name]['arms'][f'{arm}/{state}'] = {
                        'samples': samples, 'output_bytes': len(output), 'byte_reduction_percent': 100 * (1 - len(output) / len(original)),
                        'median_seconds': statistics.median(times), 'p95_seconds': times[max(0, __import__('math').ceil(.95 * len(times)) - 1)],
                        'failures': sum(s['exit_code'] != 0 for s in samples), 'facts_visible': visible,
                        'facts_available_after_recovery': [v or bool(recovered and fact.encode() in recovered) for v, fact in zip(visible, facts)],
                        'original_byte_roundtrip_verified': exact, 'transforms': transforms}
                    (out / f'{name}-{arm}-{state}.output').write_bytes(output)
            print(name, 'complete', flush=True)
            (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        # RTK's generic test filter has different semantics from read: test it explicitly.
        original, facts = fixtures()['diagnostics']
        script = root / 'failed.py'
        script.write_text('import sys\nsys.stdout.buffer.write(' + repr(original) + ')\nsys.exit(7)\n')
        env = {**os.environ, 'RTK_DB_PATH': str(root / 'rtk-test.db'), 'RTK_RECALL_DB': str(root / 'recall-test.db'), 'RTK_TELEMETRY_DISABLED': '1'}
        result = subprocess.run([str(args.rtk), 'test', 'python3', str(script)], env=env, capture_output=True, timeout=30)
        report['rtk_test_probe'] = {'exit_code': result.returncode, 'expected_exit_code': 7, 'input_bytes': len(original),
                                    'output_bytes': len(result.stdout), 'facts_visible': [f.encode() in result.stdout for f in facts]}
        (out / 'rtk-test.output').write_bytes(result.stdout)
    (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--scopelet', type=Path, required=True)
    p.add_argument('--rtk', type=Path, required=True)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--warmups', type=int, default=5)
    p.add_argument('--repetitions', type=int, default=30)
    args = p.parse_args()
    if args.repetitions < 1 or args.warmups < 0:
        p.error('positive repetitions and nonnegative warmups required')
    args.scopelet, args.rtk = args.scopelet.resolve(), args.rtk.resolve()
    with tempfile.TemporaryDirectory(prefix='scopelet-headroom-content-') as state:
        os.environ.update(HEADROOM_WORKSPACE_DIR=state, HEADROOM_CONFIG_DIR=state, HEADROOM_TELEMETRY='off',
                          HEADROOM_BEACON='off', HF_HUB_OFFLINE='1')
        measure(args)


if __name__ == '__main__':
    main()
