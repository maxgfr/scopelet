#!/usr/bin/env python3
"""Offline tests for the benchmark harness; no agent process is started."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import run


class HarnessTests(unittest.TestCase):
    def test_observed_reasoning_aliases_do_not_inflate_output(self):
        codex = run.normalize_usage("codex", json.dumps({"type":"turn.completed", "usage":{"input_tokens":100,"output_tokens":20,"reasoning_output_tokens":7,"cache_write_input_tokens":0}}).encode())
        self.assertEqual(codex["reasoning_tokens"], 7)
        self.assertEqual(codex["output_tokens"], 20)
        self.assertEqual(codex["cache_creation_input_tokens"], 0)
        claude = run.normalize_usage("claude", json.dumps({"type":"result", "usage":{"input_tokens":10,"output_tokens":20,"output_tokens_details":{"thinking_tokens":8}}}).encode())
        self.assertEqual(claude["reasoning_tokens"], 8)
        self.assertEqual(claude["output_tokens"], 20)

    def test_help_is_not_evidence_adoption(self):
        self.assertFalse(run._is_scopelet_invocation("Bash", "scopelet run --help"))
        self.assertFalse(run._is_scopelet_invocation("Bash", "scopelet bench"))

    def test_commented_commands_do_not_count_as_adoption(self):
        self.assertFalse(run._is_scopelet_invocation("Bash", "echo note # ignore; scopelet query --file x"))
        self.assertTrue(run._is_scopelet_invocation("Bash", "echo note # ignore; fake\n\"$SCOPELET_BIN\" query --file x"))

    def test_default_plan_has_twenty_runs_and_shell_control_only_task1(self) -> None:
        cases = run.planned_cases(list(run.AGENTS), list(run.ARMS), list(run.TASKS))
        self.assertEqual(len(cases), 20)
        self.assertTrue(all(case["task"] == "task1" for case in cases if case["arm"] == "shell_control"))

    def test_shell_control_is_rejected_for_non_task1(self) -> None:
        with self.assertRaises(ValueError):
            run.planned_cases(["codex"], ["shell_control"], ["task2"])

    def test_task4_is_opt_in_and_uses_three_non_control_arms(self) -> None:
        cases = run.planned_cases(list(run.AGENTS), ["baseline", "default", "ultra"], ["task4"])
        self.assertEqual(len(cases), 6)

    def test_task4_prompt_requires_pre_and_post_command_runs(self) -> None:
        treatment = run.prompt_for("codex", "task4", "default", Path("/tmp/scopelet"))
        baseline = run.prompt_for("codex", "task4", "baseline", Path("/tmp/scopelet"))
        self.assertIn('"$SCOPELET_BIN" run --mode default -- python3 checks.py', treatment)
        self.assertIn("once before editing", treatment)
        self.assertIn("then rerun python3 checks.py", baseline)

    def test_codex_usage_keeps_reported_input_as_logical_input(self) -> None:
        raw = b'{"type":"item.completed","usage":{"input_tokens":999,"output_tokens":999}}\n{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":17}}\n'
        usage = run.normalize_usage("codex", raw)
        self.assertEqual(usage["input_tokens"], 100)
        self.assertEqual(usage["logical_input_tokens"], 100)
        self.assertEqual(usage["cache_read_input_tokens"], 60)
        self.assertEqual(usage["raw_usage"][0]["cached_input_tokens"], 60)
        self.assertFalse(usage["usage_missing"])

    def test_claude_usage_adds_cache_components_for_logical_input(self) -> None:
        raw = b'{"type":"result","usage":{"input_tokens":100,"cache_read_input_tokens":60,"cache_creation_input_tokens":20,"output_tokens":17}}\n'
        usage = run.normalize_usage("claude", raw)
        self.assertEqual(usage["input_tokens"], 100)
        self.assertEqual(usage["logical_input_tokens"], 180)
        self.assertEqual(usage["output_tokens"], 17)

    def test_claude_model_usage_is_fallback_when_result_usage_is_partial(self) -> None:
        raw = b'{"type":"result","usage":{"output_tokens":17},"modelUsage":{"haiku":{"inputTokens":100,"outputTokens":17,"cacheReadInputTokens":60,"cacheCreationInputTokens":20}}}\n'
        usage = run.normalize_usage("claude", raw)
        self.assertEqual(usage["input_tokens"], 100)
        self.assertEqual(usage["logical_input_tokens"], 180)

    def test_claude_multi_model_fallback_sums_models_and_records_them(self) -> None:
        raw = (b'{"type":"system","subtype":"init","model":"claude-fable-5-1","fast_mode_state":"off"}\n'
               b'{"type":"result","usage":{"output_tokens":14},"modelUsage":{'
               b'"claude-haiku-4-5-20251001":{"inputTokens":899,"outputTokens":10,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"thinkingTokens":0,"costUSD":0.0009},'
               b'"claude-fable-5-1":{"inputTokens":2,"outputTokens":4,"cacheReadInputTokens":10078,"cacheCreationInputTokens":7202,"thinkingTokens":0,"costUSD":0.1467}},'
               b'"num_turns":1,"duration_ms":2838,"duration_api_ms":3792,"total_cost_usd":0.1477,"permission_denials":[{"tool_name":"Bash"}],"api_error_status":null,"subtype":"success","is_error":false}\n')
        usage = run.normalize_usage("claude", raw, expected_model="claude-fable-5-1")
        self.assertEqual(usage["input_tokens"], 901)
        self.assertEqual(usage["logical_input_tokens"], 901 + 10078 + 7202)
        self.assertEqual(usage["models_observed"], ["claude-fable-5-1", "claude-haiku-4-5-20251001"])
        self.assertEqual(usage["model_init"], "claude-fable-5-1")
        self.assertFalse(usage["model_mismatch"])
        self.assertEqual(usage["model_usage"]["claude-fable-5-1"]["costUSD"], 0.1467)
        self.assertEqual((usage["num_turns"], usage["duration_ms"], usage["duration_api_ms"]), (1, 2838, 3792))
        self.assertEqual(usage["total_cost_usd_reported"], 0.1477)
        self.assertEqual((usage["permission_denials_count"], usage["permission_denials"]), (1, ["Bash"]))
        self.assertEqual(usage["fast_mode_state"], "off")
        self.assertFalse(usage["usage_missing"])

    def test_claude_model_mismatch_is_flagged_but_usage_is_retained(self) -> None:
        raw = (b'{"type":"system","subtype":"init","model":"claude-haiku-4-5-20251001"}\n'
               b'{"type":"result","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens_details":{"thinking_tokens":8}},'
               b'"modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":10,"outputTokens":20}}}\n')
        usage = run.normalize_usage("claude", raw, expected_model="claude-fable-5-1")
        self.assertTrue(usage["model_mismatch"])
        self.assertEqual(usage["thinking_tokens"], 8)
        self.assertEqual(usage["output_tokens"], 20)
        self.assertEqual(usage["logical_input_tokens"], 10)
        self.assertIsNone(run.normalize_usage("claude", raw)["model_mismatch"])
        self.assertIsNone(run.normalize_usage("codex", b'{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}\n')["model_init"])

    def test_claude_command_pins_model_effort_and_hook_events(self) -> None:
        default = run.command_for("claude")
        self.assertEqual(default[default.index("--model") + 1], run.DEFAULT_CLAUDE_MODEL)
        self.assertNotIn("--effort", default)
        self.assertNotIn("--settings", default)
        self.assertNotIn("--include-hook-events", default)
        pinned = run.command_for("claude", "claude-fable-5-1", "high", "/opt/homebrew/bin/claude", include_hook_events=True)
        self.assertEqual(pinned[0], "/opt/homebrew/bin/claude")
        self.assertEqual(pinned[pinned.index("--model") + 1], "claude-fable-5-1")
        self.assertEqual(pinned[pinned.index("--effort") + 1], "high")
        self.assertIn("--include-hook-events", pinned)
        self.assertEqual(pinned[pinned.index("--setting-sources") + 1], "project")
        with self.assertRaises(ValueError):
            run.command_for("claude", effort="maximum")
        self.assertEqual(run.command_for("codex"), run.command_for("codex", "claude-fable-5-1", "high"))

    def test_purged_environment_drops_provider_overrides_only(self) -> None:
        source = {"CLAUDE_CODE_EFFORT_LEVEL": "low", "ANTHROPIC_BASE_URL": "http://x", "ANTHROPIC_API_KEY": "k", "ANTHROPIC_AUTH_TOKEN": "t", "PATH": "/usr/bin", "HOME": "/home/me"}
        purged = run.purged_environment(source)
        self.assertEqual(purged, {"PATH": "/usr/bin", "HOME": "/home/me"})
        self.assertEqual(source["ANTHROPIC_API_KEY"], "k")

    def test_missing_usage_is_null_and_flagged(self) -> None:
        usage = run.normalize_usage("codex", b'{"type":"message","text":"done"}\n')
        self.assertIsNone(usage["input_tokens"])
        self.assertIsNone(usage["output_tokens"])
        self.assertTrue(usage["usage_missing"])

    def test_tool_usage_deduplicates_lifecycle_events_and_ignores_mentions(self) -> None:
        raw = (
            b'{"type":"assistant","text":"Use scopelet for this task"}\n'
            b'{"type":"item.started","item":{"type":"command_execution","id":"c1","command":"scopelet query --file records.jsonl"}}\n'
            b'{"type":"item.completed","item":{"type":"command_execution","id":"c1","command":"scopelet query --file records.jsonl"}}\n'
            b'{"type":"item.started","item":{"type":"command_execution","id":"c2","command":"cat .agents/skills/scopelet/SKILL.md"}}\n'
        )
        tools = run.tool_usage("codex", raw, b"stderr mentions scopelet")
        self.assertEqual(tools["tool_use_count"], 2)
        self.assertEqual(tools["scopelet_invocations"], 1)
        self.assertTrue(tools["scopelet_adopted"])

    def test_prose_scopelet_mention_does_not_count_as_adoption(self) -> None:
        tools = run.tool_usage("claude", b'{"type":"assistant","text":"invoke scopelet"}\n', b"")
        self.assertEqual(tools["tool_use_count"], 0)
        self.assertFalse(tools["scopelet_adopted"])

    def test_adoption_requires_command_position_and_known_subcommand(self) -> None:
        raw = (
            b'{"type":"tool_use","id":"a","name":"Bash","input":{"command":"sed -n \'1,240p\' /tmp/.agents/skills/scopelet/SKILL.md"}}\n'
            b'{"type":"tool_use","id":"b","name":"Bash","input":{"command":"ls /tmp/scopelet"}}\n'
            b'{"type":"tool_use","id":"c","name":"Bash","input":{"command":"zsh -lc \'$SCOPELET_BIN query --file records.jsonl\'"}}\n'
            b'{"type":"tool_use","id":"d","name":"Bash","input":{"command":"node .agents/skills/scopelet/scripts/scopelet.mjs run echo ok"}}\n'
            b'{"type":"tool_use","id":"e","name":"Bash","input":{"command":"ls; printf ready; \\\"$SCOPELET_BIN\\\" query --file records.jsonl"}}\n'
            b'{"type":"tool_use","id":"f","name":"Bash","input":{"command":"cat <<EOF\\n$SCOPELET_BIN query --file records.jsonl\\nEOF"}}\n'
        )
        tools = run.tool_usage("claude", raw, b"")
        self.assertEqual(tools["tool_use_count"], 6)
        self.assertEqual(tools["scopelet_invocations"], 3)

    def test_grader_accepts_good_and_rejects_bad_task1_workspace(self) -> None:
        with tempfile.TemporaryDirectory(prefix="scopelet-harness-test-") as directory:
            root = Path(directory)
            run.create_fixture(root, "task1")
            good = root / "src/cache.py"
            good.write_text(
                "def is_expired(expires_at: int, now: int) -> bool:\n    return expires_at <= now\n",
                encoding="utf-8",
            )
            self.assertTrue(run.grade_workspace("task1", root)["passed"])
            good.write_text(
                "def is_expired(expires_at: int, now: int) -> bool:\n    return expires_at < now\n",
                encoding="utf-8",
            )
            bad = run.grade_workspace("task1", root)
            self.assertFalse(bad["passed"])
            self.assertEqual(bad["grade"], "fail")

    def test_grader_accepts_exact_task2_json(self) -> None:
        with tempfile.TemporaryDirectory(prefix="scopelet-harness-test-") as directory:
            root = Path(directory)
            run.create_fixture(root, "task2")
            expected = {
                "failed_count": 200,
                "failed_by_group": {f"group-{i}": 50 for i in range(4)},
            }
            (root / "answer.json").write_text(json.dumps(expected), encoding="utf-8")
            self.assertTrue(run.grade_workspace("task2", root)["passed"])
            (root / "records.jsonl").write_text("rewritten by agent\n", encoding="utf-8")
            self.assertFalse(run.grade_workspace("task2", root)["passed"])

    def test_grader_ignores_tampered_visible_acceptance(self) -> None:
        with tempfile.TemporaryDirectory(prefix="scopelet-harness-test-") as directory:
            root = Path(directory)
            run.create_fixture(root, "task1")
            (root / "src/cache.py").write_text(
                "def is_expired(expires_at: int, now: int) -> bool:\n    return expires_at <= now\n",
                encoding="utf-8",
            )
            (root / "acceptance.py").write_text("print('fake pass')\n", encoding="utf-8")
            self.assertTrue(run.grade_workspace("task1", root)["passed"])

    def test_task4_runs_large_output_and_grades_good_bad_without_editable_golden(self) -> None:
        with tempfile.TemporaryDirectory(prefix="scopelet-harness-test-") as directory:
            root = Path(directory)
            run.create_fixture(root, "task4")
            before = subprocess.run(
                [sys.executable, "checks.py"], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False
            )
            self.assertNotEqual(before.returncode, 0)
            self.assertEqual(len(before.stdout.splitlines()), 1203)
            self.assertIn(b"AssertionError: limit=0", before.stdout)
            (root / "src/worker.py").write_text(
                "def worker_count(items, limit=None):\n    if limit is None:\n        return list(items)\n    return list(items[:limit])\n",
                encoding="utf-8",
            )
            self.assertTrue(run.grade_workspace("task4", root)["passed"])
            (root / "src/worker.py").write_text(
                "def worker_count(items, limit=None):\n    return list(items)\n",
                encoding="utf-8",
            )
            self.assertFalse(run.grade_workspace("task4", root)["passed"])
            (root / "checks.py").write_text("print('fake checks')\n", encoding="utf-8")
            self.assertIn("modified", run.grade_workspace("task4", root)["stderr"])


if __name__ == "__main__":
    unittest.main()
