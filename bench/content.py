#!/usr/bin/env python3
"""Offline content probes for one Scopelet binary. No model calls.

Each fixture is a realistic command output with one or two facts an agent must
not lose. The probe runs `scopelet compress` on it, records bytes in and out,
timing with a cold and a warm application cache, whether every fact is still
visible in the view, and whether the original bytes come back through the
artifact manifest. `tests/content_gate.rs` pins the same fixtures by SHA-256
and rejects a compressor change that loses a fact or a byte.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import tempfile
import time


# Facts expected only after recovery: the view shows a cut line with a marker.
RECOVERY_ONLY = {'giant_unicode_line'}


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
    """Follow the view's artifact to its manifest, then to the source blob.

    Returns (exact, recovered): whether the original bytes came back exactly, and
    the bytes that were read. Raw artifact JSON is never compared with the source.
    """
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


def summarize(samples):
    seconds = sorted(s['seconds'] for s in samples)
    return {'median_seconds': statistics.median(seconds),
            'p95_seconds': seconds[max(0, math.ceil(.95 * len(seconds)) - 1)],
            'failures': sum(s['exit_code'] != 0 for s in samples), 'samples': len(samples)}


def measure(binary, out, warmups, repetitions):
    out = out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {'kind': 'offline content probe; bytes and wall time, not model tokens or cost',
              'platform': platform.platform(), 'repetitions': repetitions, 'warmups': warmups,
              'timing_note': 'elapsed includes process startup; cold clears the application cache, not the OS page cache',
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'binary_version': subprocess.run([str(binary), '--version'], capture_output=True, text=True, timeout=30).stdout.strip(),
              'cases': {}}
    with tempfile.TemporaryDirectory(prefix='scopelet-content-') as tmp:
        root = Path(tmp)
        for name, (original, facts) in fixtures().items():
            case = {'input_bytes': len(original), 'sha256': hashlib.sha256(original).hexdigest(),
                    'facts': facts if name not in RECOVERY_ONLY else ['entire giant line'],
                    'facts_expected': 'after recovery' if name in RECOVERY_ONLY else 'in view', 'states': {}}
            for state in ('cold', 'warm'):
                cache = root / f'{name}-{state}'
                samples, output = [], b''
                for rep in range(-warmups, repetitions):
                    if state == 'cold':
                        shutil.rmtree(cache, ignore_errors=True)
                    cache.mkdir(exist_ok=True)
                    started = time.perf_counter()
                    result = subprocess.run([str(binary), '--cache-dir', str(cache), 'compress'], input=original,
                                            capture_output=True, timeout=120)
                    elapsed = time.perf_counter() - started
                    output = result.stdout
                    if rep >= 0:
                        samples.append({'seconds': elapsed, 'output_bytes': len(output), 'exit_code': result.returncode,
                                        'sha256': hashlib.sha256(output).hexdigest()})
                visible = [fact in output.decode(errors='replace') for fact in facts]
                exact, recovered = recovery(binary, cache, output, original)
                case['states'][state] = {
                    **summarize(samples), 'samples': samples, 'output_bytes': len(output),
                    'byte_reduction_percent': 100 * (1 - len(output) / len(original)),
                    'facts_visible': visible,
                    'facts_available_after_recovery': [v or bool(recovered and fact.encode() in recovered) for v, fact in zip(visible, facts)],
                    'original_byte_roundtrip_verified': exact}
                (out / f'{name}-{state}.output').write_bytes(output)
            report['cases'][name] = case
            print(name, 'complete', flush=True)
            (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def failures(report):
    """Everything a publishable run must not contain."""
    problems = []
    for name, case in report['cases'].items():
        for state, cell in case['states'].items():
            if cell['failures']:
                problems.append(f'{name}/{state}: {cell["failures"]} non-zero exit(s)')
            if not cell['original_byte_roundtrip_verified']:
                problems.append(f'{name}/{state}: original bytes did not round-trip')
            if case['facts_expected'] == 'in view' and not all(cell['facts_visible']):
                problems.append(f'{name}/{state}: a fact is missing from the view')
            if not all(cell['facts_available_after_recovery']):
                problems.append(f'{name}/{state}: a fact is missing even after recovery')
    return problems


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--scopelet', type=Path, required=True)
    p.add_argument('--out', type=Path, required=True, help='fresh directory; completed evidence is never overwritten')
    p.add_argument('--warmups', type=int, default=5)
    p.add_argument('--repetitions', type=int, default=30)
    args = p.parse_args()
    if args.repetitions < 1 or args.warmups < 0:
        p.error('positive repetitions and nonnegative warmups required')
    report = measure(args.scopelet.resolve(), args.out, args.warmups, args.repetitions)
    problems = failures(report)
    for problem in problems:
        print('FAIL', problem)
    if problems:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
