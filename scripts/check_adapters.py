#!/usr/bin/env python3
"""Live local integration with installed engines; synthetic public fixtures only."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/scopelet').resolve())
with tempfile.TemporaryDirectory(prefix='scopelet-adapters-') as temp:
    root = Path(temp)
    (root/'auth.ts').write_text('export function validateToken(expiry: number, now: number) {\n  return now < expiry;\n}\n')
    (root/'client.ts').write_text('import { validateToken } from "./auth";\nexport const ok = validateToken(20, 10);\n')
    (root/'page.html').write_text('<html><title>Fixture</title><main><h1>Limits</h1><p>Maximum requests per minute: 42.</p></main></html>')
    def query(source):
        result = subprocess.run([binary,'--cache-dir',str(root/'cache'),'query','--spec','-'],
            input=json.dumps({'version':1,'source':source}),text=True,capture_output=True,timeout=120)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)
    defs = query({'type':'code','path':str(root),'symbol':'validateToken'})
    assert defs['total_records'] == 1, defs
    assert 'return now < expiry;' in defs['records'][0]['text']
    assert defs['records'][0]['start_line'] == 1
    assert defs['scan_complete'] is False
    for relation, symbol in [('callers','validateToken'),('impact','auth.ts')]:
        result = query({'type':'code','path':str(root),'symbol':symbol,'relation':relation})
        assert result['records'], result
    page = query({'type':'document','path':str(root/'page.html')})
    assert '42' in page['records'][0]['text']
    assert page['scan_complete'] is False
    print('Adapters passed: codeindex definitions/callers/impact; webindex HTML extraction')
