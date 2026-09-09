#!/usr/bin/env python3
"""Export direct-pilot measurements without prompts, commands or private paths."""
import argparse
import hashlib
import importlib.util
import json
import re
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "scopelet_comparison_harness", Path(__file__).resolve().parents[1] / "bench/run.py")
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)

MODE_EVIDENCE = (
    "Views in completed tool returns correlated with Scopelet command invocations. "
    "Shell-command recognition is conservative; combined commands can include other output. "
    "Expansion can intentionally return default mode."
)

USAGE_FIELDS = (
    "logical_input_tokens", "input_tokens", "output_tokens", "reasoning_tokens",
    "cache_read_input_tokens", "cache_creation_input_tokens", "usage_missing",
    "cache_included_in_reported_input",
)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None


def observed_scopelet_modes(raw):
    """Read views only from tool returns correlated with Scopelet commands."""
    modes = set()
    decoder = json.JSONDecoder()

    def visit(value):
        if isinstance(value, dict):
            if (value.get("schema_version") == 1
                    and re.fullmatch(r"artifact:[a-f0-9]{64}", str(value.get("artifact", "")))
                    and value.get("mode") in ("default", "ultra")
                    and isinstance(value.get("display_complete"), bool)):
                modes.add(value["mode"])
                # Text records can themselves contain quoted JSON examples.
                return
            for item in value.values():
                visit(item)
        elif isinstance(value, list):
            for item in value:
                visit(item)
        elif isinstance(value, str) and 'artifact:' in value and 'schema_version' in value:
            for match in re.finditer(r'\{\s*"(?:schema_version|artifact)"', value):
                try:
                    item, _ = decoder.raw_decode(value[match.start():])
                    visit(item)
                except ValueError:
                    pass

    scopelet_calls = set()
    for event in harness.parse_json_stream(raw):
        # Codex emits both started and completed command items. Only the latter
        # carries returned output; a nonzero child exit can still return a view.
        item = event.get("item", {})
        if (event.get("type") == "item.completed" and isinstance(item, dict)
                and item.get("type") == "command_execution"
                and harness._is_scopelet_invocation("command_execution", str(item.get("command", "")))):
            visit(item.get("aggregated_output", ""))

        # Claude puts calls in assistant messages and matching results in user
        # messages. Never recurse into prose, tool arguments or result metadata.
        message = event.get("message", {})
        blocks = message.get("content", []) if isinstance(message, dict) else []
        if not isinstance(blocks, list):
            continue
        for block in blocks:
            if not isinstance(block, dict):
                continue
            if (event.get("type") == "assistant" and block.get("type") == "tool_use"
                    and block.get("name") == "Bash" and isinstance(block.get("id"), str)
                    and harness._is_scopelet_invocation("Bash", json.dumps(block.get("input", {})))):
                scopelet_calls.add(block["id"])
            elif (event.get("type") == "user" and block.get("type") == "tool_result"
                    and block.get("tool_use_id") in scopelet_calls):
                visit(block.get("content", ""))
    return sorted(modes)


def export(directory):
    report = json.loads((directory / "report.json").read_text())
    meta = report["meta"]
    result = {"campaign": directory.name, "meta": {
        key: meta.get(key) for key in (
            "seed", "repetitions", "timeout", "planned_runs", "modes", "limitations",
            "harness_sha256", "base_harness_sha256", "source_commits", "skill_hashes",
        )}, "runs": []}
    result["meta"]["executables_sha256"] = {
        name: data.get("sha256") for name, data in meta.get("executables", {}).items()}
    result["meta"]["scopelet_view_mode_evidence"] = MODE_EVIDENCE
    result["meta"]["agent_versions"] = {
        name: data.get("version") for name, data in meta.get("agent_versions", {}).items()}
    result["meta"]["prefix_sha256"] = {
        Path(path).name: sha for path, sha in meta.get("prefix_file_hashes", {}).items()}
    for run in report["runs"]:
        row = {key: run.get(key) for key in (
            "agent", "task", "arm", "repetition", "exit_code", "timed_out", "duration_seconds",
        )}
        row["passed"] = run.get("acceptance", {}).get("passed", False)
        row["harness_error"] = "harness_error" in run
        row["artifact_error"] = "artifact_error" in run
        row["usage"] = {key: run.get("usage", {}).get(key) for key in USAGE_FIELDS}
        row["tools"] = run.get("tools", {})
        row["fixture_hashes"] = run.get("fixture_hashes", {})
        raw = directory / run.get("raw_directory", "missing")
        row["stdout_sha256"] = digest(raw / "stdout.raw")
        row["stderr_sha256"] = digest(raw / "stderr.raw")
        row["prompt_sha256"] = digest(raw / "prompt.txt")
        row["observed_scopelet_view_modes"] = observed_scopelet_modes(
            (raw / "stdout.raw").read_bytes()) if (raw / "stdout.raw").is_file() else []
        if "headroom" in run:
            row["headroom"] = {key: run["headroom"].get(key) for key in (
                "proxy_ready", "mode", "measurement_missing", "runner_sha256",
                "stats_before", "stats_after",
            )}
        result["runs"].append(row)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directories", type=Path, nargs="+")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    output = {"schema_version": 1,
              "metric": "model-reported logical input plus output; cache included; not currency",
              "models": {"codex": "gpt-5.6-luna (low)", "claude": "claude-haiku-4-5-20251001"},
              "campaigns": [export(directory) for directory in args.directories]}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(output, indent=2, sort_keys=True) + "\n")
    print(args.out)


if __name__ == "__main__":
    main()
