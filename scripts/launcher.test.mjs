import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, readdirSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const launcher = resolve('skills/scopelet/scripts/scopelet.mjs');
const version = readFileSync('Cargo.toml', 'utf8').match(/^version = "([^"]+)"$/m)[1];

test('explicit compatible offline binary preserves arguments and exit status', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-launcher-'));
  try {
    const binary = join(dir, 'scopelet');
    writeFileSync(binary, `#!/bin/sh\nif [ "$1" = "--version" ]; then echo "scopelet ${version}"; else printf "%s\\n" "$@"; exit 7; fi\n`, { mode: 0o700 });
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
    const cache = join(dir, 'scopelet', 'bin', version);
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "cached release"\n';
    writeFileSync(join(cache, 'scopelet'), content, { mode: 0o700 });
    writeFileSync(join(cache, 'scopelet.sha256'), createHash('sha256').update(content).digest('hex'));
    const env = { ...process.env, XDG_CACHE_HOME: dir, PATH: dir };
    delete env.SCOPELET_BIN;
    const result = spawnSync(process.execPath, ['--import', 'data:text/javascript,globalThis.fetch=()=>{throw new Error("Unexpected network")}', launcher, 'doctor'], { encoding: 'utf8', env, timeout: 5000 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'cached release\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});


test('interrupted install repairs its checksum sidecar without downloading the binary', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-repair-'));
  try {
    const cache = join(dir, 'scopelet', 'bin', version);
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "recovered release"\n';
    const expected = createHash('sha256').update(content).digest('hex');
    writeFileSync(join(cache, 'scopelet'), content, { mode: 0o700 });
    const targets = { 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin',
      'linux-x64': 'x86_64-unknown-linux-gnu', 'linux-arm64': 'aarch64-unknown-linux-gnu' };
    const target = targets[`${process.platform}-${process.arch}`];
    assert.ok(target, 'fixture requires a supported release platform');
    const prelude = join(dir, 'manifest-only.mjs');
    const calls = join(dir, 'fetches');
    writeFileSync(prelude, `
      import { appendFileSync } from 'node:fs';
      globalThis.fetch = async url => {
        appendFileSync(${JSON.stringify(calls)}, String(url) + '\\n');
        if (String(url) !== 'https://github.com/maxgfr/scopelet/releases/download/v${version}/SHA256SUMS') {
          throw new Error('Binary download must not happen');
        }
        return new Response(${JSON.stringify(expected + '  scopelet-' + target + '\n')});
      };
    `);
    const offline = join(dir, 'offline.mjs');
    writeFileSync(offline, 'globalThis.fetch = async () => { throw new Error("Offline: fetch forbidden"); };');
    const env = { ...process.env, XDG_CACHE_HOME: dir, PATH: dir };
    delete env.SCOPELET_BIN;
    const repaired = spawnSync(process.execPath, ['--import', prelude, launcher, 'doctor'], {
      encoding: 'utf8', env, timeout: 5000,
    });
    assert.equal(repaired.status, 0, repaired.stderr);
    assert.equal(repaired.stdout, 'recovered release\n');
    assert.equal(readFileSync(join(cache, 'scopelet.sha256'), 'utf8'), expected);
    assert.equal(readFileSync(calls, 'utf8'), `https://github.com/maxgfr/scopelet/releases/download/v${version}/SHA256SUMS\n`);
    assert.deepEqual(readdirSync(cache).sort(), ['scopelet', 'scopelet.sha256']);
    const cached = spawnSync(process.execPath, ['--import', offline, launcher, 'doctor'], {
      encoding: 'utf8', env, timeout: 5000,
    });
    assert.equal(cached.status, 0, cached.stderr);
    assert.equal(cached.stdout, 'recovered release\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
