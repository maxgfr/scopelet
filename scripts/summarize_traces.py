#!/usr/bin/env python3
"""Summarize raw campaign traces for a local audit; prints nothing private.

For each run: ordered tool calls (tool, first 90 chars of the command or path,
returned bytes, error flag), Scopelet modes and expansions, hook events, RTK
audit actions, cache path hits, the task4 checks sequence and per-model usage.
Raw transcripts stay local; this summary is for reading, not for publishing.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))
import compare  # noqa: E402
import run as harness  # noqa: E402

spec = importlib.util.spec_from_file_location("comparison_export", ROOT / "scripts/export_comparison.py")
exporter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exporter)


def describe_call(call: dict) -> str:
    payload = call["payload"]
    try:
        parsed = json.loads(payload)
    except (TypeError, ValueError):
        parsed = None
    if isinstance(parsed, dict):
        text = parsed.get("command") or parsed.get("file_path") or parsed.get("pattern") or parsed.get("skill") or json.dumps(parsed, sort_keys=True)
    else:
        text = str(payload)
    text = re.sub(r"\s+", " ", str(text)).strip()
    return text[:90]


def summarize(run_dir: Path, agent: str, task: str) -> dict:
    raw = (run_dir / "stdout.raw").read_bytes()
    calls = compare.ordered_tool_calls(agent, raw)
    usage = harness.normalize_usage(agent, raw)
    rows = [{"tool": call["label"], "what": describe_call(call), "returned_bytes": len(call["text"].encode("utf-8", "replace")),
             "error": call["is_error"], "cache_hit": bool(compare.CACHE_PATTERN.search(call["text"]))} for call in calls]
    audit = run_dir / "rtk-audit/hook-audit.log"
    return {
        "calls": rows,
        "modes": exporter.observed_scopelet_modes(raw),
        "adoption": compare.adoption(agent, raw, task, audit.read_text() if audit.is_file() else None),
        "usage": {key: usage.get(key) for key in ("model_init", "logical_input_tokens", "input_tokens", "cache_read_input_tokens",
                                                    "cache_creation_input_tokens", "output_tokens", "thinking_tokens", "num_turns", "duration_api_ms")},
        "model_usage": {name: {k: v for k, v in data.items() if k in ("inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens", "thinkingTokens")}
                        for name, data in usage.get("model_usage", {}).items()},
        "final_text_chars": sum(len(block.get("text", "")) for event in harness.parse_json_stream(raw) if event.get("type") == "result"
                                for block in [{"text": event.get("result", "")}]),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("campaign", type=Path)
    parser.add_argument("--runs", default="", help="comma-separated run ids; default all")
    parser.add_argument("--json", action="store_true", help="emit JSON instead of text")
    args = parser.parse_args()
    report = json.loads((args.campaign / "report.json").read_text())
    wanted = set(harness.csv_values(args.runs))
    output = {}
    for run in report["runs"]:
        run_id = run.get("run_id")
        if not run_id or (wanted and run_id not in wanted):
            continue
        summary = summarize(args.campaign / run["raw_directory"], run["agent"], run["task"])
        summary["grade"] = run.get("acceptance", {}).get("grade")
        summary["exit_code"] = run.get("exit_code")
        output[run_id] = summary
    if args.json:
        print(json.dumps(output, indent=1, sort_keys=True))
        return
    for run_id, summary in output.items():
        usage = summary["usage"]
        print(f"## {run_id}  grade={summary['grade']} exit={summary['exit_code']} model={usage['model_init']} turns={usage['num_turns']} "
              f"logical_in={usage['logical_input_tokens']} out={usage['output_tokens']} thinking={usage['thinking_tokens']} modes={summary['modes']}")
        adoption = summary["adoption"]
        print(f"   errors={adoption['tool_error_count']} scopelet={adoption['scopelet_invocations']} expand={adoption['scopelet_expand_invocations']} "
              f"rtk={adoption['rtk_invocations']} recall={adoption['rtk_recall_invocations']} audit={adoption['rtk_audit']} hooks={adoption['hook_events']['responses']} "
              f"checks={adoption['checks_sequence_verified']} cache_bytes={adoption['scopelet_cache_reingested_bytes']} models={summary['model_usage']}")
        for index, row in enumerate(summary["calls"], 1):
            flags = ("ERR " if row["error"] else "") + ("CACHE " if row["cache_hit"] else "")
            print(f"   {index:2d}. {row['tool']:<6} {row['returned_bytes']:>7}B {flags}{row['what']}")


if __name__ == "__main__":
    main()
