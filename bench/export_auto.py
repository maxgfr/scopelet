#!/usr/bin/env python3
"""Export an automatic pilot with typed trace audits and no local invocation paths."""
import argparse
import json
from pathlib import Path
import re
import statistics
import auto
import compare
import run as base


def audit(raw):
    calls = compare.ordered_tool_calls('codex', raw)
    outputs = [call['text'] for call in calls]
    messages = [event['item'].get('text', '') for event in base.parse_json_stream(raw)
                if event.get('type') == 'item.completed'
                and isinstance(event.get('item'), dict)
                and event['item'].get('type') == 'agent_message']
    return {
        'completed_shell_calls': len(calls),
        'automatic_commands': sum('scopelet' in c['payload'] and 'run --auto' in c['payload'] for c in calls),
        'compact_results': sum('[scopelet compact-v1 artifact:' in text for text in outputs),
        'recovery_commands': sum('scopelet' in c['payload'] and ' expand ' in c['payload'] for c in calls),
        'cache_paths_in_tool_results': sum(bool(re.search(r'(?:\.scopelet/cache|scopelet-cache)/', text)) for text in outputs),
        'final_response_bytes': len(messages[-1].encode()) if messages else None,
    }


def export(source):
    report = json.loads((source / 'report.json').read_text())
    runs = []
    for original in report['runs']:
        row = {key: original[key] for key in (
            'arm', 'task', 'repetition', 'status', 'exit_code', 'timed_out',
            'duration_seconds', 'model_requested', 'effort_requested', 'usage',
            'acceptance', 'stdout_sha256', 'stderr_sha256', 'fixture_hashes', 'error'
        ) if key in original}
        raw = source / 'runs' / f"{row['task']}-{row['arm']}-{row['repetition']}" / 'stdout.raw'
        row['trace_audit'] = audit(raw.read_bytes()) if raw.exists() else None
        row['integration_audit'] = {key: original.get('tools', {}).get(key) for key in (
            'rtk_invocations', 'rtk_recall_invocations', 'tool_error_count', 'tool_use_count',
        )}
        runs.append(row)
    report.pop('matrix', None)
    report['runs'] = runs
    report['summary'] = auto.summary(runs)
    for task in auto.TASKS:
        for arm in auto.ARMS:
            cell = report['summary'][f'{task}/{arm}']
            values = [r for r in runs if r['task'] == task and r['arm'] == arm]
            cell['stddev_tokens'] = statistics.stdev(cell['tokens']) if len(cell['tokens']) > 1 else None
            for field in ('output_tokens', 'cache_read_input_tokens'):
                measured = [r['usage'].get(field) for r in values if r.get('usage', {}).get(field) is not None]
                cell['mean_' + field] = statistics.mean(measured) if measured else None
            responses = [r['trace_audit']['final_response_bytes'] for r in values
                         if r['trace_audit'] and r['trace_audit']['final_response_bytes'] is not None]
            cell['mean_final_response_bytes'] = statistics.mean(responses) if responses else None
    report['measurement_notes'] = [
        'Tokens are provider-reported logical input plus output across the entire session, not billed cost.',
        'Luna and low effort are explicitly requested in argv. The stream does not independently identify the serving model.',
        'All attempts and failures remain; missing usage is unknown, never zero.',
        'Trace audits inspect completed typed shell events only; they do not prove coverage of other tool types.',
        'Cache-path matches are observable result mentions, not proof of content ingestion or its absence.',
        'Primary campaign binary is immutable; later hardening is validated in separate smoke campaigns.',
    ]
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    report = export(args.source)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')


if __name__ == '__main__':
    main()
