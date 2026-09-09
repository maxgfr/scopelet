#!/usr/bin/env python3
"""Export direct-pilot measurements without prompts, commands or private paths."""
import argparse
import hashlib
import importlib.util
import json
import re
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "scopelet_comparison_harness", Path(__file__).resolve().parents[1] / "bench/run.py")
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "bench"))
import compare as comparison  # noqa: E402

MODE_EVIDENCE = (
    "Views in completed tool returns correlated with Scopelet command invocations. "
    "Shell-command recognition is conservative; combined commands can include other output. "
    "Expansion can intentionally return default mode."
)

USAGE_FIELDS = (
    "logical_input_tokens", "input_tokens", "output_tokens", "reasoning_tokens", "thinking_tokens",
    "cache_read_input_tokens", "cache_creation_input_tokens", "usage_missing",
    "cache_included_in_reported_input", "model_init", "models_observed", "model_usage", "model_mismatch",
    "num_turns", "duration_ms", "duration_api_ms", "total_cost_usd_reported", "permission_denials_count",
    "permission_denials", "api_error_status", "fast_mode_state", "result_subtype", "result_is_error",
)
COST_NOTE = "total_cost_usd_reported is Claude Code's list-basis estimate for the session, not an invoice"
# Private absolute paths never belong in a published summary.
PRIVATE_PATH_PATTERN = re.compile(r"(?:/Users/|/home/|/private/|/var/folders/|[A-Za-z]:\\+Users)")


def private_paths(value):
    """Return the private path fragments found in a JSON-serializable value."""
    return sorted(set(PRIVATE_PATH_PATTERN.findall(json.dumps(value))))


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
            "model_requested", "effort_requested", "rtk_integration", "claude_bin_sha256", "environment_purged",
        )}, "runs": []}
    result["meta"]["executables_sha256"] = {
        name: data.get("sha256") for name, data in meta.get("executables", {}).items()}
    result["meta"]["executable_versions"] = {
        name: data.get("version") for name, data in meta.get("executable_versions", {}).items()}
    result["meta"]["scopelet_view_mode_evidence"] = MODE_EVIDENCE
    result["meta"]["analyzer_sha256"] = {
        "compare.py": hashlib.sha256(Path(comparison.__file__).read_bytes()).hexdigest(),
        "export_comparison.py": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    result["meta"]["cost_note"] = COST_NOTE
    result["meta"]["agent_versions"] = {
        name: data.get("version") for name, data in meta.get("agent_versions", {}).items()}
    result["meta"]["prefix_sha256"] = {
        Path(path).name: sha for path, sha in meta.get("prefix_file_hashes", {}).items()}
    result["meta"]["plugins"] = {
        name: {key: data.get(key) for key in ("plugin_json_sha256", "hooks_sha256", "skill_hashes", "git_head")}
        for name, data in meta.get("plugins", {}).items()}
    # Older campaigns hard-coded the model in the harness rather than in meta.
    result["meta"]["models"] = {"codex": "gpt-5.6-luna (low)",
                                "claude": meta.get("model_requested") or "claude-haiku-4-5-20251001"}
    # Provider usage-window rejections are kept apart from measured runs.
    result["meta"]["aborted_attempts"] = [
        {key: item.get(key) for key in ("agent", "task", "arm", "repetition", "attempt", "reason", "reset_epoch", "num_turns")}
        for item in report.get("aborted_attempts", [])]
    for run in report["runs"]:
        row = {key: run.get(key) for key in (
            "agent", "task", "arm", "repetition", "exit_code", "timed_out", "duration_seconds",
            "integration", "model_requested", "effort_requested", "cache_markers_present", "rate_limit_reset",
        )}
        row["passed"] = run.get("acceptance", {}).get("passed", False)
        row["harness_error"] = "harness_error" in run
        row["artifact_error"] = "artifact_error" in run
        row["usage"] = {key: run.get("usage", {}).get(key) for key in USAGE_FIELDS}
        row["tools"] = run.get("tools", {})
        row["fixture_hashes"] = run.get("fixture_hashes", {})
        raw = directory / run.get("raw_directory", "missing")
        if (raw / "stdout.raw").is_file() and run.get("agent"):
            # Adoption counters are derived from the raw trace by the current
            # analyzer (recorded by hash) so that analyzer fixes apply uniformly.
            audit = raw / "rtk-audit/hook-audit.log"
            row["tools"] = comparison.adoption(run["agent"], (raw / "stdout.raw").read_bytes(), run.get("task"),
                                               audit.read_text() if audit.is_file() else None)
            row["usage"] = {key: harness.normalize_usage(run["agent"], (raw / "stdout.raw").read_bytes(), run.get("model_requested")).get(key)
                            for key in USAGE_FIELDS}
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
    campaigns = [export(directory) for directory in args.directories]
    output = {"schema_version": 2,
              "metric": "model-reported logical input plus output; cache included; not currency",
              "models": {campaign["campaign"]: campaign["meta"]["models"] for campaign in campaigns},
              "campaigns": campaigns}
    leaked = private_paths(output)
    if leaked:
        raise SystemExit(f"refusing to export private paths: {leaked}")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(output, indent=2, sort_keys=True) + "\n")
    print(args.out)


if __name__ == "__main__":
    main()
