#!/usr/bin/env python3
"""Export inspected Luna performance results without prompts, commands or personal paths."""
import argparse
import json
from pathlib import Path
import compare
import performance_live as live
import run as base


def rollout(runs, complete):
    by_arm={arm:[r for r in runs if r['arm']==arm] for arm in ('baseline','candidate')}
    available=complete and all(not r.get('usage',{}).get('usage_missing',True) for rows in by_arm.values() for r in rows)
    tokens={arm:sum(r['usage']['logical_input_tokens']+r['usage']['output_tokens'] for r in rows if r['task']!='small') if available else None for arm,rows in by_arm.items()}
    change=(100*(tokens['candidate']/tokens['baseline']-1)) if available and tokens['baseline'] else None
    functional=complete and all(r.get('passed') and r.get('exit_code')==0 and not r.get('timed_out') for r in by_arm['candidate'])
    return {'complete':complete,'evidence_tokens':tokens,'change_vs_baseline_percent':change,
            'all_candidate_sessions_pass':functional,'eligible_for_v2_default':bool(functional and change is not None and change<=-10)}


def export(source):
    original=json.loads((source/'report.json').read_text())
    report={k:original[k] for k in ('model_requested','effort','live','notes','binary_sha256','harness_sha256','codex_version','stopped') if k in original}
    report['runs']=[]
    for row in original['runs']:
        clean={k:row[k] for k in ('task','arm','repetition','exit_code','timed_out','duration_seconds','usage','passed','stdout_sha256','quota_rejected','harness_error') if k in row}
        clean['fixture_hashes']={k:v for k,v in row.get('fixture_hashes',{}).items() if not k.startswith('.scopelet/')}
        raw=source/'runs'/f"{row['task']}-{row['arm']}-{row['repetition']}"/'stdout.raw'
        if raw.is_file():
            data=raw.read_bytes()
            if row.get('stdout_sha256')!=base.sha256_bytes(data):raise ValueError('raw trace hash mismatch')
            calls=compare.ordered_tool_calls('codex',data)
            recoveries=sum(bool(tokens) and base._is_scopelet_executable(tokens[0]) and len(tokens)>1 and tokens[1]=='expand'
                for call in calls for tokens in compare._shell_segments(call['label'],call['payload']))
            clean['trace']={'completed_shell_calls':len(calls),'expand_invocations':recoveries,
                'outputs_with_compact_v1':sum('[scopelet compact-v1' in c['text'] for c in calls),
                'outputs_with_compact_v2':sum('[scopelet compact-v2' in c['text'] for c in calls),
                'tool_errors':sum(c['is_error'] for c in calls)}
        report['runs'].append(clean)
    expected={(c['task'],c['arm'],c['repetition']) for c in original['matrix']}
    observed={(c['task'],c['arm'],c['repetition']) for c in report['runs']}
    complete=len(report['runs'])==len(expected) and observed==expected and not original.get('stopped')
    report['summary']=live.summarize(report['runs'])
    report['rollout']=rollout(report['runs'],complete)
    return report


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('source',type=Path);p.add_argument('output',type=Path)
    args=p.parse_args()
    with args.output.open('x') as output:output.write(json.dumps(export(args.source),indent=2)+'\n')

if __name__=='__main__':main()
