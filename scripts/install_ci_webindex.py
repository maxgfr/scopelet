#!/usr/bin/env python3
"""Fetch pinned upstream webindex release assets for reproducible adapter CI."""
import hashlib
from pathlib import Path
import sys
import urllib.request

root = Path(sys.argv[1]).resolve()
root.mkdir(parents=True, exist_ok=True)
assets = {
    'webindex.mjs': 'baf2042f77c013f458ec4860f9666f9d48bc6946842083c9e450e32ead7b48f5',
    'engine.mjs': '2dde2ee3c386c8f7762423d655753528d061e7498b6e5fc1e419193581fb6310',
}
for name, expected in assets.items():
    with urllib.request.urlopen(f'https://github.com/maxgfr/webindex/releases/download/v1.19.4/{name}', timeout=60) as response:
        data = response.read(32 * 1024 * 1024 + 1)
    if hashlib.sha256(data).hexdigest() != expected:
        raise RuntimeError(f'{name}: checksum mismatch')
    (root / name).write_bytes(data)
(root / 'webindex.mjs').chmod(0o755)
(root / 'webindex').symlink_to('webindex.mjs')
print(root)
