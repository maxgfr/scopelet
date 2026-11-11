#!/usr/bin/env python3
"""Packaging gate; no model calls or third-party Python packages."""
from pathlib import Path
import re
import sys
import zipfile

root = Path(__file__).resolve().parents[1]
skill = root / 'skills' / 'scopelet'
text = (skill / 'SKILL.md').read_text()
assert text.startswith('---\n')
front = text.split('---', 2)[1]
assert re.search(r'^name: scopelet$', front, re.M)
description = re.search(r'^description: (.+)$', front, re.M).group(1)
assert 0 < len(description) <= 1024
assert len(text.encode()) <= 1536, 'keep the entrypoint under 1.5 KiB'
version = re.search(r'^version = "([^"]+)"$', (root / 'Cargo.toml').read_text(), re.M).group(1)
assert f'version: "{version}"' in front
launcher = (skill / 'scripts/scopelet.mjs').read_text()
assert f"const version = '{version}'" in launcher
# The launcher pins the checksum of every release binary of its version, or
# none between a version bump and its release.
pinned = re.search(r'// pinned-checksums:start\n(.*?)// pinned-checksums:end', launcher, re.S)
assert pinned, 'the launcher must carry its pinned-checksums block'
pins = re.findall(r"^  '([a-z0-9_-]+)': '[a-f0-9]{64}',$", pinned[1], re.M)
assert pinned[1] == 'const pinned = {};\n' or (
    sorted(pins) == ['aarch64-apple-darwin', 'aarch64-unknown-linux-gnu', 'x86_64-apple-darwin', 'x86_64-unknown-linux-gnu']
    and pinned[1].count('\n') == 6), 'the launcher pins all four release targets or none'
# Automatic by design: every host may pick the skill on its own. Manual is the
# user's opt-out, documented in the README, never the shipped default.
assert not re.search(r'^disable-model-invocation:\s*true\s*$', front, re.M), 'Claude Code and OpenCode must be allowed to load the skill'
for link in re.findall(r'\]\(([^)]+)\)', text):
    if not link.startswith('https://'):
        assert (skill / link).is_file(), link
assert (skill / 'LICENSE').read_bytes() == (root / 'LICENSE').read_bytes()
files = sorted(p for p in skill.rglob('*') if p.is_file())
assert len(files) >= 5, 'bundle must include references and launcher'
assert all(not p.is_symlink() for p in skill.rglob('*')), 'bundle must be standalone'
assert not any(p.name == '.DS_Store' or '__pycache__' in p.parts for p in files)
if '--pack' in sys.argv:
    dist = root / 'dist'
    dist.mkdir(exist_ok=True)
    # The same bundle always packs to the same bytes: fixed timestamps and
    # modes, in sorted order, so the archive's checksum means something.
    with zipfile.ZipFile(dist / 'scopelet.skill', 'w', zipfile.ZIP_DEFLATED) as z:
        for path in files:
            entry = zipfile.ZipInfo(path.relative_to(skill.parent).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o100644 << 16
            z.writestr(entry, path.read_bytes())
    with zipfile.ZipFile(dist / 'scopelet.skill') as z:
        assert len(z.namelist()) == len(files)
        assert all('\\' not in name for name in z.namelist())
print(f'Skill valid: {len(files)} files, {len(text.encode())} entrypoint bytes, version {version}')
