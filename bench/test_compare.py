"""Competitor harness checks without model calls."""
import json
import argparse
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import compare
import run as base


def options(**overrides):
    values = {"prefix": [], "timeout": 1, "model": compare.DEFAULT_MODEL, "effort": compare.DEFAULT_EFFORT,
              "claude_bin": "claude", "plugins": {}, "rtk_integration": "hook"}
    values.update(overrides)
    return argparse.Namespace(**values)


def claude_call(command, output="", is_error=False, call_id="1", tool="Bash"):
    return (json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": call_id, "name": tool, "input": {"command": command}}]}}) + "\n"
            + json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": call_id, "content": output, "is_error": is_error}]}}) + "\n").encode()


class CompareTests(unittest.TestCase):
    def test_randomized_plan_is_reproducible_and_complete(self):
        cases = compare.cases(["codex", "claude"], list(compare.ARMS), ["task4"], 2, 7)
        self.assertEqual(len(cases), 36)
        self.assertEqual(len({tuple(sorted(item.items())) for item in cases}), 36)
        self.assertEqual(cases, compare.cases(["codex", "claude"], list(compare.ARMS), ["task4"], 2, 7))
        self.assertNotEqual(cases, compare.cases(["codex", "claude"], list(compare.ARMS), ["task4"], 2, 8))

    def test_declared_fable_campaign_plans_fifty_four_runs(self):
        cases = compare.cases(["claude"], list(compare.ARMS), list(compare.TASKS), 2, 20260911)
        self.assertEqual(len(cases), 54)
        self.assertEqual(len({tuple(sorted(item.items())) for item in cases}), 54)

    def test_prompts_activate_only_selected_skill(self):
        for agent, prefix in (("claude", "/"), ("codex", "$")):
            for arm in ("caveman", "ponytail", "scopelet", "scopelet-ultra", "scopelet-caveman"):
                prompt = compare.prompt_for({"agent": agent, "arm": arm, "task": "task4"}, Path("/bin/scopelet"))
                name = "scopelet" if arm.startswith("scopelet") else arm
                self.assertTrue(prompt.startswith(prefix + name + " "))
                if arm == "scopelet-caveman":
                    self.assertIn("caveman skill in full mode", prompt)
        native = compare.prompt_for({"agent": "claude", "arm": "native", "task": "task4"}, Path("scopelet"))
        self.assertEqual(native, base.prompt_for("claude", "task4", "baseline", Path("scopelet")))

    def test_plugin_activation_uses_namespaced_skill_and_hook_rtk_keeps_native_prompt(self):
        prompt = compare.prompt_for({"agent": "claude", "arm": "caveman", "task": "task3"}, Path("scopelet"), plugins=("caveman",))
        self.assertTrue(prompt.startswith("/caveman:caveman "))
        self.assertIn("caveman plugin skill", prompt)
        combined = compare.prompt_for({"agent": "claude", "arm": "scopelet-caveman", "task": "task3"}, Path("scopelet"), plugins=("caveman",))
        self.assertTrue(combined.startswith("/scopelet "))
        self.assertIn("caveman plugin skill in full mode", combined)
        hook = compare.prompt_for({"agent": "claude", "arm": "rtk", "task": "task4"}, Path("scopelet"), rtk_integration="hook")
        self.assertEqual(hook, base.prompt_for("claude", "task4", "baseline", Path("scopelet")))
        manual = compare.prompt_for({"agent": "claude", "arm": "rtk", "task": "task4"}, Path("scopelet"), rtk_integration="manual")
        self.assertIn('"$RTK_BIN" test python3 checks.py', manual)

    def test_claude_command_pins_model_effort_hooks_and_plugins(self):
        command = compare.agent_command({"agent": "claude", "arm": "native"}, {}, [], options(claude_bin="/opt/claude"))
        self.assertEqual(command[0], "/opt/claude")
        self.assertEqual(command[command.index("--model") + 1], "claude-fable-5-1")
        self.assertEqual(command[command.index("--effort") + 1], "high")
        self.assertIn("--include-hook-events", command)
        self.assertNotIn("--settings", command)
        self.assertNotIn('{"disableAllHooks":true}', command)
        plugin = compare.agent_command({"agent": "claude", "arm": "ponytail"}, {}, [], options(plugins={"ponytail": Path("/plugins/ponytail")}))
        self.assertEqual(plugin[plugin.index("--plugin-dir") + 1], "/plugins/ponytail")
        combined = compare.agent_command({"agent": "claude", "arm": "scopelet-caveman"}, {}, [], options(plugins={"caveman": Path("/plugins/caveman")}))
        self.assertEqual(combined[combined.index("--plugin-dir") + 1], "/plugins/caveman")
        legacy = compare.agent_command({"agent": "claude", "arm": "caveman"}, {}, [], options())
        self.assertNotIn("--plugin-dir", legacy)
        with self.assertRaises(ValueError):
            compare.agent_command({"agent": "codex", "arm": "caveman"}, {}, [], options(plugins={"caveman": Path("/p")}))

    def test_rtk_hook_is_injected_as_documented_settings_json(self):
        command = compare.agent_command({"agent": "claude", "arm": "rtk"}, {"rtk": Path("/frozen/rtk")}, [], options())
        settings = json.loads(command[command.index("--settings") + 1])
        hooks = settings["hooks"]["PreToolUse"]
        self.assertEqual(hooks[0]["matcher"], "Bash")
        self.assertEqual(hooks[0]["hooks"][0], {"type": "command", "command": "/frozen/rtk hook claude"})
        manual = compare.agent_command({"agent": "claude", "arm": "rtk"}, {"rtk": Path("/frozen/rtk")}, [], options(rtk_integration="manual"))
        self.assertNotIn("--settings", manual)

    def test_headroom_prefix_is_explicit_and_keeps_native_argv(self):
        case = {"agent": "claude", "arm": "headroom"}
        with self.assertRaises(ValueError):
            compare.agent_command(case, {"headroom": Path("/headroom")}, [])
        actual = compare.agent_command(case, {"headroom": Path("/headroom")}, ["{headroom}", "custom"])
        self.assertEqual(actual, ["/headroom", "custom"] + base.command_for("claude", include_hook_events=True))

    def test_codex_global_disabling_does_not_refer_to_project_skills(self):
        with patch.object(compare, "global_skill_paths", return_value=[Path('/tmp/home/skill/SKILL.md')]):
            command = compare.agent_command({"agent": "codex", "arm": "native"}, {}, [])
        self.assertEqual(command[-1], 'skills.config=[{path="/tmp/home/skill/SKILL.md",enabled=false}]')

    def test_adoption_ignores_prose_comments_and_help(self):
        def output(command):
            return json.dumps({"type": "tool_use", "id": "1", "name": "Bash", "input": {"command": command}}).encode()
        for command in ('echo "rtk test python3 checks.py"', '# rtk test python3 checks.py', 'rtk --help'):
            self.assertEqual(compare.adoption("claude", output(command))["rtk_invocations"], 0)
        self.assertEqual(compare.adoption("claude", output('"$RTK_BIN" test python3 checks.py'))["rtk_invocations"], 1)
        recall = compare.adoption("claude", claude_call("rtk recall abc123 --grep Assertion"))
        self.assertEqual((recall["rtk_invocations"], recall["rtk_recall_invocations"]), (1, 1))

    def test_checks_sequence_requires_failure_then_success(self):
        failing = "[1200/1200] ... ok\nTraceback\nAssertionError: limit=0 must select zero workers"
        passing = "[1201/1200] checks complete ... passed"
        raw = claude_call("python3 checks.py", failing, True, "a") + claude_call("python3 checks.py", passing, False, "b")
        tools = compare.adoption("claude", raw, "task4")
        self.assertTrue(tools["checks_sequence_verified"])
        self.assertEqual(tools["checks_command_invocations"], 2)
        self.assertEqual(tools["tool_error_count"], 1)
        reversed_raw = claude_call("python3 checks.py", passing, False, "a") + claude_call("python3 checks.py", failing, True, "b")
        self.assertFalse(compare.adoption("claude", reversed_raw, "task4")["checks_sequence_verified"])
        single = claude_call('"$SCOPELET_BIN" run --mode ultra -- python3 checks.py', '{"exit_code":1,"result":{}}', False, "a")
        self.assertFalse(compare.adoption("claude", single, "task4")["checks_sequence_verified"])
        wrapped = single + claude_call('"$SCOPELET_BIN" run --mode ultra -- python3 checks.py', '{"exit_code":0,"result":{}}', False, "b")
        self.assertTrue(compare.adoption("claude", wrapped, "task4")["checks_sequence_verified"])
        self.assertIsNone(compare.adoption("claude", wrapped, "task3")["checks_sequence_verified"])

    def test_checks_sequence_reads_persisted_output_and_echoed_exit_status(self):
        preview = "<persisted-output>\nOutput too large (127.1KB). Full output saved to: /x/tool-results/abc.txt\n\nPreview (first 2KB):\n[0001/1200] ... ok\n</persisted-output>"
        raw = (claude_call('python3 checks.py; echo "EXIT=$?"', preview, False, "a")
               + claude_call("grep -v ' ok ' /x/tool-results/abc.txt", "Traceback\nAssertionError: limit=0 must select zero workers\nEXIT=1", False, "b")
               + claude_call("cat src/worker.py", "def worker_count", False, "c")
               + claude_call('python3 checks.py | tail -3; echo "checks exit=${PIPESTATUS[0]}"', "[1201/1200] checks complete ... passed\nchecks exit=0", False, "d"))
        tools = compare.adoption("claude", raw, "task4")
        self.assertTrue(tools["checks_sequence_verified"])
        self.assertEqual([run["truncated"] for run in tools["checks_runs"]], [True, False])
        self.assertTrue(tools["checks_runs"][0]["failure_evidence"])
        unknown_then_pass = (claude_call('python3 checks.py', preview, False, "a")
                             + claude_call('python3 checks.py >/dev/null; echo "checks exit=$?"', "checks exit=0", False, "b"))
        self.assertFalse(compare.adoption("claude", unknown_then_pass, "task4")["checks_sequence_verified"])
        exit_ten = claude_call('python3 checks.py; echo "exit=$?"', "exit=10", False, "a") + claude_call('python3 checks.py; echo "exit=$?"', "exit=0", False, "b")
        self.assertTrue(compare.adoption("claude", exit_ten, "task4")["checks_sequence_verified"])
        binary = "/campaign/frozen/scopelet/bin/scopelet-0.1.3-aarch64-apple-darwin"
        by_path = (claude_call(f"{binary} run -- python3 checks.py", '{"exit_code":1,"result":{}}', True, "a")
                   + claude_call(f"{binary} run -- python3 checks.py", '{"exit_code":0,"result":{}}', False, "b"))
        tools = compare.adoption("claude", by_path, "task4")
        self.assertTrue(tools["checks_sequence_verified"])
        self.assertEqual((tools["scopelet_invocations"], tools["checks_command_invocations"]), (2, 2))

    def test_cache_reingestion_counts_only_returned_cache_paths(self):
        hit = "scopelet-cache/blobs/abc:1:cached line\n"
        raw = claude_call("rg --hidden limit", "src/worker.py:3:limit\n" + hit + "scopelet-cache/artifacts/def:1:more\n")
        tools = compare.adoption("claude", raw)
        self.assertEqual(tools["scopelet_cache_reingested_bytes"], len(hit.strip()) + len("scopelet-cache/artifacts/def:1:more"))
        prose = claude_call("echo scopelet-cache/blobs", "")
        self.assertEqual(compare.adoption("claude", prose)["scopelet_cache_reingested_bytes"], 0)

    def test_hook_events_expand_and_retrieve_counters(self):
        events = (json.dumps({"type": "system", "subtype": "hook_started", "hook_name": "SessionStart:startup", "hook_event": "SessionStart"}) + "\n"
                  + json.dumps({"type": "system", "subtype": "hook_response", "hook_name": "SessionStart:startup", "hook_event": "SessionStart", "output": "CAVEMAN MODE ACTIVE"}) + "\n"
                  + json.dumps({"type": "system", "subtype": "hook_response", "hook_name": "PreToolUse:Bash", "hook_event": "PreToolUse", "output": ""}) + "\n").encode()
        raw = events + claude_call('"$SCOPELET_BIN" expand artifact:' + "a" * 64, "{}", False, "x")
        raw += json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "m", "name": "mcp__headroom__headroom_retrieve", "input": {"id": "1"}}]}}).encode() + b"\n"
        raw += json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "s", "name": "Skill", "input": {"skill": "caveman:caveman"}}]}}).encode() + b"\n"
        tools = compare.adoption("claude", raw)
        self.assertEqual(tools["hook_events"]["started"], {"SessionStart:startup": 1})
        self.assertEqual(tools["hook_events"]["responses"], {"PreToolUse:Bash": 1, "SessionStart:startup": 1})
        self.assertEqual(tools["hook_events"]["responses_with_output"], {"SessionStart:startup": 1})
        self.assertEqual(tools["scopelet_expand_invocations"], 1)
        self.assertEqual(tools["headroom_retrieve_calls"], 1)
        self.assertTrue(tools["skill_tool_invocations"]["caveman"])
        audit = "2026-09-09T10:45:39 | rewrite | git status | rtk git status\n2026-09-09T10:45:41 | skip:defer | /usr/bin/git status | \n"
        summary = compare.adoption("claude", raw, rtk_audit=audit)["rtk_audit"]
        self.assertEqual(summary, {"lines": 2, "actions": {"rewrite": 1, "skip:defer": 1}, "rewrites": 1})
        self.assertIsNone(tools["rtk_audit"])

    def test_run_environment_purges_provider_overrides_and_scopes_treatments(self):
        with tempfile.TemporaryDirectory() as tmp, patch.dict(compare.os.environ, {
                "CLAUDE_CODE_EFFORT_LEVEL": "low", "ANTHROPIC_BASE_URL": "http://proxy", "ANTHROPIC_API_KEY": "k",
                "HEADROOM_OUTPUT_SHAPER": "1", "SCOPELET_BIN": "/stale", "RTK_HOOK_AUDIT": "1", "PATH": "/usr/bin"}):
            workspace, run_dir = Path(tmp) / "ws", Path(tmp) / "run"
            paths = {"scopelet": Path("/frozen/scopelet"), "rtk": Path("/frozen/bin/rtk")}
            native = compare.run_environment({"arm": "native"}, options(), paths, workspace, run_dir)
            for variable in ("CLAUDE_CODE_EFFORT_LEVEL", "ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "HEADROOM_OUTPUT_SHAPER", "SCOPELET_BIN", "RTK_HOOK_AUDIT"):
                self.assertNotIn(variable, native)
            ultra = compare.run_environment({"arm": "scopelet-ultra"}, options(), paths, workspace, run_dir)
            self.assertEqual(ultra["SCOPELET_CACHE_DIR"], str(workspace / "scopelet-cache"))
            rtk = compare.run_environment({"arm": "rtk"}, options(), paths, workspace, run_dir)
            self.assertEqual(rtk["RTK_HOOK_AUDIT"], "1")
            self.assertTrue(rtk["PATH"].startswith("/frozen/bin:"))
            self.assertEqual(rtk["RTK_TELEMETRY_DISABLED"], "1")
            caveman = compare.run_environment({"arm": "scopelet-caveman"}, options(plugins={"caveman": Path("/p")}), paths, workspace, run_dir)
            self.assertEqual((caveman["CAVEMAN_DEFAULT_MODE"], caveman["DO_NOT_TRACK"]), ("full", "1"))
            headroom = compare.run_environment({"arm": "headroom"}, options(), paths, workspace, run_dir)
            self.assertEqual(headroom["HEADROOM_BEACON"], "off")
            self.assertNotIn("HEADROOM_OUTPUT_SHAPER", headroom)

    def test_dry_run_cannot_start_processes_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(compare.subprocess, "Popen", side_effect=AssertionError("no model calls")):
            self.assertEqual(compare.main(["--dry-run", "--out", tmp]), 0)
            report = json.loads((Path(tmp) / "report.json").read_text())
            self.assertEqual(report["meta"]["planned_runs"], 18)
            self.assertEqual(report["meta"]["model_requested"], "claude-fable-5-1")
            self.assertEqual(report["meta"]["effort_requested"], "high")
            self.assertFalse(report["runs"])
            with self.assertRaises(SystemExit):
                compare.main(["--dry-run", "--out", tmp])

    def test_dry_run_declared_campaign_has_fifty_four_seeded_cases(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(compare.subprocess, "Popen", side_effect=AssertionError("no model calls")):
            self.assertEqual(compare.main(["--dry-run", "--agents", "claude", "--tasks", "task4,task2,task3", "--repetitions", "2",
                                           "--seed", "20260911", "--timeout", "900", "--out", tmp]), 0)
            report = json.loads((Path(tmp) / "report.json").read_text())
            self.assertEqual(report["meta"]["planned_runs"], 54)
            self.assertEqual(report["planned_cases"], compare.cases(["claude"], list(compare.ARMS), list(compare.TASKS), 2, 20260911))
            self.assertEqual(report["meta"]["timeout"], 900)

    def test_rate_limit_rejection_is_detected_from_stream_events(self):
        rejected = (json.dumps({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": 1788945000, "rateLimitType": "five_hour"}}) + "\n"
                    + json.dumps({"type": "result", "subtype": "success", "is_error": True, "api_error_status": 429, "result": "You've hit your session limit", "num_turns": 1}) + "\n").encode()
        self.assertEqual(compare.rate_limit_reset(rejected), 1788945000)
        allowed = (json.dumps({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed", "resetsAt": 1788945000}}) + "\n"
                   + json.dumps({"type": "result", "subtype": "success", "is_error": False, "api_error_status": None, "result": "ok"}) + "\n").encode()
        self.assertIsNone(compare.rate_limit_reset(allowed))
        self.assertEqual(compare.rate_limit_reset(b'{"type":"result","api_error_status":429}\n'), 0)

    def test_rate_limited_attempts_are_kept_apart_and_the_case_is_retried(self):
        limited = {"agent": "claude", "task": "task3", "arm": "native", "repetition": 1, "run_id": "claude_task3_native_1",
                   "rate_limit_reset": 1, "acceptance": {"grade": "fail", "passed": False}, "usage": {"num_turns": 1}, "exit_code": 1}
        measured = {**limited, "rate_limit_reset": None, "acceptance": {"grade": "pass", "passed": True}, "usage": {"num_turns": 5}, "exit_code": 0}
        outcomes = iter([limited, measured])
        waits = []

        def fake_execute(case, args, paths, skills, out):
            (out / "runs" / "claude_task3_native_1").mkdir(parents=True)
            return dict(next(outcomes))

        with tempfile.TemporaryDirectory() as tmp, patch.object(compare, "execute", side_effect=fake_execute), \
                patch.object(compare.time, "sleep", side_effect=waits.append), \
                patch.object(compare.subprocess, "Popen", side_effect=AssertionError("no model calls")), \
                patch.object(base, "agent_cli_version", return_value={"available": False}), \
                patch.object(compare.shutil, "which", return_value="/bin/true"):
            code = compare.main(["--live", "--agents", "claude", "--arms", "native", "--tasks", "task3", "--reset-margin", "5", "--out", tmp])
            report = json.loads((Path(tmp) / "report.json").read_text())
            self.assertTrue((Path(tmp) / "aborted" / "claude_task3_native_1_attempt1").is_dir())
        self.assertEqual(code, 0)
        self.assertEqual(len(report["runs"]), 1)
        self.assertTrue(report["runs"][0]["acceptance"]["passed"])
        self.assertEqual(len(report["aborted_attempts"]), 1)
        self.assertEqual(report["aborted_attempts"][0]["attempt"], 1)
        self.assertEqual(len(waits), 1)
        self.assertGreaterEqual(waits[0], 5)

    def test_modified_checks_are_rejected_by_shared_external_grader(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            base.create_fixture(root, "task4")
            original = base.fixture_hashes(root)
            (root / "checks.py").write_text("print('pass')")
            grade = base.grade_workspace("task4", root, expected_fixture_hashes=original)
            self.assertFalse(grade["passed"])
            self.assertIn("modified", grade["stderr"])

    def test_partial_report_preserves_error_and_missing_usage(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = {"runs": [{"agent": "claude", "task": "task4", "arm": "headroom", "repetition": 1,
                                "harness_error": "integration unavailable", "acceptance": {"grade": "harness_error"}}]}
            compare.write_report(Path(tmp), report)
            self.assertEqual(json.loads((Path(tmp) / "report.json").read_text()), report)
            self.assertIn("None", (Path(tmp) / "report.md").read_text())

    def test_archive_failure_preserves_measured_outcome(self):
        measured = {"stdout": b'{"type":"turn.completed","usage":{"input_tokens":31,"output_tokens":9}}',
                    "stderr": b"", "exit_code": 0, "timed_out": False}
        with tempfile.TemporaryDirectory() as tmp, \
                patch.object(compare, "invoke", return_value=measured), \
                patch.object(base, "grade_workspace", return_value={"passed": True, "grade": "pass"}), \
                patch.object(base, "_copy_tree_after", side_effect=OSError("unreadable generated file")):
            result = compare.execute({"agent": "codex", "task": "task3", "arm": "native", "repetition": 1},
                                     options(), {}, {}, Path(tmp))
        self.assertTrue(result["acceptance"]["passed"])
        self.assertEqual(result["usage"]["logical_input_tokens"], 31)
        self.assertEqual(result["usage"]["output_tokens"], 9)
        self.assertIn("artifact_error", result)

    def test_execute_records_model_mismatch_and_cache_markers(self):
        stream = (json.dumps({"type": "system", "subtype": "init", "model": "claude-haiku-4-5-20251001"}) + "\n"
                  + json.dumps({"type": "result", "usage": {"input_tokens": 3, "output_tokens": 4, "cache_read_input_tokens": 1, "cache_creation_input_tokens": 2},
                                "modelUsage": {"claude-haiku-4-5-20251001": {"inputTokens": 3, "outputTokens": 4}}, "num_turns": 2}) + "\n").encode()
        measured = {"stdout": stream, "stderr": b"", "exit_code": 0, "timed_out": False}

        def fake_invoke(command, prompt, workspace, timeout, env):
            for name in ("blobs", "artifacts"):
                (workspace / "scopelet-cache" / name).mkdir(parents=True)
                (workspace / "scopelet-cache" / name / ".ignore").write_text("*\n")
                (workspace / "scopelet-cache" / name / ".gitignore").write_text("*\n")
            self.assertEqual(env["SCOPELET_CACHE_DIR"], str(workspace / "scopelet-cache"))
            return dict(measured)

        with tempfile.TemporaryDirectory() as tmp, patch.object(compare, "invoke", side_effect=fake_invoke), \
                patch.object(base, "grade_workspace", return_value={"passed": True, "grade": "pass"}):
            result = compare.execute({"agent": "claude", "task": "task3", "arm": "scopelet-ultra", "repetition": 1},
                                     options(), {"scopelet": Path("/frozen/scopelet")}, {}, Path(tmp))
        self.assertTrue(result["usage"]["model_mismatch"])
        self.assertEqual(result["usage"]["model_init"], "claude-haiku-4-5-20251001")
        self.assertEqual(result["usage"]["num_turns"], 2)
        self.assertEqual(result["model_requested"], "claude-fable-5-1")
        self.assertEqual(result["effort_requested"], "high")
        self.assertEqual(result["cache_markers_present"], {"blobs": True, "artifacts": True})
        self.assertIn("Project skill", result["integration"])


if __name__ == "__main__":
    unittest.main()
