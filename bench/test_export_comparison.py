import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "comparison_export", Path(__file__).resolve().parents[1] / "scripts/export_comparison.py")
exporter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exporter)


class ExportTests(unittest.TestCase):
    @staticmethod
    def view(mode="default"):
        return {"schema_version": 1, "artifact": "artifact:" + "a" * 64,
                "mode": mode, "display_complete": True}

    @staticmethod
    def trace(*events):
        return "\n".join(json.dumps(event) for event in events).encode()

    def claude_events(self, command, content, tool="Bash", result_id="call-1"):
        return (
            {"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": tool, "id": "call-1",
                 "input": {"command": command}}]}},
            {"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": result_id, "content": content}]}},
        )

    def test_modes_come_from_returned_views_not_requested_mode(self):
        events = self.claude_events('"$SCOPELET_BIN" query --mode ultra --spec -',
                                    "Output:\n" + json.dumps(self.view()))
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(*events)), ["default"])
        self.assertEqual(exporter.observed_scopelet_modes(b'{"input":"--mode ultra"}'), [])

    def test_quoted_views_in_prose_arguments_and_unrelated_returns_do_not_count(self):
        quoted = json.dumps(self.view("ultra"))
        prose = {"type": "assistant", "message": {"content": [
            {"type": "text", "text": quoted}]}}
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(prose)), [])
        # An actual Scopelet call is not evidence of a returned mode by itself.
        call, _ = self.claude_events('scopelet query --spec - <<EOF\n' + quoted + '\nEOF', "")
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(call)), [])
        for command, tool, result_id in (
                ('echo ' + quoted, "Bash", "call-1"),
                ('scopelet query --file example', "Read", "call-1"),
                ('scopelet query --file example', "Bash", "different-call")):
            with self.subTest(command=command, tool=tool, result_id=result_id):
                events = self.claude_events(command, quoted, tool, result_id)
                self.assertEqual(exporter.observed_scopelet_modes(self.trace(*events)), [])

    def test_codex_requires_completed_scopelet_command_output(self):
        item = {"type": "command_execution", "id": "item-1",
                "command": '/bin/zsh -lc \'"$SCOPELET_BIN" run --mode ultra -- python3 checks.py\'',
                "aggregated_output": json.dumps({"result": self.view("ultra")}),
                "exit_code": 1, "status": "failed"}
        completed = {"type": "item.completed", "item": item}
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(completed)), ["ultra"])
        started = {"type": "item.started", "item": item}
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(started)), [])
        unrelated = {"type": "item.completed", "item": {**item, "command": "cat example.json"}}
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(unrelated)), [])

    def test_expansion_mode_is_retained_without_scanning_quoted_record_text(self):
        view = {**self.view("ultra"), "records": [{"text": json.dumps(self.view())}]}
        first = self.claude_events('SCOPELET_CACHE_DIR=/tmp/test scopelet query --file example',
                                   [{"type": "text", "text": json.dumps(view)}])
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(*first)), ["ultra"])
        second = self.claude_events('node /skill/scripts/scopelet.mjs expand artifact:' + "a" * 64,
                                    json.dumps(self.view()))
        self.assertEqual(exporter.observed_scopelet_modes(self.trace(*first, *second)), ["default", "ultra"])

    def test_private_commands_paths_and_raw_usage_are_excluded(self):
        report = {"meta": {"executables": {"tool": {"path": "/private/credential", "sha256": "abc"}},
                           "disabled_codex_skills": ["/private/credential"]},
                  "runs": [{"agent": "claude", "task": "task3", "arm": "native", "repetition": 1,
                            "command": ["credential"], "acceptance": {"passed": True, "stdout": "credential"},
                            "usage": {"input_tokens": 31, "output_tokens": 9,
                                      "raw_usage": [{"credential": "credential"}]},
                            "harness_error": "credential"}]}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "report.json").write_text(json.dumps(report))
            result = exporter.export(root)
        self.assertNotIn("credential", json.dumps(result))
        self.assertEqual(result["runs"][0]["usage"]["input_tokens"], 31)
        self.assertIsNone(result["runs"][0]["usage"]["logical_input_tokens"])
        self.assertTrue(result["runs"][0]["harness_error"])
        self.assertIsNone(result["runs"][0]["stdout_sha256"])
        self.assertEqual(result["meta"]["models"]["claude"], "claude-haiku-4-5-20251001")

    def test_provider_fields_plugins_and_model_label_are_exported_without_paths(self):
        report = {"meta": {"model_requested": "claude-fable-5-1", "effort_requested": "high", "rtk_integration": "hook",
                           "claude_bin": "/Users/someone/bin/claude", "claude_bin_sha256": "c" * 64,
                           "executables": {"rtk": {"path": "/Users/someone/rtk", "sha256": "r" * 64}},
                           "executable_versions": {"rtk": {"version": "rtk 0.42.4", "stderr": "/Users/someone"}},
                           "plugins": {"caveman": {"path": "/Users/someone/caveman", "plugin_json_sha256": "p" * 64,
                                                   "hooks_sha256": {"src/hooks/caveman-activate.js": "h" * 64}, "skill_hashes": {"SKILL.md": "s" * 64}, "git_head": "15581d1"}},
                           "environment_purged": ["CLAUDE_CODE_EFFORT_LEVEL"]},
                  "runs": [{"agent": "claude", "task": "task4", "arm": "rtk", "repetition": 2, "command": ["/Users/someone/claude"],
                            "integration": "Documented PreToolUse Bash hook [hook]", "model_requested": "claude-fable-5-1", "effort_requested": "high",
                            "cache_markers_present": None, "acceptance": {"passed": True},
                            "usage": {"input_tokens": 3, "output_tokens": 4, "thinking_tokens": 2, "model_init": "claude-fable-5-1",
                                      "models_observed": ["claude-fable-5-1"], "model_usage": {"claude-fable-5-1": {"costUSD": 0.5}},
                                      "model_mismatch": False, "num_turns": 7, "total_cost_usd_reported": 0.5, "permission_denials": [],
                                      "raw_usage": [{"private": "/Users/someone"}]},
                            "tools": {"rtk_audit": {"rewrites": 2}, "hook_events": {"responses": {"PreToolUse:Bash": 3}}}}]}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "report.json").write_text(json.dumps(report))
            result = exporter.export(root)
        self.assertEqual(exporter.private_paths(result), [])
        self.assertEqual(result["meta"]["models"]["claude"], "claude-fable-5-1")
        self.assertEqual(result["meta"]["effort_requested"], "high")
        self.assertEqual(result["meta"]["plugins"]["caveman"]["git_head"], "15581d1")
        self.assertEqual(result["meta"]["executable_versions"]["rtk"], "rtk 0.42.4")
        row = result["runs"][0]
        self.assertEqual(row["integration"], "Documented PreToolUse Bash hook [hook]")
        self.assertEqual(row["usage"]["model_usage"]["claude-fable-5-1"]["costUSD"], 0.5)
        self.assertEqual(row["usage"]["num_turns"], 7)
        self.assertEqual(row["tools"]["rtk_audit"]["rewrites"], 2)
        leaked = exporter.private_paths({"x": "/Users/someone/a", "y": "C:\\Users\\x", "z": "/private/tmp/venv"})
        self.assertEqual(len(leaked), 3)
        self.assertIn("/Users/", leaked)
        self.assertIn("/private/", leaked)
