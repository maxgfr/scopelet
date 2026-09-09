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
    for name in ('README.md','skills/scopelet/references/setup.md'):
        path=root/name;text=path.read_text().replace(f'--tag v{old}',f'--tag v{version}')
        text=text.replace(f'installs Scopelet **{old}**',f'installs Scopelet **{version}**')
        updates[path]=text
    for path,text in updates.items():path.write_text(text)

if __name__=='__main__':prepare(Path(__file__).resolve().parents[1],sys.argv[1])
