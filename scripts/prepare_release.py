#!/usr/bin/env python3
"""Synchronize release versions. Does not change dependency versions or publish."""
import re
from pathlib import Path
import sys

def prepare(root, version):
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('expected stable semantic version')
    cargo=root/'Cargo.toml';old=re.search(r'(?ms)^\[package\]\n(?:(?!^\[).)*?^version = "([^"]+)"',cargo.read_text())[1]
    replacements={
        cargo:(f'version = "{old}"',f'version = "{version}"'),
        root/'Cargo.lock':(f'name = "scopelet"\nversion = "{old}"',f'name = "scopelet"\nversion = "{version}"'),
        root/'skills/scopelet/SKILL.md':(f'version: "{old}"',f'version: "{version}"'),
        root/'skills/scopelet/scripts/scopelet.mjs':(f"const version = '{old}'",f"const version = '{version}'"),
    }
    updates={}
    for path,(before,after) in replacements.items():
        text=path.read_text()
        if text.count(before)!=1:raise ValueError(f'ambiguous version in {path}')
        updates[path]=text.replace(before,after,1)
    # The launcher pins the hashes of one version's binaries: a new version
    # starts without any, until `release_checksums.py` writes its own.
    launcher=root/'skills/scopelet/scripts/scopelet.mjs'
    if old!=version:
        updates[launcher],count=re.subn(r'(?s)(// pinned-checksums:start\n).*?(// pinned-checksums:end)',
                                        r'\1const pinned = {};\n\2',updates[launcher])
        if count!=1:raise ValueError(f'{launcher} has no pinned-checksums block')
    # The documented versions are prose, so their wording can drift: what is
    # checked is that each file names the new version afterwards. The README
    # may already carry it (the publish job takes the one the build bumped).
    documented={
        'README.md':(f'--tag v{version} ',),
        'skills/scopelet/references/setup.md':(f'--tag v{version} ',f'installs Scopelet **{version}**'),
    }
    for name,expected in documented.items():
        path=root/name;text=path.read_text().replace(f'--tag v{old} ',f'--tag v{version} ')
        text=text.replace(f'installs Scopelet **{old}**',f'installs Scopelet **{version}**')
        for wanted in expected:
            if wanted not in text:raise ValueError(f'{name} does not document {wanted.strip()!r}; it would keep an old version')
        updates[path]=text
    for path,text in updates.items():path.write_text(text)

if __name__=='__main__':prepare(Path(__file__).resolve().parents[1],sys.argv[1])
