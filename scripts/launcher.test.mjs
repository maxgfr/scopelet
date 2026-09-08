import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const launcher = resolve('skills/scopelet/scripts/scopelet.mjs');

test('explicit compatible offline binary preserves arguments and exit status', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-launcher-'));
  try {
    const binary = join(dir, 'scopelet');
    writeFileSync(binary, '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "scopelet 0.1.0"; else printf "%s\\n" "$@"; exit 7; fi\n', { mode: 0o700 });
    const result = spawnSync(process.execPath, [launcher, 'one argument', '$(not-executed)'], { encoding: 'utf8', env: { ...process.env, SCOPELET_BIN: binary } });
    assert.equal(result.status, 7);
    assert.equal(result.stdout, 'one argument\n$(not-executed)\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('explicit wrong-version binary fails without fetching a replacement', () => {
  const result = spawnSync(process.execPath, [launcher, 'doctor'], { encoding: 'utf8', env: { ...process.env, SCOPELET_BIN: '/usr/bin/true' } });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /SCOPELET_BIN must point/);
});

test('verified cached release runs offline', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-cached-'));
  try {
    const cache = join(dir, 'scopelet', 'bin', '0.1.0');
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "cached release"\n';
    writeFileSync(join(cache, 'scopelet'), content, { mode: 0o700 });
    writeFileSync(join(cache, 'scopelet.sha256'), createHash('sha256').update(content).digest('hex'));
    const env = { ...process.env, XDG_CACHE_HOME: dir, PATH: dir };
    delete env.SCOPELET_BIN;
    const result = spawnSync(process.execPath, [launcher, 'doctor'], { encoding: 'utf8', env, timeout: 5000 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'cached release\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
