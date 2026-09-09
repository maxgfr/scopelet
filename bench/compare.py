#!/usr/bin/env python3
"""Bounded competitor comparison; writes a plan unless --live is explicit.

This reuses the existing frozen-fixture grader and provider usage accounting.
Modes have different integration boundaries; this is not a universal ranking.
Claude Code runs a pinned model and effort; plugin hooks, the RTK PreToolUse
hook and the Headroom proxy are proven by stream events, audit logs and proxy
counters rather than assumed.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import random
import re
import shlex
import shutil
import subprocess
import tempfile
import time
from typing import Any

import run as base

ARMS = ("native", "concise", "scopelet", "scopelet-ultra", "scopelet-caveman", "caveman", "ponytail", "rtk", "headroom")
TASKS = ("task4", "task2", "task3")
DEFAULT_MODEL = "claude-fable-5-1"
DEFAULT_EFFORT = "high"
PLUGIN_NAMES = ("caveman", "ponytail")
RTK_INTEGRATIONS = ("hook", "manual")
CONCISE = "Keep narration and the final report concise. Preserve exact code, commands, quantities, negation, evidence, and unresolved limitations."
MODES = {
    "native": "Unchanged native file/shell workflow",
    "concise": "Native workflow plus a short prose-only instruction",
    "scopelet": "Project skill, explicit /scopelet activation and default-mode CLI guidance",
    "scopelet-ultra": "Project skill, explicit /scopelet activation and ultra-mode CLI guidance",
    "scopelet-caveman": "Scopelet ultra plus the upstream Caveman plugin (automatic hooks) and explicit activation",
    "caveman": "Upstream plugin loaded with --plugin-dir (SessionStart/UserPromptSubmit hooks) plus explicit activation; legacy: copied project skill",
    "ponytail": "Upstream plugin loaded with --plugin-dir (SessionStart/UserPromptSubmit hooks) plus explicit activation; legacy: copied project skill",
    "rtk": "Documented PreToolUse Bash hook (rtk hook claude) injected through --settings; transparent rewriting without manual prefixes; legacy: manual wrapper guidance",
    "headroom": "Externally supplied Headroom token-mode proxy prefix wrapping the native agent CLI",
}
SHELL_LABELS = ("bash", "command_execution", "exec_command", "shell", "shell_command")
CACHE_PATTERN = re.compile(r"scopelet-cache/(?:blobs|artifacts)/")
CHECKS_FAILURE_PATTERNS = ("AssertionError", "Exit code 1", '"exit_code":1', '"exit_code": 1', "limit=0 must select")
CHECKS_SUCCESS_PATTERNS = ("checks complete", '"exit_code":0', '"exit_code": 0')


def cases(agents: list[str], arms: list[str], tasks: list[str], repetitions: int, seed: int) -> list[dict[str, Any]]:
    result = [{"agent": agent, "task": task, "arm": arm, "repetition": repetition}
              for repetition in range(1, repetitions + 1) for agent in agents for task in tasks for arm in arms]
    random.Random(seed).shuffle(result)
    return result


def plugin_for(arm: str) -> str | None:
    if arm == "scopelet-caveman":
        return "caveman"
    return arm if arm in PLUGIN_NAMES else None


def activation(agent: str, name: str, plugin: bool) -> str:
    """Explicit skill activation: plugin skills are namespaced in Claude Code."""
    if agent == "codex":
        return "$" + name + " "
    return f"/{name}:{name} " if plugin else "/" + name + " "


def prompt_for(case: dict[str, Any], binary: Path, plugins: tuple[str, ...] = (), rtk_integration: str = "manual") -> str:
    arm, agent, task = case["arm"], case["agent"], case["task"]
    if arm.startswith("scopelet"):
        prompt = base.prompt_for(agent, task, "default" if arm == "scopelet" else "ultra", binary)
        if arm == "scopelet-caveman":
            plugin = "caveman" in plugins
            source = "caveman plugin skill" if plugin else "copied caveman skill"
            prompt += f"Also invoke the {source} in full mode for communication. Keep Scopelet ultra for the local evidence operations.\n"
            if agent == "codex":
                prompt = "$caveman " + prompt
        return "$scopelet " + prompt if agent == "codex" else prompt
    prompt = base.prompt_for(agent, task, "baseline", binary)
    if arm == "concise":
        prompt += CONCISE + "\n"
    elif arm in PLUGIN_NAMES:
        plugin = arm in plugins
        prompt = activation(agent, arm, plugin) + prompt
        prompt += f"Use the {arm} {'plugin' if plugin else 'copied'} skill for this task.\n"
    elif arm == "rtk" and rtk_integration == "manual":
        prompt += 'RTK is available at "$RTK_BIN". Use it for supported noisy test output. '
        if task == "task4":
            prompt += 'Wrap both required checks with CLAUDE_CONFIG_DIR="$RTK_LOCAL_CONFIG" "$RTK_BIN" test python3 checks.py. '
        prompt += 'Recover omitted output with CLAUDE_CONFIG_DIR="$RTK_LOCAL_CONFIG" "$RTK_BIN" recall HASH --grep PATTERN when needed. Use native tools for operations outside RTK supported commands.\n'
    # The hook integration keeps the native prompt: rewriting is transparent.
    return prompt


def global_skill_paths() -> list[Path]:
    roots = [Path.home() / ".agents/skills", Path.home() / ".codex/skills"]
    return sorted({path.absolute() for root in roots if root.is_dir() for path in root.rglob("SKILL.md") if path.is_file()})


def rtk_hook_settings(rtk: Path) -> str:
    """The settings JSON `rtk init` documents for Claude Code, pointing at one binary."""
    return json.dumps({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": f"{rtk} hook claude"}]}]}})


def agent_command(case: dict[str, Any], paths: dict[str, Path], prefix: list[str], options: argparse.Namespace | None = None) -> list[str]:
    agent, arm = case["agent"], case["arm"]
    model = getattr(options, "model", None)
    effort = getattr(options, "effort", None)
    claude_bin = getattr(options, "claude_bin", None)
    plugins: dict[str, Path] = getattr(options, "plugins", None) or {}
    rtk_integration = getattr(options, "rtk_integration", "manual")
    if agent == "claude":
        command = base.command_for("claude", model, effort, claude_bin, include_hook_events=True)
    else:
        command = base.command_for(agent)
    plugin = plugin_for(arm)
    if plugin is not None and plugin in plugins:
        if agent != "claude":
            raise ValueError("plugin integration is verified for Claude Code only")
        command += ["--plugin-dir", str(plugins[plugin])]
    if arm == "rtk" and rtk_integration == "hook":
        if agent != "claude":
            raise ValueError("the RTK PreToolUse hook is a Claude Code integration")
        command += ["--settings", rtk_hook_settings(paths["rtk"])]
    if agent == "codex":
        disabled = global_skill_paths()
        if disabled:
            command += ["-c", "skills.config=[" + ",".join("{path=" + json.dumps(str(path)) + ",enabled=false}" for path in disabled) + "]"]
    if arm == "headroom":
        if not prefix:
            raise ValueError("headroom arm requires --headroom-prefix-json; no proxy syntax is guessed")
        command = [str(paths["headroom"]) if part == "{headroom}" else part for part in prefix] + command
    return command


def _result_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(_result_text(item) for item in content)
    if isinstance(content, dict):
        text = content.get("text")
        return text if isinstance(text, str) else _result_text(content.get("content", ""))
    return ""


def ordered_tool_calls(agent: str, raw: bytes) -> list[dict[str, Any]]:
    """Tool calls in stream order with their returned text and error flag."""
    calls: list[dict[str, Any]] = []
    by_id: dict[str, dict[str, Any]] = {}
    for event in base.parse_json_stream(raw):
        if agent == "codex":
            item = event.get("item")
            if event.get("type") == "item.completed" and isinstance(item, dict) and item.get("type") == "command_execution":
                exit_code = item.get("exit_code")
                calls.append({"label": "command_execution", "payload": str(item.get("command", "")),
                              "text": _result_text(item.get("aggregated_output", "")),
                              "is_error": isinstance(exit_code, int) and exit_code != 0})
            continue
        # Claude puts calls in assistant messages and results in user messages;
        # bare tool_use records are accepted for compatibility with tests and
        # other stream shapes. Records inside prose are never inspected.
        for block in base._walk_dicts(event):
            if block.get("type") == "tool_use" and isinstance(block.get("id"), str):
                payload = block.get("input", {})
                call = {"label": str(block.get("name") or "tool_use"),
                        "payload": json.dumps(payload, sort_keys=True) if not isinstance(payload, str) else payload,
                        "text": "", "is_error": False, "returned": False}
                if block["id"] not in by_id:
                    by_id[block["id"]] = call
                    calls.append(call)
            elif block.get("type") == "tool_result" and block.get("tool_use_id") in by_id:
                call = by_id[block["tool_use_id"]]
                if not call["returned"]:
                    call["returned"] = True
                    call["text"] = _result_text(block.get("content", ""))
                    call["is_error"] = block.get("is_error") is True
    return calls


def _shell_segments(label: str, payload: str) -> list[list[str]]:
    if label.lower() not in SHELL_LABELS:
        return []
    segments = []
    for segment in base._split_shell_commands(base._unwrap_shell_script(base._command_payload(payload))):
        tokens = base._shell_tokens(segment)
        if tokens and not any(arg in ("--help", "-h", "--version", "-V") for arg in tokens):
            segments.append(tokens)
    return segments


def _is_checks_invocation(tokens: list[str]) -> bool:
    runners = ("python", "python3", "scopelet", "rtk")
    return bool(tokens) and (Path(tokens[0]).name in runners or tokens[0] in ("$SCOPELET_BIN", "${SCOPELET_BIN}", "$RTK_BIN", "${RTK_BIN}")) and "checks.py" in tokens[1:]


def _is_rtk_tokens(tokens: list[str]) -> bool:
    return bool(tokens) and (Path(tokens[0]).name == "rtk" or tokens[0] in ("$RTK_BIN", "${RTK_BIN}")) and len(tokens) > 1


def checks_sequence(agent: str, raw: bytes) -> dict[str, Any]:
    """Task4 evidence: the first checks run must fail and the last must pass.

    Text patterns cover native output, Claude's `Exit code N` wrapper, RTK
    summaries and Scopelet run views. This is trace evidence, not a proof of
    semantic fidelity.
    """
    runs = []
    for call in ordered_tool_calls(agent, raw):
        if any(_is_checks_invocation(tokens) for tokens in _shell_segments(call["label"], call["payload"])):
            text = call["text"]
            failure = call["is_error"] or any(pattern in text for pattern in CHECKS_FAILURE_PATTERNS)
            success = (not failure) and (any(pattern in text for pattern in CHECKS_SUCCESS_PATTERNS) or (call.get("returned", True) and not call["is_error"]))
            runs.append({"failure_evidence": failure, "success_evidence": success})
    verified = len(runs) >= 2 and runs[0]["failure_evidence"] and runs[-1]["success_evidence"]
    return {"checks_runs": runs, "verified": verified}


def hook_events(raw: bytes) -> dict[str, Any]:
    """Hook lifecycle events emitted with --include-hook-events, counted by name."""
    started: dict[str, int] = {}
    responses: dict[str, int] = {}
    nonempty: dict[str, int] = {}
    for event in base.parse_json_stream(raw):
        if event.get("type") != "system":
            continue
        name = str(event.get("hook_name") or event.get("hook_event") or "unknown")
        if event.get("subtype") == "hook_started":
            started[name] = started.get(name, 0) + 1
        elif event.get("subtype") == "hook_response":
            responses[name] = responses.get(name, 0) + 1
            output = event.get("output")
            if isinstance(output, str) and output.strip():
                nonempty[name] = nonempty.get(name, 0) + 1
    return {"started": dict(sorted(started.items())), "responses": dict(sorted(responses.items())), "responses_with_output": dict(sorted(nonempty.items()))}


def rtk_audit_summary(text: str) -> dict[str, Any]:
    """Count RTK hook audit actions (`timestamp | action | original | rewritten`)."""
    actions: dict[str, int] = {}
    for line in text.splitlines():
        parts = line.split(" | ")
        if len(parts) >= 3:
            action = parts[1].strip()
            actions[action] = actions.get(action, 0) + 1
    return {"lines": sum(actions.values()), "actions": dict(sorted(actions.items())), "rewrites": actions.get("rewrite", 0)}


def adoption(agent: str, raw: bytes, task: str | None = None, rtk_audit: str | None = None) -> dict[str, Any]:
    result = base.tool_usage(agent, raw, b"")
    calls = ordered_tool_calls(agent, raw)
    rtk = recall = expand = checks = 0
    errors = sum(1 for call in calls if call["is_error"])
    retrieve = sum(1 for call in calls if call["label"].startswith("mcp__headroom__"))
    skills = {name: False for name in PLUGIN_NAMES}
    reingested = 0
    for call in calls:
        for line in call["text"].splitlines():
            if CACHE_PATTERN.search(line):
                reingested += len(line.encode("utf-8", "replace"))
        if call["label"] == "Skill":
            try:
                arguments = json.loads(call["payload"])
            except json.JSONDecodeError:
                arguments = {}
            requested = str(arguments.get("skill", "")) if isinstance(arguments, dict) else ""
            for name in skills:
                if requested == name or requested.endswith(":" + name):
                    skills[name] = True
        for tokens in _shell_segments(call["label"], call["payload"]):
            if _is_checks_invocation(tokens):
                checks += 1
            if _is_rtk_tokens(tokens):
                rtk += 1
                if tokens[1] == "recall":
                    recall += 1
            if base._is_scopelet_invocation(call["label"], shlex.join(tokens)) and "expand" in tokens[1:3]:
                expand += 1
    sequence = checks_sequence(agent, raw) if task == "task4" else {"checks_runs": [], "verified": None}
    result.update({
        "rtk_invocations": rtk, "rtk_recall_invocations": recall, "checks_command_invocations": checks,
        "checks_sequence_verified": sequence["verified"], "checks_runs": sequence["checks_runs"],
        "skill_tool_invocations": skills,
        "skill_compliance": "not inferred from short output or discovery; inspect raw trace",
        "tool_error_count": errors, "scopelet_expand_invocations": expand, "headroom_retrieve_calls": retrieve,
        "hook_events": hook_events(raw), "scopelet_cache_reingested_bytes": reingested,
        "rtk_audit": rtk_audit_summary(rtk_audit) if rtk_audit is not None else None,
        "headroom_compression_verified": None,
    })
    return result


def cache_markers(workspace: Path) -> dict[str, Any] | None:
    cache = workspace / "scopelet-cache"
    if not cache.is_dir():
        return None
    return {name: (cache / name / ".ignore").is_file() and (cache / name / ".gitignore").is_file()
            for name in ("blobs", "artifacts") if (cache / name).is_dir()}


def invoke(command: list[str], prompt: str, workspace: Path, timeout: float, env: dict[str, str]) -> dict[str, Any]:
    started = time.monotonic()
    process = None
    result = {"command": command, "stdout": b"", "stderr": b"", "exit_code": None,
              "timed_out": False, "spawn_error": None}
    try:
        process = subprocess.Popen(command, cwd=workspace, env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        result["stdout"], result["stderr"] = process.communicate(prompt.encode(), timeout=timeout)
        result["exit_code"] = process.returncode
    except subprocess.TimeoutExpired:
        result["timed_out"] = True
        base._kill_process_group(process)
        try:
            result["stdout"], result["stderr"] = process.communicate(timeout=5)
        except subprocess.TimeoutExpired as error:
            result["stdout"], result["stderr"] = error.output or b"", error.stderr or b""
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream:
                    stream.close()
        result["exit_code"] = process.poll()
    except OSError as error:
        result["spawn_error"] = str(error)
    result["duration_seconds"] = round(time.monotonic() - started, 6)
    return result


def rtk_audit_path() -> Path:
    # The hook writes here regardless of RTK_AUDIT_DIR, which only affects the reader.
    return Path.home() / ".local/share/rtk/hook-audit.log"


def _file_size(path: Path) -> int:
    try:
        return path.stat().st_size
    except OSError:
        return 0


def _read_from(path: Path, offset: int) -> str:
    try:
        with path.open("rb") as stream:
            stream.seek(offset)
            return stream.read().decode("utf-8", "replace")
    except OSError:
        return ""


def run_environment(case: dict[str, Any], args: argparse.Namespace, paths: dict[str, Path], workspace: Path, run_dir: Path) -> dict[str, str]:
    env = base.purged_environment()
    # Avoid carrying treatment settings into native and skill-only arms.
    for variable in list(env):
        if variable in ("SCOPELET_BIN", "SCOPELET_CACHE_DIR", "RTK_BIN", "RTK_LOCAL_CONFIG", "CAVEMAN_DEFAULT_MODE", "PONYTAIL_DEFAULT_MODE", "DO_NOT_TRACK") or variable.startswith(("RTK_", "HEADROOM_")):
            env.pop(variable, None)
    arm = case["arm"]
    name = "scopelet" if arm.startswith("scopelet") else arm
    if name == "scopelet":
        env["SCOPELET_BIN"] = str(paths["scopelet"])
        # Kept inside the workspace deliberately: it exercises the cache markers.
        env["SCOPELET_CACHE_DIR"] = str(workspace / "scopelet-cache")
    if name == "rtk":
        env["RTK_BIN"] = str(paths["rtk"])
        env["RTK_TELEMETRY_DISABLED"] = "1"
        env["RTK_LOCAL_CONFIG"] = str(workspace / ".rtk-claude")
        env["NO_COLOR"] = "1"
        env["RTK_DB_PATH"] = str(workspace / "rtk-tracking.db")
        env["RTK_RECALL_DB"] = str(workspace / "rtk-recall.db")
        env["RTK_HOOK_AUDIT"] = "1"
        env["RTK_AUDIT_DIR"] = str(run_dir / "rtk-audit")
        # The hook rewrites commands to a bare `rtk`, so the frozen binary must be on PATH.
        env["PATH"] = str(paths["rtk"].parent) + os.pathsep + env.get("PATH", "")
    plugin = plugin_for(arm)
    if plugin == "caveman":
        env["CAVEMAN_DEFAULT_MODE"] = "full"
        env["DO_NOT_TRACK"] = "1"
    if plugin == "ponytail":
        env["PONYTAIL_DEFAULT_MODE"] = "full"
    if arm == "headroom":
        env["HEADROOM_BEACON"] = "off"
        env["HEADROOM_TELEMETRY"] = "off"
        env["HEADROOM_LOG_FILE"] = str(run_dir / "headroom" / "request-log.jsonl")
    return env


def execute(case: dict[str, Any], args: argparse.Namespace, paths: dict[str, Path], skills: dict[str, Path], out: Path) -> dict[str, Any]:
    run_id = f"{case['agent']}_{case['task']}_{case['arm']}_{case['repetition']}"
    run_dir = out / "runs" / run_id
    run_dir.mkdir(parents=True)
    plugins: dict[str, Path] = getattr(args, "plugins", None) or {}
    with tempfile.TemporaryDirectory(prefix="scopelet-comparison-") as temporary:
        workspace = Path(temporary)
        base.create_fixture(workspace, case["task"])
        hashes = base.fixture_hashes(workspace)
        name = "scopelet" if case["arm"].startswith("scopelet") else case["arm"]
        skill_names = ["scopelet", "caveman"] if case["arm"] == "scopelet-caveman" else [name]
        for skill_name in skill_names:
            # Plugin-provided skills are loaded by --plugin-dir, never copied as well.
            if skill_name in skills and skill_name not in plugins:
                for parent in (".agents/skills", ".claude/skills"):
                    shutil.copytree(skills[skill_name], workspace / parent / skill_name)
        base._git_init(workspace)
        prompt = prompt_for(case, paths.get("scopelet", Path("scopelet")), tuple(plugins), getattr(args, "rtk_integration", "manual"))
        (run_dir / "prompt.txt").write_text(prompt)
        env = run_environment(case, args, paths, workspace, run_dir)
        if case["arm"] == "headroom":
            (run_dir / "headroom").mkdir(exist_ok=True)
        audit_offset = _file_size(rtk_audit_path()) if case["arm"] == "rtk" else None
        invocation = invoke(agent_command(case, paths, args.prefix, args), prompt, workspace, args.timeout, env)
        rtk_audit = None
        if audit_offset is not None:
            rtk_audit = _read_from(rtk_audit_path(), audit_offset)
            (run_dir / "rtk-audit").mkdir(exist_ok=True)
            (run_dir / "rtk-audit" / "hook-audit.log").write_text(rtk_audit)
        stdout, stderr = invocation.pop("stdout"), invocation.pop("stderr")
        (run_dir / "stdout.raw").write_bytes(stdout)
        (run_dir / "stderr.raw").write_bytes(stderr)
        expected_model = getattr(args, "model", None) if case["agent"] == "claude" else None
        integration = MODES[case["arm"]]
        if case["arm"] in PLUGIN_NAMES or case["arm"] == "scopelet-caveman":
            integration += " [" + ("plugin" if plugin_for(case["arm"]) in plugins else "legacy copied skill") + "]"
        if case["arm"] == "rtk":
            integration += f" [{getattr(args, 'rtk_integration', 'manual')}]"
        result = {**case, **invocation, "run_id": run_id, "integration": integration,
                  "model_requested": expected_model, "effort_requested": getattr(args, "effort", None) if case["agent"] == "claude" else None,
                  "fixture_hashes": hashes, "acceptance": base.grade_workspace(case["task"], workspace, expected_fixture_hashes=hashes),
                  "usage": base.normalize_usage(case["agent"], stdout, expected_model),
                  "tools": adoption(case["agent"], stdout, case["task"], rtk_audit),
                  "cache_markers_present": cache_markers(workspace),
                  "raw_directory": str(run_dir.relative_to(out))}
        if case["arm"] == "headroom":
            try:
                result["headroom"] = json.loads((workspace / ".headroom-comparison/summary.json").read_text())
            except (OSError, ValueError):
                result["headroom"] = {"measurement_missing": True}
        # Preserve measured outcomes even if a generated artifact cannot be archived.
        try:
            base._copy_tree_after(workspace, run_dir / "workspace_after")
        except OSError as error:
            result["artifact_error"] = f"{type(error).__name__}: {error}"
        (run_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True))
        return result


def write_report(out: Path, report: dict[str, Any]) -> None:
    # A crash cannot leave the previous completed-run report partially overwritten.
    temporary = out / "report.json.tmp"
    temporary.write_text(json.dumps(report, indent=2, sort_keys=True))
    temporary.replace(out / "report.json")
    lines = ["# Competitor comparison", "", "Integration modes differ. No universal ranking is established.", "",
             "| Agent | Task | Arm | Repeat | Grade | Exit | Model | Turns | Input | Output | Seconds |",
             "|---|---|---|---:|---|---:|---|---:|---:|---:|---:|"]
    for item in report["runs"]:
        usage = item.get("usage", {})
        lines.append(f"| {item['agent']} | {item['task']} | {item['arm']} | {item['repetition']} | {item.get('acceptance', {}).get('grade', 'harness_error')} | {item.get('exit_code')} | {usage.get('model_init')} | {usage.get('num_turns')} | {usage.get('logical_input_tokens')} | {usage.get('output_tokens')} | {item.get('duration_seconds')} |")
    (out / "report.md").write_text("\n".join(lines) + "\n")


def plugin_provenance(name: str, path: Path) -> dict[str, Any]:
    """Hashes of what the plugin loads; the checkout revision is recorded when available."""
    manifest = path / ".claude-plugin/plugin.json"
    hooks: dict[str, str] = {}
    try:
        declared = json.loads(manifest.read_text()).get("hooks")
    except (OSError, ValueError):
        declared = None
    if isinstance(declared, str) and (path / declared).is_file():
        hooks[declared] = base.sha256_file(path / declared)
    for candidate in sorted((path / "hooks").glob("*")) if (path / "hooks").is_dir() else []:
        if candidate.is_file():
            hooks[str(candidate.relative_to(path))] = base.sha256_file(candidate)
    for candidate in sorted((path / "src/hooks").glob("*.js")) if (path / "src/hooks").is_dir() else []:
        hooks[str(candidate.relative_to(path))] = base.sha256_file(candidate)
    try:
        head = subprocess.run(["git", "-C", str(path), "rev-parse", "HEAD"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=10, check=False).stdout.decode().strip() or None
    except (OSError, subprocess.TimeoutExpired):
        head = None
    skill = path / "skills" / name
    return {"path": str(path), "plugin_json_sha256": base.sha256_file(manifest) if manifest.is_file() else None,
            "hooks_sha256": hooks, "skill_hashes": base.fixture_hashes(skill) if skill.is_dir() else {}, "git_head": head}


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--agents", default="codex,claude")
    result.add_argument("--arms", default=",".join(ARMS))
    result.add_argument("--tasks", default="task4")
    result.add_argument("--repetitions", type=int, default=1)
    result.add_argument("--seed", type=int, default=20260909)
    result.add_argument("--timeout", type=float, default=300)
    result.add_argument("--out", default="bench/runs/direct-comparison")
    result.add_argument("--model", default=DEFAULT_MODEL, help="Claude Code model id; verified against init/modelUsage per run")
    result.add_argument("--effort", default=DEFAULT_EFFORT, choices=(*base.CLAUDE_EFFORTS, "none"), help="Claude Code --effort; 'none' omits the flag")
    result.add_argument("--claude-bin", default="claude", help="Claude Code executable; pin an absolute path to avoid shell aliases")
    result.add_argument("--scopelet-binary")
    result.add_argument("--scopelet-skill", default=str(Path(__file__).resolve().parents[1] / "skills/scopelet"))
    result.add_argument("--caveman-skill", help="Legacy: copy this skill into the workspace instead of loading the plugin")
    result.add_argument("--ponytail-skill", help="Legacy: copy this skill into the workspace instead of loading the plugin")
    result.add_argument("--caveman-plugin", help="Plugin checkout containing .claude-plugin/plugin.json (loaded with --plugin-dir)")
    result.add_argument("--ponytail-plugin", help="Plugin checkout containing .claude-plugin/plugin.json (loaded with --plugin-dir)")
    result.add_argument("--rtk-binary")
    result.add_argument("--rtk-integration", default="hook", choices=RTK_INTEGRATIONS, help="hook: documented PreToolUse hook via --settings; manual: pilot wrapper guidance")
    result.add_argument("--headroom-binary")
    result.add_argument("--source-commits-json", default="{}", help="Operator-recorded upstream commit metadata; hashes remain independent")
    result.add_argument("--headroom-prefix-json", default="[]", help='JSON argv prefix; {headroom} expands to executable. Native agent argv is appended.')
    mode = result.add_mutually_exclusive_group()
    mode.add_argument("--live", action="store_true")
    mode.add_argument("--dry-run", action="store_true")
    return result


def main(argv: list[str] | None = None) -> int:
    cli = parser()
    args = cli.parse_args(argv)
    if args.effort == "none":
        args.effort = None
    agents, arms, tasks = map(base.csv_values, (args.agents, args.arms, args.tasks))
    for label, values, allowed in (("agents", agents, base.AGENTS), ("arms", arms, ARMS), ("tasks", tasks, TASKS)):
        if not values or len(values) != len(set(values)) or any(value not in allowed for value in values):
            cli.error(f"invalid or duplicate {label}")
    if args.repetitions < 1 or not math.isfinite(args.timeout) or not 0 < args.timeout <= 3600:
        cli.error("repetitions must be positive and timeout in (0, 3600]")
    try:
        commits = json.loads(args.source_commits_json)
        if not isinstance(commits, dict) or any(not isinstance(k, str) or not isinstance(v, str) for k, v in commits.items()):
            raise ValueError("source commits must be a JSON object of strings")
        args.prefix = json.loads(args.headroom_prefix_json)
        if not isinstance(args.prefix, list) or any(not isinstance(part, str) or not part for part in args.prefix):
            raise ValueError("prefix must be a JSON list of nonempty strings")
    except (ValueError, json.JSONDecodeError) as error:
        cli.error(str(error))
    out = Path(args.out).resolve()
    if out.exists() and any(out.iterdir()):
        cli.error("output directory must be empty; completed evidence is never overwritten")
    selected = {"scopelet" if arm.startswith("scopelet") else arm for arm in arms}
    if "scopelet-caveman" in arms:
        selected.add("caveman")
    paths, skills, plugins = {}, {}, {}
    for name in selected & {"scopelet", "rtk", "headroom"}:
        value = getattr(args, name + "_binary")
        if args.live and (not value or not Path(value).is_file()):
            cli.error(f"--{name}-binary must name an existing file")
        paths[name] = Path(value).resolve() if value else Path(name)
    for name in selected & set(PLUGIN_NAMES):
        plugin_value = getattr(args, name + "_plugin")
        if plugin_value:
            if not (Path(plugin_value) / ".claude-plugin/plugin.json").is_file():
                cli.error(f"--{name}-plugin must contain .claude-plugin/plugin.json")
            plugins[name] = Path(plugin_value).resolve()
    for name in selected & {"scopelet", "caveman", "ponytail"}:
        value = getattr(args, name + "_skill")
        if name in plugins:
            continue
        if args.live and (not value or not (Path(value) / "SKILL.md").is_file()):
            cli.error(f"--{name}-skill must contain SKILL.md, or pass --{name}-plugin" if name != "scopelet" else "--scopelet-skill must contain SKILL.md")
        if value:
            skills[name] = Path(value).resolve()
    if args.live and "headroom" in selected and not args.prefix:
        cli.error("headroom requires an explicit --headroom-prefix-json")
    if args.live and "claude" in agents and not shutil.which(args.claude_bin) and not Path(args.claude_bin).is_file():
        cli.error(f"--claude-bin not found: {args.claude_bin}")
    out.mkdir(parents=True, exist_ok=True)
    if args.live:
        for name in sorted(paths):
            if name != "headroom":
                paths[name] = base.freeze_binary(paths[name], out / "frozen" / name)
        for name in sorted(skills):
            destination = out / "frozen" / "skills" / name
            shutil.copytree(skills[name], destination)
            skills[name] = destination
        for name in sorted(plugins):
            destination = out / "frozen" / "plugins" / name
            shutil.copytree(plugins[name], destination, ignore=shutil.ignore_patterns(".git"), symlinks=True)
            plugins[name] = destination
        frozen_prefix = []
        for index, part in enumerate(args.prefix):
            source = Path(part)
            if source.is_file() and source.suffix == ".py":
                destination = out / "frozen" / "prefix" / f"{index}-{source.name}"
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, destination)
                part = str(destination)
            frozen_prefix.append(part)
        args.prefix = frozen_prefix
    args.plugins = plugins
    claude_bin = Path(shutil.which(args.claude_bin) or args.claude_bin)
    planned = cases(agents, arms, tasks, args.repetitions, args.seed)
    limitations = [
        "Pilot-scale repetitions; not powered for quality noninferiority",
        "Provider caches are observed, not forcibly cold",
        "Effort is requested through --effort and cannot be verified from provider output; thinking tokens are an indirect indicator",
        "Claude Code built-in skills and tools remain visible to every arm (shared host context)",
        "Global Codex skill paths explicitly disabled; other host instructions may remain",
        "Plugin marker files are written under the real HOME because an isolated CLAUDE_CONFIG_DIR loses OAuth authentication",
        "RTK hook rewrites only commands RTK recognizes and skips piped commands; Headroom compression adoption requires proxy telemetry",
        "Reported cost is Claude Code's list-basis estimate, not an invoice",
    ]
    report = {"meta": {"live": args.live, "seed": args.seed, "repetitions": args.repetitions,
               "timeout": args.timeout, "planned_runs": len(planned), "modes": MODES,
               "model_requested": args.model, "effort_requested": args.effort, "rtk_integration": args.rtk_integration,
               "claude_bin": str(claude_bin), "claude_bin_sha256": base.sha256_file(claude_bin) if claude_bin.is_file() else None,
               "environment_purged": list(base.PURGED_ENVIRONMENT),
               "harness_sha256": base.sha256_file(Path(__file__)), "base_harness_sha256": base.sha256_file(Path(base.__file__)),
               "limitations": limitations,
               "executables": {name: {"path": str(path), "sha256": base.sha256_file(path) if path.is_file() else None} for name, path in paths.items()},
               "executable_versions": {name: base.cli_version(path) for name, path in paths.items() if path.is_file()} if args.live else {},
               "skill_hashes": {name: base.fixture_hashes(path) if path.is_dir() else {} for name, path in skills.items()},
               "plugins": {name: plugin_provenance(name, path) for name, path in plugins.items()},
               "agent_versions": {agent: base.agent_cli_version(agent, claude_bin if agent == "claude" else None) for agent in agents} if args.live else {},
               "headroom_prefix": args.prefix, "prefix_file_hashes": {part: base.sha256_file(Path(part)) for part in args.prefix if Path(part).is_file()}, "disabled_codex_skills": [str(path) for path in global_skill_paths()],
               "source_commits": json.loads(args.source_commits_json)}, "planned_cases": planned, "runs": []}
    write_report(out, report)
    if not args.live:
        print(f"Dry-run plan: {out / 'report.json'} ({len(planned)} runs)")
        return 0
    for case in planned:
        print(f"Running {case}", flush=True)
        try:
            result = execute(case, args, paths, skills, out)
        except Exception as error:
            result = {**case, "harness_error": f"{type(error).__name__}: {error}",
                      "acceptance": {"grade": "harness_error", "passed": False}}
        report["runs"].append(result)
        write_report(out, report)
    return 0 if all(item["acceptance"]["passed"] and item.get("exit_code") == 0 and not item.get("timed_out") for item in report["runs"]) else 1


if __name__ == "__main__":
    raise SystemExit(main())
