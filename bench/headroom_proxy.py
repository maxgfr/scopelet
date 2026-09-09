#!/usr/bin/env python3
"""Isolated Claude Code benchmark proxy; rejects unobserved Headroom routing."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

PIN = 'e67b3c8a29443a60d6b0018fb22f525c5cd7e709'
children = []

def stop_children():
    for child in reversed(children):
        if child.poll() is None:
            child.terminate()
    deadline = time.monotonic() + .5
    for child in reversed(children):
        try:
            child.wait(timeout=max(.1, deadline-time.monotonic()))
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()

def interrupted(signum, frame):
    raise SystemExit(128 + signum)

def get_json(url):
    with urllib.request.urlopen(url, timeout=2) as response:
        return json.load(response)

def safe_stats(value):
    # Numeric aggregate evidence only. Never copy request bodies or identifiers.
    if isinstance(value, dict):
        result = {}
        for key in ('requests', 'tokens', 'compression', 'cache', 'latency'):
            item = value.get(key)
            if isinstance(item, dict):
                result[key] = {name: number for name, number in item.items()
                               if isinstance(number, (int, float, bool)) or number is None}
        return result
    return {}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--headroom', required=True, type=Path)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    options = parser.parse_args()
    headroom = str(options.headroom.resolve())
    args = options.command
    if not args:
        raise SystemExit('Pass the original claude command after this prefix.')
    agent = Path(args[0]).name
    if agent != 'claude':
        raise SystemExit('Only Claude Code routing is verified; Codex is unmeasured.')
    state = Path.cwd() / '.headroom-comparison'
    state.mkdir(mode=0o700, exist_ok=True)
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    url = 'http://127.0.0.1:' + str(port)
    env = os.environ.copy()
    env.update(HEADROOM_WORKSPACE_DIR=str(state / 'state'),
               HEADROOM_CONFIG_DIR=str(state / 'config'),
               HEADROOM_AGENT_TYPE=agent,
               HEADROOM_TELEMETRY='off',
               HEADROOM_LOG_MESSAGES='false')
    mode = os.environ.get('SCOPELET_HEADROOM_MODE', 'token')
    if mode not in ('token', 'cache'):
        raise SystemExit('SCOPELET_HEADROOM_MODE must be token or cache.')
    meta = {'upstream_commit': PIN, 'mode': mode, 'agent': agent,
            'integration': 'standalone proxy with native recovery MCP',
            'proxy_ready': False, 'agent_exit_code': None,
            'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, interrupted)
    proxy_log = (state / 'proxy-private.log').open('wb')
    try:
        proxy = subprocess.Popen([headroom, 'proxy', '--host', '127.0.0.1',
                                  '--port', str(port), '--mode', mode,
                                  '--no-telemetry', '--no-learn'], env=env,
                                 stdin=subprocess.DEVNULL, stdout=proxy_log,
                                 stderr=subprocess.STDOUT)
        children.append(proxy)
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if proxy.poll() is not None:
                raise RuntimeError('Headroom proxy exited before readiness; inspect private log.')
            try:
                health = get_json(url + '/readyz')
                meta['proxy_ready'] = True
                break
            except (OSError, ValueError):
                time.sleep(.2)
        if not meta['proxy_ready']:
            raise RuntimeError('Headroom proxy readiness timed out.')
        mcp_args = ['mcp', 'serve', '--proxy-url', url]
        env['ANTHROPIC_BASE_URL'] = url
        env.setdefault('ENABLE_TOOL_SEARCH', 'true')
        server = {'command': headroom, 'args': mcp_args,
                  'env': {key: env[key] for key in ('HEADROOM_WORKSPACE_DIR', 'HEADROOM_CONFIG_DIR', 'HEADROOM_TELEMETRY')}}
        config = {'mcpServers': {'headroom': server}}
        if '--mcp-config' in args:
            index = args.index('--mcp-config') + 1
            prior = json.loads(args[index])
            prior.setdefault('mcpServers', {})['headroom'] = server
            args[index] = json.dumps(prior)
        else:
            args += ['--strict-mcp-config', '--mcp-config', json.dumps(config)]
        if '--allowedTools' in args:
            index = args.index('--allowedTools') + 1
            args[index] += ',mcp__headroom__*'

        meta['stats_before'] = safe_stats(get_json(url + '/stats'))
        child = subprocess.Popen(args, env=env)
        children.append(child)
        meta['agent_exit_code'] = child.wait()
        meta['stats_after'] = safe_stats(get_json(url + '/stats'))
        before = meta['stats_before'].get('requests', {}).get('total', 0)
        after = meta['stats_after'].get('requests', {}).get('total', 0)
        meta['routing_verified'] = after > before
        if not meta['routing_verified']:
            print('Headroom received no requests; this is not a measured treatment.', file=sys.stderr)
        return meta['agent_exit_code'] or (0 if meta['routing_verified'] else 125)
    finally:
        if meta['proxy_ready']:
            try:
                meta['stats_after'] = safe_stats(get_json(url + '/stats'))
            except (OSError, ValueError):
                meta['stats_error'] = 'Proxy statistics unavailable at cleanup.'
        stop_children()
        proxy_log.close()
        (state / 'summary.json').write_text(json.dumps(meta, indent=2) + '\n')

if __name__ == '__main__':
    sys.exit(main())
