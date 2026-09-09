#!/usr/bin/env python3
"""Validate all four versioned build artifacts before publication."""
import hashlib
from pathlib import Path
import sys
TARGETS=('aarch64-apple-darwin','x86_64-apple-darwin','aarch64-unknown-linux-gnu','x86_64-unknown-linux-gnu')
def checksums(root,version):
    files=[root/f'scopelet-{target}' for target in TARGETS]
    for file in files:
        if not file.is_file() or file.stat().st_size==0:raise ValueError(f'missing binary: {file.name}')
        marker=file.with_suffix('.version')
        if marker.read_text().strip()!=f'scopelet {version}':raise ValueError(f'wrong version: {file.name}')
    (root/'SHA256SUMS').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n' for p in files))
    for file in files:file.with_suffix('.version').unlink()
if __name__=='__main__':checksums(Path('dist'),sys.argv[1])
