#!/usr/bin/env python3
"""Export inspected small-task campaigns without personal paths or raw transcripts."""
import argparse
import json
from pathlib import Path
import proportional


def export(source):
    original=json.loads((source/'report.json').read_text())
    report={k:original[k] for k in ['model_requested','effort','live','notes','binary_sha256','harness_sha256','codex_version','stopped'] if k in original}
    report['runs']=[]
    for row in original['runs']:
        clean={k:row[k] for k in ['task','arm','repetition','status','exit_code','timed_out','duration_seconds','usage','passed','fixture_hashes','stdout_sha256','trace'] if k in row}
        if row.get('error') or row.get('spawn_error'):
            clean['error']='Harness or spawn failure; details retained in private raw report.'
        report['runs'].append(clean)
    report['summary']=proportional.summarize(report['runs'])
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('source',type=Path);parser.add_argument('output',type=Path);a=parser.parse_args()
    a.output.write_text(json.dumps(export(a.source),indent=2)+'\n')


if __name__=='__main__':main()
