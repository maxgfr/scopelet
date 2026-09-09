"""Proxy measurement and isolation contracts without provider calls."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import headroom_proxy as proxy


class ProxyTests(unittest.TestCase):
    def invoke(self, requests, exit_code=0, config=True):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = ['headroom_proxy.py', '--headroom', '/bin/headroom', 'claude', '-p', '--allowedTools', 'Bash']
            if config:
                args += ['--mcp-config', '{"mcpServers":{}}']
            daemon = Mock()
            daemon.poll.return_value = None
            child = Mock()
            child.wait.return_value = exit_code
            child.poll.return_value = exit_code
            stats = {'requests': {'total': requests}, 'tokens': {'saved': 3}}
            proxy.children.clear()
            with patch.object(proxy.sys, 'argv', args), \
                    patch.object(proxy.Path, 'cwd', return_value=root), \
                    patch.object(proxy.signal, 'signal'), \
                    patch.object(proxy, 'get_json', side_effect=[{}, {'requests': {'total': 0}}, stats, stats]), \
                    patch.object(proxy.subprocess, 'Popen', side_effect=[daemon, child]) as popen:
                code = proxy.main()
            result = json.loads((root / '.headroom-comparison/summary.json').read_text())
            calls = popen.call_args_list
            daemon.terminate.assert_called_once()
            for call in calls:
                self.assertFalse(call.kwargs.get('start_new_session', False))
            agent_argv = calls[1].args[0]
            server = json.loads(agent_argv[agent_argv.index('--mcp-config') + 1])['mcpServers']['headroom']
            self.assertEqual(server['command'], '/bin/headroom')
            self.assertIn('mcp__headroom__*', agent_argv[agent_argv.index('--allowedTools') + 1])
            self.assertTrue(calls[1].kwargs['env']['ANTHROPIC_BASE_URL'].startswith('http://127.0.0.1:'))
            return code, result

    def test_verified_routing_and_recovery_are_scoped_to_child(self):
        code, meta = self.invoke(2, config=False)
        self.assertEqual(code, 0)
        self.assertTrue(meta['routing_verified'])

    def test_agent_success_without_proxy_requests_is_not_a_measurement(self):
        code, meta = self.invoke(0)
        self.assertEqual(code, 125)
        self.assertFalse(meta['routing_verified'])

    def test_agent_failure_survives_verified_routing(self):
        code, meta = self.invoke(2, exit_code=7)
        self.assertEqual(code, 7)
        self.assertEqual(meta['agent_exit_code'], 7)

    def test_unverified_codex_integration_is_rejected_before_launch(self):
        with patch.object(proxy.sys, 'argv', ['proxy.py', '--headroom', '/bin/headroom', 'codex', 'exec']), \
                patch.object(proxy.subprocess, 'Popen') as popen:
            with self.assertRaisesRegex(SystemExit, 'Codex is unmeasured'):
                proxy.main()
            popen.assert_not_called()

    def test_stats_exclude_request_contents(self):
        self.assertEqual(proxy.safe_stats({'messages': ['private'], 'requests': {'total': 3, 'body': 'private'}}),
                         {'requests': {'total': 3}})


if __name__ == '__main__':
    unittest.main()
