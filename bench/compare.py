#!/usr/bin/env python3
"""Bounded competitor pilot; writes a plan unless --live is explicit.

This reuses the existing frozen-fixture grader and provider usage accounting.
Modes have different integration boundaries; this is not a universal ranking.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import random
import shlex
import shutil
import subprocess
import tempfile
import time
from typing import Any

import run as base

ARMS = ("native", "concise", "scopelet", "scopelet-ultra", "scopelet-caveman", "caveman", "ponytail", "rtk", "headroom")
TASKS = ("task4", "task2", "task3")
CONCISE = "Keep narration and the final report concise. Preserve exact code, commands, quantities, negation, evidence, and unresolved limitations."
MODES = {
    "native": "Unchanged native file/shell workflow",
    "concise": "Native workflow plus a short prose-only instruction",
    "scopelet": "Project skill and explicit Scopelet default CLI guidance",
    "scopelet-ultra": "Project skill and explicit Scopelet ultra CLI guidance",
    "scopelet-caveman": "Scopelet ultra CLI plus full upstream Caveman communication skill",
    "caveman": "Full upstream project skill; no automatic host hook installation",
    "ponytail": "Full upstream project skill; no automatic host hook installation",
    "rtk": "Manual RTK test wrapper guidance; no automatic host hook installation",
    "headroom": "Externally supplied Headroom proxy prefix wrapping native agent CLI",
}


def cases(agents: list[str], arms: list[str], tasks: list[str], repetitions: int, seed: int) -> list[dict[str, Any]]:
    result = [{"agent": agent, "task": task, "arm": arm, "repetition": repetition}
              for repetition in range(1, repetitions + 1) for agent in agents for task in tasks for arm in arms]
    random.Random(seed).shuffle(result)
    return result


def prompt_for(case: dict[str, Any], binary: Path) -> str:
    arm, agent, task = case["arm"], case["agent"], case["task"]
    if arm.startswith("scopelet"):
        prompt = base.prompt_for(agent, task, "default" if arm == "scopelet" else "ultra", binary)
        if arm == "scopelet-caveman":
            prompt += "Also invoke the copied caveman skill in full mode for communication. Keep Scopelet ultra for the local evidence operations.\n"
            if agent == "codex":
                prompt = "$caveman " + prompt
        return "$scopelet " + prompt if agent == "codex" else prompt
    prompt = base.prompt_for(agent, task, "baseline", binary)
    if arm == "concise":
        prompt += CONCISE + "\n"
    elif arm in ("caveman", "ponytail"):
        activation = "/" if agent == "claude" else "$"
        prompt = activation + arm + " " + prompt
        prompt += f"Use the copied {arm} skill for this task.\n"
    elif arm == "rtk":
        prompt += 'RTK is available at "$RTK_BIN". Use it for supported noisy test output. '
        if task == "task4":
            prompt += 'Wrap both required checks with CLAUDE_CONFIG_DIR="$RTK_LOCAL_CONFIG" "$RTK_BIN" test python3 checks.py. '
        prompt += 'Recover omitted output with CLAUDE_CONFIG_DIR="$RTK_LOCAL_CONFIG" "$RTK_BIN" recall HASH --grep PATTERN when needed. Use native tools for operations outside RTK supported commands.\n'
    return prompt


def global_skill_paths() -> list[Path]:
    roots = [Path.home() / ".agents/skills", Path.home() / ".codex/skills"]
    return sorted({path.absolute() for root in roots if root.is_dir() for path in root.rglob("SKILL.md") if path.is_file()})


def agent_command(case: dict[str, Any], paths: dict[str, Path], prefix: list[str]) -> list[str]:
    command = base.command_for(case["agent"])
    if case["agent"] == "codex":
        disabled = global_skill_paths()
        if disabled:
            command += ["-c", "skills.config=[" + ",".join("{path=" + json.dumps(str(path)) + ",enabled=false}" for path in disabled) + "]"]
    if case["arm"] == "headroom":
        if not prefix:
            raise ValueError("headroom arm requires --headroom-prefix-json; no proxy syntax is guessed")
        command = [str(paths["headroom"]) if part == "{headroom}" else part for part in prefix] + command
    return command


def adoption(agent: str, raw: bytes) -> dict[str, Any]:
    result = base.tool_usage(agent, raw, b"")
    calls = {}
    for event in base.parse_json_stream(raw):
        for value in base._walk_dicts(event):
            invocation = base._tool_invocation(value)
            if invocation:
                identifier, label, payload = invocation
                calls.setdefault(identifier, (label, payload))
    rtk = 0
    checks = 0
    skills = {name: False for name in ("caveman", "ponytail")}
    for label, payload in calls.values():
        if label == "Skill":
            try:
                arguments = json.loads(payload)
            except json.JSONDecodeError:
                arguments = {}
            for name in skills:
                if isinstance(arguments, dict) and arguments.get("skill") == name:
                    skills[name] = True
        if label.lower() not in ("bash", "command_execution", "exec_command", "shell", "shell_command"):
            continue
        for segment in base._split_shell_commands(base._unwrap_shell_script(base._command_payload(payload))):
            tokens = base._shell_tokens(segment)
            if tokens and (Path(tokens[0]).name in ("python", "python3", "scopelet", "rtk") or tokens[0] in ("$SCOPELET_BIN", "${SCOPELET_BIN}", "$RTK_BIN", "${RTK_BIN}")):
                if "checks.py" in tokens[1:] and not any(arg in ("--help", "-h", "--version", "-V") for arg in tokens):
                    checks += 1
            if tokens and (Path(tokens[0]).name == "rtk" or tokens[0] in ("$RTK_BIN", "${RTK_BIN}")):
                if len(tokens) > 1 and not any(arg in ("--help", "-h", "--version", "-V") for arg in tokens):
                    rtk += 1
    result.update({"rtk_invocations": rtk, "checks_command_invocations": checks,
                   "checks_sequence_verified": None, "skill_tool_invocations": skills,
                   "skill_compliance": "not inferred from short output or discovery; inspect raw trace",
                   "headroom_compression_verified": None})
    return result


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


def execute(case: dict[str, Any], args: argparse.Namespace, paths: dict[str, Path], skills: dict[str, Path], out: Path) -> dict[str, Any]:
    run_id = f"{case['agent']}_{case['task']}_{case['arm']}_{case['repetition']}"
    run_dir = out / "runs" / run_id
    run_dir.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix="scopelet-comparison-") as temporary:
        workspace = Path(temporary)
        base.create_fixture(workspace, case["task"])
        hashes = base.fixture_hashes(workspace)
        name = "scopelet" if case["arm"].startswith("scopelet") else case["arm"]
        skill_names = ["scopelet", "caveman"] if case["arm"] == "scopelet-caveman" else [name]
        for skill_name in skill_names:
            if skill_name in skills:
                for parent in (".agents/skills", ".claude/skills"):
                    shutil.copytree(skills[skill_name], workspace / parent / skill_name)
        base._git_init(workspace)
        prompt = prompt_for(case, paths.get("scopelet", Path("scopelet")))
        (run_dir / "prompt.txt").write_text(prompt)
        env = os.environ.copy()
        # Avoid carrying treatment paths into native and skill-only arms.
        for variable in ("SCOPELET_BIN", "SCOPELET_CACHE_DIR", "RTK_BIN"):
            env.pop(variable, None)
        if name == "scopelet":
            env["SCOPELET_BIN"] = str(paths[name])
            env["SCOPELET_CACHE_DIR"] = str(workspace / "scopelet-cache")
        if name == "rtk":
            env["RTK_BIN"] = str(paths[name])
            env["RTK_TELEMETRY_DISABLED"] = "1"
            env["RTK_LOCAL_CONFIG"] = str(workspace / ".rtk-claude")
            env["NO_COLOR"] = "1"
            env["RTK_DB_PATH"] = str(workspace / "rtk-tracking.db")
            env["RTK_RECALL_DB"] = str(workspace / "rtk-recall.db")
        invocation = invoke(agent_command(case, paths, args.prefix), prompt, workspace, args.timeout, env)
        stdout, stderr = invocation.pop("stdout"), invocation.pop("stderr")
        (run_dir / "stdout.raw").write_bytes(stdout)
        (run_dir / "stderr.raw").write_bytes(stderr)
        result = {**case, **invocation, "run_id": run_id, "integration": MODES[case["arm"]],
                  "fixture_hashes": hashes, "acceptance": base.grade_workspace(case["task"], workspace, expected_fixture_hashes=hashes),
                  "usage": base.normalize_usage(case["agent"], stdout), "tools": adoption(case["agent"], stdout),
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
    lines = ["# Direct competitor pilot", "", "Single repetitions are exploratory; integration modes differ. No universal ranking is established.", "",
             "| Agent | Task | Arm | Repeat | Grade | Exit | Input | Output | Seconds |",
             "|---|---|---|---:|---|---:|---:|---:|---:|"]
    for item in report["runs"]:
        usage = item.get("usage", {})
        lines.append(f"| {item['agent']} | {item['task']} | {item['arm']} | {item['repetition']} | {item.get('acceptance', {}).get('grade', 'harness_error')} | {item.get('exit_code')} | {usage.get('logical_input_tokens')} | {usage.get('output_tokens')} | {item.get('duration_seconds')} |")
    (out / "report.md").write_text("\n".join(lines) + "\n")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--agents", default="codex,claude")
    result.add_argument("--arms", default=",".join(ARMS))
    result.add_argument("--tasks", default="task4")
    result.add_argument("--repetitions", type=int, default=1)
    result.add_argument("--seed", type=int, default=20260909)
    result.add_argument("--timeout", type=float, default=300)
    result.add_argument("--out", default="bench/runs/direct-comparison")
    result.add_argument("--scopelet-binary")
    result.add_argument("--scopelet-skill", default=str(Path(__file__).resolve().parents[1] / "skills/scopelet"))
    result.add_argument("--caveman-skill")
    result.add_argument("--ponytail-skill")
    result.add_argument("--rtk-binary")
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
    paths, skills = {}, {}
    for name in selected & {"scopelet", "rtk", "headroom"}:
        value = getattr(args, name + "_binary")
        if args.live and (not value or not Path(value).is_file()):
            cli.error(f"--{name}-binary must name an existing file")
        paths[name] = Path(value).resolve() if value else Path(name)
    for name in selected & {"scopelet", "caveman", "ponytail"}:
        value = getattr(args, name + "_skill")
        if args.live and (not value or not (Path(value) / "SKILL.md").is_file()):
            cli.error(f"--{name}-skill must contain SKILL.md")
        if value:
            skills[name] = Path(value).resolve()
    if args.live and "headroom" in selected and not args.prefix:
        cli.error("headroom requires an explicit --headroom-prefix-json")
    out.mkdir(parents=True, exist_ok=True)
    if args.live:
        for name in sorted(paths):
            if name != "headroom":
                paths[name] = base.freeze_binary(paths[name], out / "frozen" / name)
        for name in sorted(skills):
            destination = out / "frozen" / "skills" / name
            shutil.copytree(skills[name], destination)
            skills[name] = destination
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
    planned = cases(agents, arms, tasks, args.repetitions, args.seed)
    report = {"meta": {"live": args.live, "seed": args.seed, "repetitions": args.repetitions,
               "timeout": args.timeout, "planned_runs": len(planned), "modes": MODES,
               "harness_sha256": base.sha256_file(Path(__file__)), "base_harness_sha256": base.sha256_file(Path(base.__file__)),
               "limitations": ["Exploratory pilot, not powered for quality noninferiority", "Provider caches are observed, not forcibly cold", "Global Codex skill paths explicitly disabled; other host instructions may remain", "RTK hooks and competitor hooks are not installed", "Headroom compression adoption requires separate proxy telemetry"],
               "executables": {name: {"path": str(path), "sha256": base.sha256_file(path) if path.is_file() else None} for name, path in paths.items()},
               "skill_hashes": {name: base.fixture_hashes(path) if path.is_dir() else {} for name, path in skills.items()},
               "agent_versions": {agent: base.agent_cli_version(agent) for agent in agents} if args.live else {},
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
