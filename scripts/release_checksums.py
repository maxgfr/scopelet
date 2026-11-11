#!/usr/bin/env python3
"""Validate all four versioned build artifacts before publication.

`release_checksums.py VERSION` checks the binaries in `dist/`, writes
`dist/SHA256SUMS` and pins the same hashes in the launcher, so the skill that
is packed next carries them: a binary replaced on the release page afterwards
cannot match. `release_checksums.py --skill`, run once the skill is packed,
adds its line to `SHA256SUMS`.
"""
import hashlib
from pathlib import Path
import re
import sys
TARGETS=('aarch64-apple-darwin','x86_64-apple-darwin','aarch64-unknown-linux-gnu','x86_64-unknown-linux-gnu')
LAUNCHER=Path('skills/scopelet/scripts/scopelet.mjs')
PINS=re.compile(r'(// pinned-checksums:start\n).*?(// pinned-checksums:end)',re.S)
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def pin(launcher,version,sums):
    """Write `sums` (target to hash) into the launcher's pinned block."""
    text=launcher.read_text()
    if f"const version = '{version}'" not in text:raise ValueError(f'{launcher} is not at version {version}')
    body='const pinned = {\n'+''.join(f"  '{target}': '{digest}',\n" for target,digest in sums.items())+'};\n'
    pinned,count=PINS.subn(lambda block:block[1]+body+block[2],text)
    if count!=1:raise ValueError(f'{launcher} has no pinned-checksums block')
    launcher.write_text(pinned)
def checksums(root,version,launcher=None):
    files=[root/f'scopelet-{target}' for target in TARGETS]
    for file in files:
        if not file.is_file() or file.stat().st_size==0:raise ValueError(f'missing binary: {file.name}')
        marker=file.with_suffix('.version')
        if marker.read_text().strip()!=f'scopelet {version}':raise ValueError(f'wrong version: {file.name}')
    sums={target:sha(file) for target,file in zip(TARGETS,files)}
    # Before anything is written: a launcher that cannot be pinned fails the release whole.
    if launcher is not None:pin(launcher,version,sums)
    (root/'SHA256SUMS').write_text(''.join(f'{digest}  scopelet-{target}\n' for target,digest in sums.items()))
    for file in files:file.with_suffix('.version').unlink()
def skill(root):
    """Add the packed skill to SHA256SUMS, after the four binaries."""
    manifest=root/'SHA256SUMS';archive=root/'scopelet.skill'
    lines=[line for line in manifest.read_text().splitlines() if line.split()[1:]!=[archive.name]]
    if len(lines)!=len(TARGETS):raise ValueError('SHA256SUMS must list the four binaries first')
    manifest.write_text(''.join(line+'\n' for line in lines)+f'{sha(archive)}  {archive.name}\n')
if __name__=='__main__':
    if sys.argv[1]=='--skill':skill(Path('dist'))
    else:checksums(Path('dist'),sys.argv[1],LAUNCHER)
