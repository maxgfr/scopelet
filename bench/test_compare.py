"""Competitor harness checks without model calls."""
import json
import argparse
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import compare
import run as base


class CompareTests(unittest.TestCase):
    def test_randomized_plan_is_reproducible_and_complete(self):
        cases = compare.cases(["codex", "claude"], list(compare.ARMS), ["task4"], 2, 7)
        self.assertEqual(len(cases), 36)
        self.assertEqual(len({tuple(sorted(item.items())) for item in cases}), 36)
        self.assertEqual(cases, compare.cases(["codex", "claude"], list(compare.ARMS), ["task4"], 2, 7))
        self.assertNotEqual(cases, compare.cases(["codex", "claude"], list(compare.ARMS), ["task4"], 2, 8))

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

    def test_headroom_prefix_is_explicit_and_keeps_native_argv(self):
        case = {"agent": "claude", "arm": "headroom"}
        with self.assertRaises(ValueError):
            compare.agent_command(case, {"headroom": Path("/headroom")}, [])
        actual = compare.agent_command(case, {"headroom": Path("/headroom")}, ["{headroom}", "custom"])
        self.assertEqual(actual, ["/headroom", "custom"] + base.command_for("claude"))

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

    def test_dry_run_cannot_start_processes_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(compare.subprocess, "Popen", side_effect=AssertionError("no model calls")):
            self.assertEqual(compare.main(["--dry-run", "--out", tmp]), 0)
            report = json.loads((Path(tmp) / "report.json").read_text())
            self.assertEqual(report["meta"]["planned_runs"], 18)
            self.assertFalse(report["runs"])
            with self.assertRaises(SystemExit):
                compare.main(["--dry-run", "--out", tmp])

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
                                     argparse.Namespace(prefix=[], timeout=1), {}, {}, Path(tmp))
        self.assertTrue(result["acceptance"]["passed"])
        self.assertEqual(result["usage"]["logical_input_tokens"], 31)
        self.assertEqual(result["usage"]["output_tokens"], 9)
        self.assertIn("artifact_error", result)


if __name__ == "__main__":
    unittest.main()
