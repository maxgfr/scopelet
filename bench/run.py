#!/usr/bin/env python3
"""Run bounded, reproducible Scopelet agent comparisons.

The default mode only writes a campaign plan.  Use --live explicitly to
start external agent processes.  The harness itself uses only Python's
standard library so that its measurements do not depend on a Python stack.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable


AGENTS = ("codex", "claude")
ARMS = ("baseline", "default", "ultra", "shell_control")
TASKS = ("task1", "task2", "task3")
ALL_TASKS = (*TASKS, "task4")
DEFAULT_CLAUDE_MODEL = "claude-haiku-4-5-20251001"
CLAUDE_EFFORTS = ("low", "medium", "high", "xhigh", "max")
CLAUDE_ALLOWED_TOOLS = "Read,Write,Edit,Glob,Grep,Bash,Skill"
# Provider routing and effort overrides must not leak from the operator's
# shell into measured sessions; the effort variable overrides --effort.
PURGED_ENVIRONMENT = ("CLAUDE_CODE_EFFORT_LEVEL", "ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")


def csv_values(value: str) -> list[str]:
    return [part.strip() for part in value.split(",") if part.strip()]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def fixture_hashes(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): sha256_file(path)
        for path in sorted(root.rglob("*"))
        if path.is_file() and ".git" not in path.parts
    }


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def records_jsonl() -> str:
    rows = []
    for index in range(1000):
        status = "failed" if index % 10 in (0, 1) else "passed"
        rows.append(
            json.dumps(
                {
                    "id": index,
                    "status": status,
                    "group": f"group-{index % 4}",
                    "payload": "stable synthetic record",
                },
                separators=(",", ":"),
            )
        )
    return "\n".join(rows) + "\n"


def _decoys(root: Path) -> None:
    for index in range(40):
        _write(
            root / "decoys" / f"decoy_{index:02d}.py",
            """# Deliberately similar names keep repository search honest.
def is_expired(expires_at, now):
    return expires_at < now

""",
        )


def checks_script() -> str:
    lines = [
        "import sys",
        "sys.path.insert(0, '.')",
        "from src.app import run_worker_check",
        "",
        "for index in range(1200):",
        "    package = f'packages/service_{index % 17:02d}'",
        "    test_name = f'test_pipeline_stage_{index % 29:02d}'",
        "    artifact = f'build/cache/{index:04d}.json'",
        "    print(f'[{index + 1:04d}/1200] {package}::{test_name} ... ok (artifact={artifact}, cases={index + 3})')",
        "",
        "if run_worker_check(0) != []:",
        "    print('Traceback (most recent call last):')",
        "    print('  File \\\"src/worker.py\\\", line 4, in worker_count')",
        "    print('AssertionError: limit=0 must select zero workers; None means default workers')",
        "    raise SystemExit(1)",
        "if run_worker_check(None) != ['alpha', 'beta', 'gamma']:",
        "    print('Traceback (most recent call last):')",
        "    print('AssertionError: limit=None must retain the default worker set')",
        "    raise SystemExit(1)",
        "print('[1201/1200] checks complete ... passed')",
    ]
    return "\n".join(lines) + "\n"


def create_fixture(root: Path, task: str) -> dict[str, Any]:
    """Create one deterministic task workspace and its independent grader."""
    (root / "src").mkdir(parents=True, exist_ok=True)
    if task == "task1":
        _write(
            root / "src/cache.py",
            """def is_expired(expires_at: int, now: int) -> bool:
    # Boundary behavior is intentionally wrong for this exercise.
    return expires_at < now
""",
        )
        _decoys(root)
        _write(
            root / "README.md",
            """The cache check used by app.py treats an item expiring exactly at the
current time as still valid. Correct that boundary behavior while preserving
the public helper signature and keeping the change small.
""",
        )
        _write(
            root / "app.py",
            """from src.cache import is_expired

def should_refresh(expires_at: int, now: int) -> bool:
    return is_expired(expires_at, now)
""",
        )
        acceptance = """import sys
sys.path.insert(0, ".")
from src.cache import is_expired

assert is_expired(9, 10) is True
assert is_expired(10, 10) is True
assert is_expired(11, 10) is False
print("task1 acceptance passed")
"""
    elif task == "task2":
        _write(root / "records.jsonl", records_jsonl())
        expected = {"failed_count": 200, "failed_by_group": {f"group-{i}": 50 for i in range(4)}}
        _write(
            root / "README.md",
            "Compute exact failed record counts by group from the supplied records.\n"
            "Write only the required JSON object to answer.json.\n",
        )
        acceptance = f"""import json
expected = {json.dumps(expected, sort_keys=True)}
with open("answer.json", encoding="utf-8") as stream:
    actual = json.load(stream)
assert actual == expected, (actual, expected)
print("task2 acceptance passed")
"""
    elif task == "task3":
        _write(
            root / "src/helpers.py",
            """def normalize_identifier(value: str) -> str:
    # Correct whitespace handling is the small requested repair.
    return value.lower().replace(" ", "_")
""",
        )
        _write(
            root / "README.md",
            "Correct src/helpers.py so normalize_identifier trims surrounding whitespace,\n"
            "lowercases, and collapses each run of internal whitespace to one underscore.\n",
        )
        acceptance = """import sys
sys.path.insert(0, ".")
from src.helpers import normalize_identifier

assert normalize_identifier("  Hello   Scopelet  ") == "hello_scopelet"
assert normalize_identifier("Already_OK") == "already_ok"
assert normalize_identifier(" one\\ttwo ") == "one_two"
print("task3 acceptance passed")
"""
    elif task == "task4":
        _write(
            root / "src/worker.py",
            """def worker_count(items, limit=None):
    # The check fixture distinguishes an explicit zero from the None default.
    if not limit:
        return list(items)
    return list(items[:limit])
""",
        )
        _write(
            root / "src/app.py",
            """from src.worker import worker_count

def run_worker_check(limit=None):
    return worker_count(["alpha", "beta", "gamma"], limit)
""",
        )
        _write(root / "checks.py", checks_script())
        _write(
            root / "README.md",
            "Run python3 checks.py once before editing to capture the failing build/test output.\n"
            "Repair the worker limit behavior through its application call path, then run the same command again.\n",
        )
        acceptance = """import sys
sys.path.insert(0, ".")
from src.app import run_worker_check
assert run_worker_check(0) == []
assert run_worker_check(None) == ["alpha", "beta", "gamma"]
print("task4 acceptance passed")
"""
    else:
        raise ValueError(f"unknown task: {task}")
    if task == "task2":
        acceptance = 'import json\nwith open("answer.json") as f: answer = json.load(f)\nassert set(answer) == {"failed_count", "failed_by_group"}\nassert isinstance(answer["failed_count"], int)\nprint("shape check passed; exact counts are graded externally")\n'
    _write(root / "acceptance.py", acceptance)
    return {"task": task, "expected": "acceptance.py"}


def pristine_acceptance_source(task: str) -> str:
    """Return grader code kept outside the agent workspace."""
    if task == "task1":
        return """import sys
sys.path.insert(0, ".")
from src.cache import is_expired

assert is_expired(9, 10) is True
assert is_expired(10, 10) is True
assert is_expired(11, 10) is False
print("task1 acceptance passed")
"""
    if task == "task2":
        expected = {"failed_count": 200, "failed_by_group": {f"group-{i}": 50 for i in range(4)}}
        return f"""import json
expected = {json.dumps(expected, sort_keys=True)}
with open("answer.json", encoding="utf-8") as stream:
    actual = json.load(stream)
assert actual == expected, (actual, expected)
print("task2 acceptance passed")
"""
    if task == "task3":
        return """import sys
sys.path.insert(0, ".")
from src.helpers import normalize_identifier

assert normalize_identifier("  Hello   Scopelet  ") == "hello_scopelet"
assert normalize_identifier("Already_OK") == "already_ok"
assert normalize_identifier(" one\\ttwo ") == "one_two"
print("task3 acceptance passed")
"""
    if task == "task4":
        return """import hashlib
import sys
sys.path.insert(0, ".")
from src.app import run_worker_check

assert run_worker_check(0) == []
assert run_worker_check(None) == ["alpha", "beta", "gamma"]
with open("checks.py", "rb") as stream:
    assert hashlib.sha256(stream.read()).hexdigest() == __CHECKS_HASH__
print("task4 acceptance passed")
""".replace("__CHECKS_HASH__", repr(sha256_bytes(checks_script().encode("utf-8"))))
    raise ValueError(f"unknown task: {task}")


def skill_copy(root: Path, skill: Path, arm: str) -> None:
    if arm not in ("default", "ultra"):
        return
    for parent in (".agents/skills/scopelet", ".claude/skills/scopelet"):
        destination = root / parent
        shutil.copytree(skill, destination, dirs_exist_ok=True)


def prompt_for(agent: str, task: str, arm: str, binary: Path) -> str:
    mode = {
        "default": "default",
        "ultra": "ultra",
    }.get(arm)
    if arm in ("default", "ultra"):
        method = (
            f"Invoke the copied Scopelet skill explicitly and use scopelet mode {mode}. "
            f"Run the command through the direct executable \"$SCOPELET_BIN\" ({binary}); do not rely on a login-shell PATH."
        )
        if task == "task4":
            method += f" Wrap both checks.py runs with \"$SCOPELET_BIN\" run --mode {mode} -- python3 checks.py."
    elif arm == "shell_control":
        method = "Use a shell control workflow: batch searches and reads up front, then process data locally before printing results. Do not invoke Scopelet."
    else:
        method = "Work directly with the available file and shell tools, inspect the relevant call path, and verify the result. Do not invoke Scopelet."
    task_text = {
        "task1": "Fix the cache expiration behavior so an item expiring at the exact current time is treated as expired. Find the implementation through the application call path and preserve its public interface.",
        "task2": "Use records.jsonl to compute the exact counts, then write answer.json with exactly two keys: failed_count (integer) and failed_by_group (group-name to count mapping).",
        "task3": "Correct normalize_identifier in src/helpers.py for trimming, lowercasing, and collapsing whitespace.",
        "task4": "Run python3 checks.py once before editing to capture its failing command output. Find the worker implementation through the application call path, fix the explicit-zero versus None behavior, then rerun python3 checks.py.",
    }[task]
    activation = "/scopelet " if agent == "claude" and arm in ("default", "ultra") else ""
    return activation + f"""You are working in the current isolated git workspace.
{task_text}
{method}
Inspect only what is needed, make the smallest correct edits, and verify the independent acceptance behavior before finishing.
Do not ask questions and do not report success until the requested file change is saved.
"""


def command_for(
    agent: str,
    model: str | None = None,
    effort: str | None = None,
    claude_bin: str | None = None,
    include_hook_events: bool = False,
) -> list[str]:
    """Build the agent argv; model/effort/binary apply to Claude Code only."""
    if agent == "codex":
        return [
            "codex",
            "exec",
            "--ignore-user-config",
            "--model",
            "gpt-5.6-luna",
            "-c",
            'model_reasoning_effort="low"',
            "--sandbox",
            "workspace-write",
            "--skip-git-repo-check",
            "--ephemeral",
            "--json",
        ]
    if agent == "claude":
        # User/global hooks and skills are excluded by --setting-sources
        # project; plugin hooks passed explicitly by a caller remain active
        # and are proven by the hook lifecycle events in the stream.
        command = [
            claude_bin or "claude",
            "-p",
            "--model",
            model or DEFAULT_CLAUDE_MODEL,
            "--output-format",
            "stream-json",
            "--verbose",
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--mcp-config",
            '{"mcpServers":{}}',
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            CLAUDE_ALLOWED_TOOLS,
            "--no-session-persistence",
        ]
        if effort is not None:
            if effort not in CLAUDE_EFFORTS:
                raise ValueError(f"unknown Claude effort: {effort}")
            command += ["--effort", effort]
        if include_hook_events:
            command.append("--include-hook-events")
        return command
    raise ValueError(f"unknown agent: {agent}")


def purged_environment(source: dict[str, str] | None = None) -> dict[str, str]:
    """Copy the environment without provider routing or effort overrides."""
    environment = dict(os.environ if source is None else source)
    for variable in PURGED_ENVIRONMENT:
        environment.pop(variable, None)
    return environment


def _walk_dicts(value: Any) -> Iterable[dict[str, Any]]:
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from _walk_dicts(child)
    elif isinstance(value, list):
        for child in value:
            yield from _walk_dicts(child)


def parse_json_stream(raw: bytes) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    for line in raw.splitlines():
        try:
            value = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        if isinstance(value, dict):
            events.append(value)
    return events


def _number(value: Any) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _first_number(mapping: dict[str, Any], names: tuple[str, ...]) -> int | None:
    for name in names:
        number = _number(mapping.get(name))
        if number is not None:
            return number
    return None


def _usage_from(mapping: dict[str, Any]) -> dict[str, Any]:
    reasoning = _first_number(mapping, ("reasoning_tokens", "reasoningTokens", "thinking_tokens", "reasoning_output_tokens"))
    details = mapping.get("output_tokens_details")
    if reasoning is None and isinstance(details, dict):
        reasoning = _first_number(details, ("thinking_tokens", "reasoning_tokens"))
    return {
        "input_tokens": _first_number(mapping, ("input_tokens", "inputTokens", "prompt_tokens")),
        "output_tokens": _first_number(mapping, ("output_tokens", "outputTokens", "completion_tokens")),
        "cache_read_input_tokens": _first_number(
            mapping,
            ("cache_read_input_tokens", "cache_read_tokens", "cached_input_tokens", "cachedInputTokens", "cacheReadInputTokens"),
        ),
        "cache_creation_input_tokens": _first_number(
            mapping,
            ("cache_creation_input_tokens", "cache_creation_tokens", "cacheCreationInputTokens", "cache_write_input_tokens"),
        ),
        "reasoning_tokens": reasoning,
    }


def _sum_usages(usages: list[dict[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "reasoning_tokens"):
        values = [usage[key] for usage in usages if usage.get(key) is not None]
        result[key] = sum(values) if values and len(values) == len(usages) else None
    return result


def _outer_usage(event: dict[str, Any]) -> dict[str, Any] | None:
    candidate = event.get("usage")
    return candidate if isinstance(candidate, dict) else None


def _model_usages(events: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    """Return the final result event's per-model usage, scalar fields only."""
    result: dict[str, dict[str, Any]] = {}
    for event in events:
        model_usage = event.get("modelUsage")
        if not isinstance(model_usage, dict):
            continue
        result = {}
        for name, candidate in model_usage.items():
            if isinstance(candidate, dict):
                result[str(name)] = {key: value for key, value in candidate.items() if isinstance(value, (int, float, str, bool)) or value is None}
    return result


def _init_event(events: list[dict[str, Any]]) -> dict[str, Any]:
    for event in events:
        if event.get("type") == "system" and event.get("subtype") == "init":
            return event
    return {}


def _result_event(events: list[dict[str, Any]]) -> dict[str, Any]:
    for event in reversed(events):
        if event.get("type") == "result":
            return event
    return {}


def _provider_fields(agent: str, events: list[dict[str, Any]], expected_model: str | None) -> dict[str, Any]:
    """Claude Code result/init metadata kept beside token usage, never combined with it."""
    if agent != "claude":
        return {"model_init": None, "models_observed": [], "model_usage": {}, "model_mismatch": None, "num_turns": None,
                "duration_ms": None, "duration_api_ms": None, "total_cost_usd_reported": None, "permission_denials_count": None,
                "permission_denials": [], "api_error_status": None, "fast_mode_state": None, "result_subtype": None, "result_is_error": None}
    init = _init_event(events)
    result = _result_event(events)
    model_usage = _model_usages(events)
    model_init = init.get("model") if isinstance(init.get("model"), str) else None
    denials = result.get("permission_denials") if isinstance(result.get("permission_denials"), list) else []
    denial_names = sorted({str(item.get("tool_name")) for item in denials if isinstance(item, dict) and item.get("tool_name")})
    mismatch: bool | None = None
    if expected_model is not None:
        # The primary model comes from the init event. Auxiliary models can
        # legitimately appear in modelUsage (Claude Code uses a small model
        # for side tasks), so they are recorded rather than treated as a
        # mismatch; an absent expected model in modelUsage is one.
        mismatch = model_init != expected_model or (bool(model_usage) and expected_model not in model_usage)
    cost = result.get("total_cost_usd")
    return {
        "model_init": model_init,
        "models_observed": sorted(model_usage),
        "model_usage": model_usage,
        "model_mismatch": mismatch,
        "num_turns": _number(result.get("num_turns")),
        "duration_ms": _number(result.get("duration_ms")),
        "duration_api_ms": _number(result.get("duration_api_ms")),
        "total_cost_usd_reported": cost if isinstance(cost, (int, float)) and not isinstance(cost, bool) else None,
        "permission_denials_count": len(denials) if result else None,
        "permission_denials": denial_names,
        "api_error_status": result.get("api_error_status") if isinstance(result.get("api_error_status"), (int, str)) else None,
        "fast_mode_state": result.get("fast_mode_state") if isinstance(result.get("fast_mode_state"), str) else (init.get("fast_mode_state") if isinstance(init.get("fast_mode_state"), str) else None),
        "result_subtype": result.get("subtype") if isinstance(result.get("subtype"), str) else None,
        "result_is_error": result.get("is_error") if isinstance(result.get("is_error"), bool) else None,
    }


def normalize_usage(agent: str, raw: bytes, expected_model: str | None = None) -> dict[str, Any]:
    """Normalize the final usage event while preserving absent fields as null.

    Codex's reported input token count already includes cached input. Claude's
    usage reports cache reads/creation separately, so those are added only to
    the logical input total for Claude. Reasoning/thinking tokens are already
    part of reported output and are never added again. When Claude's outer
    result usage is partial or absent, every model in modelUsage is summed.
    """
    events = parse_json_stream(raw)
    selected_raw: list[dict[str, Any]] = []
    selected: list[dict[str, Any]] = []
    if agent == "codex":
        completed = [
            _outer_usage(event)
            for event in events
            if event.get("type") == "turn.completed" and _outer_usage(event) is not None
        ]
        selected_raw = completed
        selected = [_usage_from(usage) for usage in selected_raw]
        if selected:
            usage = _sum_usages(selected)
        else:
            fallback = [_outer_usage(event) for event in events if _outer_usage(event) is not None]
            selected_raw = [fallback[-1]] if fallback else []
            selected = [_usage_from(selected_raw[0])] if selected_raw else []
            usage = selected[-1] if selected else _usage_from({})
    else:
        result_events = [
            _outer_usage(event)
            for event in events
            if event.get("type") == "result" and _outer_usage(event) is not None
        ]
        models = list(_model_usages(events).values())
        usage = _usage_from({})
        if result_events:
            selected_raw = [result_events[-1]]
            selected = [_usage_from(selected_raw[0])]
            usage = selected[0]
        if (usage["input_tokens"] is None or usage["output_tokens"] is None) and models:
            # Some Claude stream versions put complete accounting in
            # modelUsage while leaving the outer result partial or absent.
            summed = _sum_usages([_usage_from(model) for model in models])
            if summed["input_tokens"] is not None and summed["output_tokens"] is not None:
                selected_raw = models
                selected = [summed]
                usage = summed
    input_tokens = usage["input_tokens"]
    output_tokens = usage["output_tokens"]
    cache_read = usage["cache_read_input_tokens"]
    cache_creation = usage["cache_creation_input_tokens"]
    if agent == "claude":
        parts = [part for part in (input_tokens, cache_read, cache_creation) if part is not None]
        logical_input = sum(parts) if len(parts) == 3 else None
    else:
        logical_input = input_tokens
    return {
        "raw_usage": selected_raw,
        "raw_events_with_usage": len(selected_raw),
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_read_input_tokens": cache_read,
        "cache_creation_input_tokens": cache_creation,
        "reasoning_tokens": usage["reasoning_tokens"],
        "thinking_tokens": usage["reasoning_tokens"] if agent == "claude" else None,
        "logical_input_tokens": logical_input,
        "cache_included_in_reported_input": agent == "codex",
        "usage_missing": not bool(selected_raw) or input_tokens is None or output_tokens is None,
        **_provider_fields(agent, events, expected_model),
    }


def _tool_invocation(mapping: dict[str, Any]) -> tuple[str, str, str] | None:
    event_type = mapping.get("type")
    if not isinstance(event_type, str):
        return None
    kind = event_type.lower()
    if kind == "tool_use":
        label = str(mapping.get("name") or "tool_use")
    elif kind == "command_execution":
        label = "command_execution"
    elif kind in ("function_call", "function", "tool_call"):
        label = str(mapping.get("name") or kind)
    else:
        return None
    identifier = mapping.get("id") or mapping.get("tool_use_id") or mapping.get("call_id")
    payload = mapping.get("input") or mapping.get("arguments") or mapping.get("command") or ""
    if identifier is None:
        identifier = sha256_bytes(json.dumps({"label": label, "payload": payload}, sort_keys=True).encode("utf-8"))
    return str(identifier), label, json.dumps(payload, sort_keys=True) if not isinstance(payload, str) else payload


def _shell_tokens(command: str) -> list[str]:
    try:
        tokens = shlex.split(command)
    except ValueError:
        return []
    while tokens:
        # Environment assignments and the env/command wrappers do not change
        # which executable is actually invoked.
        if "=" in tokens[0] and not tokens[0].startswith("="):
            tokens.pop(0)
            continue
        if tokens[0] in ("env", "/usr/bin/env", "command", "/usr/bin/command", "exec"):
            tokens.pop(0)
            continue
        if tokens[0] in ("sh", "bash", "zsh", "/bin/sh", "/bin/bash", "/bin/zsh"):
            try:
                index = next(index for index, token in enumerate(tokens[1:], 1) if token in ("-c", "-lc", "-ec", "-lec"))
            except StopIteration:
                return []
            if index + 1 >= len(tokens):
                return []
            try:
                tokens = shlex.split(tokens[index + 1])
            except ValueError:
                return []
            continue
        break
    return tokens


def _unwrap_shell_script(command: str) -> str:
    """Unwrap a leading shell launcher while preserving its script text."""
    script = command.strip()
    for _ in range(4):
        try:
            tokens = shlex.split(script)
        except ValueError:
            return ""
        if not tokens:
            return ""
        executable = Path(tokens[0]).name
        if executable in ("sh", "bash", "zsh"):
            try:
                index = next(index for index, token in enumerate(tokens[1:], 1) if token in ("-c", "-lc", "-ec", "-lec"))
            except StopIteration:
                return script
            if index + 1 >= len(tokens):
                return ""
            script = tokens[index + 1]
            continue
        return script
    return script


def _split_shell_commands(script: str) -> list[str]:
    """Split unquoted shell statement operators, excluding heredoc bodies."""
    segments: list[str] = []
    current: list[str] = []
    quote: str | None = None
    escaped = False
    heredoc = False
    index = 0
    while index < len(script):
        char = script[index]
        if escaped:
            current.append(char)
            escaped = False
            index += 1
            continue
        if quote == "'":
            current.append(char)
            if char == "'":
                quote = None
            index += 1
            continue
        if quote == '"':
            current.append(char)
            if char == '"':
                quote = None
            elif char == "\\":
                escaped = True
            index += 1
            continue
        if char == "\\":
            current.append(char)
            escaped = True
            index += 1
            continue
        if char in ("'", '"'):
            quote = char
            current.append(char)
            index += 1
            continue
        if char == "#" and (not current or current[-1].isspace()):
            while index < len(script) and script[index] != "\n":
                index += 1
            continue
        if char == "<" and index + 1 < len(script) and script[index + 1] == "<":
            heredoc = True
            current.extend((char, char))
            index += 2
            continue
        if char == "\n":
            if current:
                segments.append("".join(current).strip())
                current.clear()
            if heredoc:
                break
            index += 1
            continue
        if char == ";" or char == "&" or char == "|":
            if current:
                segments.append("".join(current).strip())
                current.clear()
            if index + 1 < len(script) and script[index + 1] == char and char in ("&", "|"):
                index += 2
            else:
                index += 1
            continue
        current.append(char)
        index += 1
    if current:
        segments.append("".join(current).strip())
    return [segment for segment in segments if segment]


def _command_payload(payload: str) -> str:
    try:
        parsed = json.loads(payload)
    except json.JSONDecodeError:
        return payload
    if isinstance(parsed, list) and all(isinstance(item, str) for item in parsed):
        return shlex.join(parsed)
    if isinstance(parsed, dict):
        for key in ("command", "cmd", "script", "shell_command"):
            value = parsed.get(key)
            if isinstance(value, str):
                return value
            if isinstance(value, list) and all(isinstance(item, str) for item in value):
                return shlex.join(value)
    return payload


def _is_scopelet_invocation(label: str, payload: str) -> bool:
    command = _command_payload(payload)
    known_subcommands = {"query", "run", "expand"}
    for segment in _split_shell_commands(_unwrap_shell_script(command)):
        tokens = _shell_tokens(segment)
        if not tokens:
            continue
        if any(token in ("--help", "-h", "--version", "-V") for token in tokens):
            continue
        executable = Path(tokens[0]).name.lower()
        if executable in ("scopelet", "scopelet-bin") or tokens[0] in ("$SCOPELET_BIN", "${SCOPELET_BIN}"):
            args = tokens[1:]
            if args[:1] == ["--cache-dir"]:
                args = args[2:]
            elif args and args[0].startswith("--cache-dir="):
                args = args[1:]
            if bool(args) and args[0] in known_subcommands:
                return True
        if executable in ("node", "nodejs") and len(tokens) > 2:
            launcher = Path(tokens[1]).name.lower()
            if launcher in ("scopelet.mjs", "scopelet.js") and tokens[2] in known_subcommands:
                return True
    return False


def tool_usage(agent: str, stdout: bytes, stderr: bytes) -> dict[str, Any]:
    del stderr
    calls: dict[str, tuple[str, str]] = {}
    for event in parse_json_stream(stdout):
        for mapping in _walk_dicts(event):
            invocation = _tool_invocation(mapping)
            if invocation is None:
                continue
            identifier, label, payload = invocation
            calls.setdefault(identifier, (label, payload))
    counts: defaultdict[str, int] = defaultdict(int)
    scopelet_hits = 0
    for label, payload in calls.values():
        counts[label] += 1
        if _is_scopelet_invocation(label, payload):
            scopelet_hits += 1
    return {
        "tool_use_count": len(calls),
        "tool_counts": dict(sorted(counts.items())),
        "scopelet_invocations": scopelet_hits,
        "scopelet_adopted": scopelet_hits > 0,
        "skill_discovered": next((any("scopelet" in str(skill) for skill in event.get("skills", [])) for event in parse_json_stream(stdout) if agent == "claude" and event.get("subtype") == "init"), None),
        "skill_tool_invoked": any(label == "Skill" and "scopelet" in payload for label, payload in calls.values()) if agent == "claude" else None,
    }


def grade_workspace(
    task: str,
    workspace: Path,
    timeout: float = 15.0,
    expected_fixture_hashes: dict[str, str] | None = None,
) -> dict[str, Any]:
    started = time.monotonic()
    readonly_name = {"task2": "records.jsonl", "task4": "checks.py"}.get(task)
    if readonly_name is not None:
        expected_hash = (expected_fixture_hashes or {}).get(readonly_name)
        if expected_hash is None and task == "task2":
            expected_hash = sha256_bytes(records_jsonl().encode("utf-8"))
        if expected_hash is None and task == "task4":
            expected_hash = sha256_bytes(checks_script().encode("utf-8"))
        readonly_path = workspace / readonly_name
        if not readonly_path.is_file() or sha256_file(readonly_path) != expected_hash:
            return {
                "task": task,
                "grade": "fail",
                "passed": False,
                "exit_code": None,
                "timed_out": False,
                "stdout": "",
                "stderr": f"readonly fixture {readonly_name} was modified",
                "duration_seconds": round(time.monotonic() - started, 6),
            }
    with tempfile.TemporaryDirectory(prefix="scopelet-pristine-grader-") as grader_directory:
        grader = Path(grader_directory) / "acceptance.py"
        grader.write_text(pristine_acceptance_source(task), encoding="utf-8")
        try:
            result = subprocess.run(
                [sys.executable, str(grader)],
                cwd=workspace,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=timeout,
                check=False,
            )
            return {
                "task": task,
                "grade": "pass" if result.returncode == 0 else "fail",
                "passed": result.returncode == 0,
                "exit_code": result.returncode,
                "timed_out": False,
                "stdout": result.stdout.decode("utf-8", "replace"),
                "stderr": result.stderr.decode("utf-8", "replace"),
                "duration_seconds": round(time.monotonic() - started, 6),
            }
        except subprocess.TimeoutExpired as error:
            return {
                "task": task,
                "grade": "timeout",
                "passed": False,
                "exit_code": None,
                "timed_out": True,
                "stdout": (error.stdout or b"").decode("utf-8", "replace"),
                "stderr": (error.stderr or b"").decode("utf-8", "replace"),
                "duration_seconds": round(time.monotonic() - started, 6),
            }


def _kill_process_group(process: subprocess.Popen[bytes]) -> None:
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGKILL)
        else:
            process.kill()
    except (ProcessLookupError, OSError):
        pass


def run_agent(
    agent: str,
    prompt: str,
    workspace: Path,
    timeout: float,
    env: dict[str, str],
) -> dict[str, Any]:
    command = command_for(agent)
    started = time.monotonic()
    process: subprocess.Popen[bytes] | None = None
    stdout = b""
    stderr = b""
    timed_out = False
    spawn_error: str | None = None
    exit_code: int | None = None
    try:
        process = subprocess.Popen(
            command,
            cwd=workspace,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        stdout, stderr = process.communicate(prompt.encode("utf-8"), timeout=timeout)
        exit_code = process.returncode
    except FileNotFoundError as error:
        spawn_error = str(error)
    except subprocess.TimeoutExpired as error:
        timed_out = True
        if process is not None:
            _kill_process_group(process)
            # A second communicate() returns the complete buffered stream;
            # concatenating TimeoutExpired.output would duplicate its prefix.
            stdout, stderr = process.communicate()
            exit_code = process.returncode
    except OSError as error:
        spawn_error = str(error)
    duration = round(time.monotonic() - started, 6)
    return {
        "command": command,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "spawn_error": spawn_error,
        "duration_seconds": duration,
        "stdout": stdout,
        "stderr": stderr,
    }


def cli_version(binary: Path) -> dict[str, Any]:
    try:
        result = subprocess.run(
            [str(binary), "--version"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5, check=False
        )
        return {
            "available": True,
            "exit_code": result.returncode,
            "version": result.stdout.decode("utf-8", "replace").strip(),
            "stderr": result.stderr.decode("utf-8", "replace").strip(),
        }
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired) as error:
        return {"available": False, "error": str(error)}


def freeze_binary(binary: Path, out: Path) -> Path:
    """Copy one immutable executable into the campaign directory."""
    frozen = out / "bin" / binary.name
    frozen.parent.mkdir(parents=True, exist_ok=True)
    if binary.resolve() != frozen.resolve():
        shutil.copy2(binary, frozen)
    return frozen


def agent_cli_version(agent: str, binary: Path | None = None) -> dict[str, Any]:
    try:
        result = subprocess.run(
            [str(binary) if binary else agent, "--version"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=False
        )
        return {
            "available": True,
            "exit_code": result.returncode,
            "version": result.stdout.decode("utf-8", "replace").strip(),
            "stderr": result.stderr.decode("utf-8", "replace").strip(),
        }
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired) as error:
        return {"available": False, "error": str(error)}


def planned_cases(agents: list[str], arms: list[str], tasks: list[str]) -> list[dict[str, str]]:
    if "shell_control" in arms and "task1" not in tasks:
        raise ValueError("shell_control is supported for task1 only")
    return [
        {"agent": agent, "task": task, "arm": arm}
        for agent in agents
        for task in tasks
        for arm in arms
        if arm != "shell_control" or task == "task1"
    ]


def _git_init(root: Path) -> None:
    subprocess.run(["git", "init", "-q"], cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)


def _copy_tree_after(workspace: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    for path in workspace.iterdir():
        if path.name == ".git":
            continue
        target = destination / path.name
        if path.is_dir():
            shutil.copytree(path, target, dirs_exist_ok=True)
        else:
            shutil.copy2(path, target)


def execute_case(case: dict[str, str], config: argparse.Namespace, binary: Path, skill: Path, out: Path) -> dict[str, Any]:
    run_id = f"{case['agent']}_{case['task']}_{case['arm']}"
    run_dir = out / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix=f"scopelet-{run_id}-") as temporary:
        workspace = Path(temporary)
        create_fixture(workspace, case["task"])
        hashes = fixture_hashes(workspace)
        skill_copy(workspace, skill, case["arm"])
        _git_init(workspace)
        prompt = prompt_for(case["agent"], case["task"], case["arm"], binary)
        (run_dir / "prompt.txt").write_text(prompt, encoding="utf-8")
        environment = os.environ.copy()
        environment["SCOPELET_BIN"] = str(binary)
        cache = workspace / "scopelet-cache"
        cache.mkdir(parents=True, exist_ok=True)
        environment["SCOPELET_CACHE_DIR"] = str(cache)
        environment["PATH"] = str(binary.parent) + os.pathsep + environment.get("PATH", "")
        invocation = run_agent(case["agent"], prompt, workspace, config.timeout, environment)
        stdout = invocation.pop("stdout")
        stderr = invocation.pop("stderr")
        (run_dir / "stdout.raw").write_bytes(stdout)
        (run_dir / "stderr.raw").write_bytes(stderr)
        grade = grade_workspace(case["task"], workspace, expected_fixture_hashes=hashes)
        _copy_tree_after(workspace, run_dir / "workspace_after")
        usage = normalize_usage(case["agent"], stdout)
        tools = tool_usage(case["agent"], stdout, stderr)
        result = {
            **case,
            "run_id": run_id,
            "binary": str(binary),
            "binary_sha256": sha256_file(binary) if binary.is_file() else None,
            "fixture_hashes": hashes,
            "prompt_file": str((run_dir / "prompt.txt").relative_to(out)),
            "stdout_file": str((run_dir / "stdout.raw").relative_to(out)),
            "stderr_file": str((run_dir / "stderr.raw").relative_to(out)),
            "workspace_after": str((run_dir / "workspace_after").relative_to(out)),
            "cache_dir": str((run_dir / "workspace_after/scopelet-cache").relative_to(out)),
            "acceptance": grade,
            "usage": usage,
            "tools": tools,
            "started_monotonic": started,
            "duration_seconds": round(time.monotonic() - started, 6),
            **invocation,
        }
        (run_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
        return result


def make_markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Scopelet agent benchmark",
        "",
        "This report records isolated runs and acceptance outcomes. It does not estimate cost or combine unlike agents into a global average.",
        "",
        f"- Live campaign: `{report['meta']['live']}`",
        f"- Planned runs: `{report['meta']['planned_runs']}`",
        f"- Scopelet CLI: `{report['meta']['cli_version'].get('version', 'unavailable')}`",
        "",
        "## Per-agent/task comparisons",
        "",
    ]
    groups: defaultdict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for run in report.get("runs", []):
        groups[(run["agent"], run["task"])].append(run)
    for (agent, task), runs in sorted(groups.items()):
        lines.append(f"### {agent} / {task}")
        lines.append("")
        lines.append("| Arm | Grade | Agent exit | Timed out | Scopelet uses | Input tokens | Output tokens | Duration (s) |")
        lines.append("|---|---|---:|---:|---:|---:|---:|---:|")
        for run in runs:
            usage = run["usage"]
            lines.append(
                f"| {run['arm']} | {run['acceptance']['grade']} | {run.get('exit_code')} | "
                f"{run.get('timed_out')} | {run['tools']['scopelet_invocations']} | "
                f"{usage['logical_input_tokens']} | {usage['output_tokens']} | {run['duration_seconds']} |"
            )
        lines.append("")
    if not report.get("runs"):
        lines.append("No live runs were executed. Inspect report.json for the planned command lines.")
        lines.append("")
    return "\n".join(lines)


def write_report(out: Path, report: dict[str, Any]) -> None:
    out.mkdir(parents=True, exist_ok=True)
    (out / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True), encoding="utf-8")
    (out / "report.md").write_text(make_markdown(report), encoding="utf-8")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--agents", default=",".join(AGENTS), help="comma-separated agent names")
    result.add_argument("--arms", default=",".join(ARMS), help="comma-separated arms")
    result.add_argument("--tasks", default=",".join(TASKS), help="comma-separated tasks")
    result.add_argument("--out", default="bench/runs/campaign", help="report directory")
    result.add_argument("--timeout", type=float, default=180.0, help="per-agent timeout in seconds")
    result.add_argument("--skill", default=None, help="scopelet skill bundle directory")
    result.add_argument("--binary", default=None, help="scopelet binary path")
    result.add_argument("--live", action="store_true", help="run external agents")
    result.add_argument("--dry-run", action="store_true", help="write plan only (default)")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    if args.live and args.dry_run:
        parser().error("--live and --dry-run are mutually exclusive")
    agents, arms, tasks = map(csv_values, (args.agents, args.arms, args.tasks))
    for name, values, allowed in (("agents", agents, AGENTS), ("arms", arms, ARMS), ("tasks", tasks, ALL_TASKS)):
        unknown = [value for value in values if value not in allowed]
        if unknown:
            parser().error(f"unknown {name}: {', '.join(unknown)}")
    try:
        cases = planned_cases(agents, arms, tasks)
    except ValueError as error:
        parser().error(str(error))
    if not cases:
        parser().error("at least one agent, arm, and task must be selected")
    if args.timeout <= 0:
        parser().error("--timeout must be positive")
    out = Path(args.out).resolve()
    binary = Path(args.binary).resolve() if args.binary else Path(shutil.which("scopelet") or "scopelet")
    skill = Path(args.skill).resolve() if args.skill else Path(__file__).resolve().parents[1] / "skills/scopelet"
    if args.live:
        if not binary.exists():
            parser().error(f"scopelet binary does not exist: {binary}")
        binary = freeze_binary(binary, out)
    if args.live and skill.is_dir():
        frozen_skill = out / "skill"
        if frozen_skill.exists():
            parser().error("output already has a frozen skill; choose a new campaign directory")
        shutil.copytree(skill, frozen_skill)
        skill = frozen_skill
    binary_hash = sha256_file(binary) if binary.is_file() else None
    skill_hashes = fixture_hashes(skill) if skill.is_dir() else {}
    meta = {
        "live": bool(args.live),
        "planned_runs": len(cases),
        "agents": agents,
        "arms": arms,
        "tasks": tasks,
        "timeout_seconds": args.timeout,
        "binary": str(binary),
        "binary_sha256": binary_hash,
        "skill": str(skill),
        "skill_hashes": skill_hashes,
        "cli_version": cli_version(binary) if args.live else {"available": None, "reason": "dry_run"},
        "agent_cli_versions": {agent: agent_cli_version(agent) for agent in agents} if args.live else {
            agent: {"available": None, "reason": "dry_run"} for agent in agents
        },
        "no_cost_estimate": True,
    }
    report: dict[str, Any] = {"meta": meta, "plan": [{**case, "command": command_for(case["agent"])} for case in cases], "runs": []}
    if args.live:
        if not skill.is_dir() and any(case["arm"] in ("default", "ultra") for case in cases):
            parser().error(f"scopelet skill bundle does not exist: {skill}")
        for case in cases:
            report["runs"].append(execute_case(case, args, binary, skill, out))
            write_report(out, report)
            print(json.dumps({"completed": case, "grade": report["runs"][-1]["acceptance"]["grade"]}), flush=True)
    write_report(out, report)
    print(json.dumps({"out": str(out), "live": args.live, "planned_runs": len(cases), "completed_runs": len(report["runs"])}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
