#!/usr/bin/env python3
"""Measure one binary and publish its figures: the only numbers the repository keeps.

Runs the content probe and the performance probe on the given binary, replaces
everything under bench/results/ with the two fresh reports, and rewrites the
speed table in the README between its markers. Older figures are not kept:
a benchmark describes the version that ships, nothing else.
"""
from __future__ import annotations
import argparse
from argparse import Namespace
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import content  # noqa: E402
import performance  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
RESULTS = ROOT / 'bench' / 'results'
START, END = '<!-- speed-table:start -->', '<!-- speed-table:end -->'

# README row -> (report, case) whose cold median is quoted.
ROWS = [
    ('Compress a 136 KB log', ('content', 'diagnostics')),
    ('Compress a 3 MB log', ('performance', 'log/cold')),
    ('Repository query over 400 files', ('performance', 'repo/cold')),
    ('Compress a 32 MiB stream', ('performance', 'limit_32m/cold')),
]


def medians(content_report, performance_report):
    """Cold medians in milliseconds for every README row."""
    arm = performance_report['arms'][0]
    values = {}
    for label, (report, case) in ROWS:
        if report == 'content':
            seconds = content_report['cases'][case]['states']['cold']['median_seconds']
        else:
            seconds = performance_report['cases'][case][arm]['summary']['median_seconds']
        values[label] = seconds * 1000
    return values


def machine():
    """`macOS arm64`, `Linux x86_64`: what the reader needs to compare."""
    system = {'Darwin': 'macOS'}.get(platform.system(), platform.system())
    arch = {'aarch64': 'arm64'}.get(platform.machine(), platform.machine())
    return f'{system} {arch}'


def table(values, version, machine_name, repetitions):
    lines = [START,
             f'Median over {repetitions} runs, cold application cache, {machine_name}, Scopelet',
             f'{version}, measured by `bench/publish.py` on the binary that shipped.',
             '',
             '| Operation | Median |',
             '| --- | ---: |']
    for label, _ in ROWS:
        cell = f'{values[label]:.0f} ms'
        lines.append(f'| {label} | **{cell}** |' if values[label] < 100 else f'| {label} | {cell} |')
    lines.append(END)
    return '\n'.join(lines)


def rewrite(readme, block):
    """Replace the marked speed table; the README must already carry the markers."""
    pattern = re.compile(re.escape(START) + r'.*?' + re.escape(END), re.S)
    if not pattern.search(readme):
        raise ValueError(f'README has no {START} ... {END} block')
    return pattern.sub(lambda _: block, readme, count=1)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--scopelet', type=Path, required=True, help='the release binary to measure and publish')
    parser.add_argument('--repetitions', type=int, default=30)
    parser.add_argument('--warmups', type=int, default=5)
    parser.add_argument('--readme', type=Path, default=ROOT / 'README.md')
    options = parser.parse_args()
    binary = options.scopelet.resolve()
    version = performance.binary_version(binary).split()[-1]
    with tempfile.TemporaryDirectory(prefix='scopelet-publish-') as tmp:
        runs = Path(tmp)
        content_report = content.measure(binary, runs / 'content', options.warmups, options.repetitions)
        problems = content.failures(content_report)
        if problems:
            for problem in problems:
                print('FAIL', problem, file=sys.stderr)
            return 1
        performance_report = performance.run(Namespace(
            out=runs / 'performance', binary=[('release', binary)], warmups=options.warmups,
            repetitions=options.repetitions, stress=True, cases=None))
        if performance_report['output_mismatches'] or any(
                cell['summary']['failures'] for case in performance_report['cases'].values() for cell in case.values()):
            print('FAIL performance probe reported failures', file=sys.stderr)
            return 1
        shutil.rmtree(RESULTS, ignore_errors=True)
        RESULTS.mkdir(parents=True)
        shutil.copy2(runs / 'content' / 'report.json', RESULTS / 'content.json')
        shutil.copy2(runs / 'performance' / 'report.json', RESULTS / 'performance.json')
    values = medians(content_report, performance_report)
    block = table(values, version, machine(), options.repetitions)
    options.readme.write_text(rewrite(options.readme.read_text(), block))
    for label, value in values.items():
        print(f'  {label}: {value:.1f} ms')
    print(f'Published Scopelet {version} figures to {RESULTS.relative_to(ROOT)} and {options.readme.name}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
