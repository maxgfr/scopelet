#!/usr/bin/env python3
"""Verify the skill launcher and reversible host installation without model calls."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def verify(skill, binary=None):
    skill = skill.resolve()
    launcher = skill / 'scripts/scopelet.mjs'
    with tempfile.TemporaryDirectory(prefix='scopelet-install-') as temporary:
        root = Path(temporary)
        env = os.environ.copy()
        for key in list(env):
            if key.startswith('SCOPELET_'):
                env.pop(key)
        env.update(SCOPELET_CONFIG_DIR=str(root / 'config'),
                   SCOPELET_CACHE_DIR=str(root / 'cache'),
                   CODEX_HOME=str(root / 'codex'),
                   CLAUDE_CONFIG_DIR=str(root / 'claude'),
                   XDG_CACHE_HOME=str(root / 'launcher-cache'))
        if binary:
            env['SCOPELET_BIN'] = str(binary.resolve())

        def call(*args):
            return subprocess.check_output(['node', str(launcher), *args],
                                           env=env, cwd=root, text=True, timeout=120)

        version = call('--version').strip()
        assert version.startswith('scopelet ')
        expected = version.removeprefix('scopelet ')
        assert f'version: "{expected}"' in (skill / 'SKILL.md').read_text()
        files = [root / 'codex/hooks.json', root / 'claude/settings.json']
        existing = {'hooks': {'SessionStart': [{'hooks': [{'type': 'command', 'command': 'echo preserved'}]}]},
                    'unrelated_setting': True}
        for path in files:
            path.parent.mkdir(parents=True)
            path.write_text(json.dumps(existing))
        call('install', '--agent', 'all')
        installed = [path.read_bytes() for path in files]
        call('install', '--agent', 'all')
        assert [path.read_bytes() for path in files] == installed, 'reinstall changed hooks'
        health = json.loads(call('doctor'))['integration']
        assert health['binary_installed'] and health['mode'] == 'default'
        assert len(health['hosts']) == 2 and all(h['hooks_configured'] for h in health['hosts'])
        for path in files:
            value = json.loads(path.read_text())
            assert value['unrelated_setting'] is True
            assert value['hooks']['SessionStart'][0] == existing['hooks']['SessionStart'][0]
        for mode in ('caveman', 'off', 'default'):
            call('mode', mode)
            event = json.dumps({'hook_event_name': 'SessionStart', 'session_id': 'installation-check'})
            output = subprocess.check_output(['node', str(launcher), 'hook', 'codex'],
                                             env=env, cwd=root, input=event, text=True, timeout=30)
            context = json.loads(output)['hookSpecificOutput']['additionalContext']
            assert {'caveman': 'caveman', 'off': 'off', 'default': 'Scopelet auto'}[mode] in context
        bench = json.loads(call('bench'))
        assert bench['passed']
        call('uninstall', '--agent', 'all')
        assert all(json.loads(path.read_text()) == existing for path in files)
        assert all(not h['hooks_configured'] for h in json.loads(call('doctor'))['integration']['hosts'])
        return {'version': version, 'passed': True,
                'launcher': 'explicit release binary' if binary else 'normal resolution with fresh cache',
                'skill_sha256': hashlib.sha256((skill / 'SKILL.md').read_bytes()).hexdigest(),
                'checks': ['launcher version', 'both host hooks', 'default enabled', 'idempotent reinstall',
                           'unrelated hooks preserved', 'caveman/off/default', 'offline bench', 'reversible uninstall']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--skill', type=Path, default=Path('skills/scopelet'))
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    result = verify(args.skill, args.binary)
    text = json.dumps(result, indent=2) + '\n'
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(text)
    print(text, end='')


if __name__ == '__main__':
    main()
