import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, readdirSync, mkdirSync, rmSync, existsSync, utimesSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const launcher = resolve('skills/scopelet/scripts/scopelet.mjs');
const version = readFileSync('Cargo.toml', 'utf8').match(/^version = "([^"]+)"$/m)[1];
const targets = { 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu', 'linux-arm64': 'aarch64-unknown-linux-gnu' };
const target = targets[`${process.platform}-${process.arch}`];
const release = `https://github.com/maxgfr/scopelet/releases/download/v${version}`;
const sha256 = content => createHash('sha256').update(content).digest('hex');

// The launcher under test with the checksums its release pinned replaced by
// `pins` (none by default). The repository's copy pins the hashes of its own
// published binaries, which no fixture has.
function launcherWith(dir, pins = {}) {
  const source = readFileSync(launcher, 'utf8');
  const block = /\/\/ pinned-checksums:start\n[\s\S]*?\/\/ pinned-checksums:end/;
  assert.match(source, block);
  const body = Object.entries(pins).map(([name, hash]) => `  '${name}': '${hash}',\n`).join('');
  const file = join(dir, 'scopelet.mjs');
  writeFileSync(file, source.replace(block, `// pinned-checksums:start\nconst pinned = {\n${body}};\n// pinned-checksums:end`));
  return file;
}

// A fetch that serves `assets` (by the last part of the URL), records every
// address asked in `calls`, and fails for anything else.
function serving(dir, assets) {
  const prelude = join(dir, 'release.mjs');
  const calls = join(dir, 'fetches');
  writeFileSync(calls, '');
  writeFileSync(prelude, `
    import { appendFileSync } from 'node:fs';
    const assets = ${JSON.stringify(assets)};
    globalThis.fetch = async url => {
      appendFileSync(${JSON.stringify(calls)}, String(url) + '\\n');
      const name = String(url).split('/').pop();
      if (!(name in assets)) throw new Error('Unexpected download: ' + url);
      return new Response(assets[name]);
    };
  `);
  return { prelude, asked: () => readFileSync(calls, 'utf8').split('\n').filter(Boolean) };
}

function offline(dir) {
  const prelude = join(dir, 'offline.mjs');
  writeFileSync(prelude, 'globalThis.fetch = async () => { throw new Error("Offline: fetch forbidden"); };');
  return prelude;
}

function run(script, dir, prelude, extra = {}) {
  const env = { ...process.env, XDG_CACHE_HOME: dir, PATH: dir, ...extra };
  delete env.SCOPELET_BIN;
  return spawnSync(process.execPath, [...(prelude ? ['--import', prelude] : []), script, 'doctor'], { encoding: 'utf8', env, timeout: 5000 });
}

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

test('explicit binary errors say what was found', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-launcher-'));
  try {
    const run = binary => spawnSync(process.execPath, [launcher, 'doctor'], { encoding: 'utf8', env: { ...process.env, SCOPELET_BIN: binary } });
    const missing = run(join(dir, 'absent'));
    assert.equal(missing.status, 2);
    assert.match(missing.stderr, /SCOPELET_BIN must point to scopelet .*ENOENT/);
    const other = join(dir, 'scopelet');
    writeFileSync(other, '#!/bin/sh\necho "scopelet 0.0.1"\n', { mode: 0o700 });
    const wrong = run(other);
    assert.equal(wrong.status, 2);
    assert.match(wrong.stderr, /SCOPELET_BIN must point to scopelet .*reports "scopelet 0\.0\.1"/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('the repository launcher pins a checksum for every release target, or none', () => {
  const block = readFileSync(launcher, 'utf8').match(/\/\/ pinned-checksums:start\n([\s\S]*?)\/\/ pinned-checksums:end/)[1];
  const pins = [...block.matchAll(/^  '([^']+)': '([a-f0-9]{64})',$/gm)].map(match => match[1]);
  assert.ok(pins.length === 0 || pins.sort().join() === Object.values(targets).sort().join(), block);
});

test('a pinned release installs without asking the release for its checksums', () => {
  assert.ok(target, 'fixture requires a supported release platform');
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-pinned-'));
  try {
    const content = '#!/bin/sh\necho "pinned release"\n';
    const script = launcherWith(dir, { [target]: sha256(content) });
    const served = serving(dir, { [`scopelet-${target}`]: content });
    const installed = run(script, dir, served.prelude);
    assert.equal(installed.status, 0, installed.stderr);
    assert.equal(installed.stdout, 'pinned release\n');
    assert.deepEqual(served.asked(), [`${release}/scopelet-${target}`]);
    // Afterwards it runs from the cache, checked against the pin, offline.
    const cached = run(script, dir, offline(dir));
    assert.equal(cached.status, 0, cached.stderr);
    assert.equal(cached.stdout, 'pinned release\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a binary that does not match its pinned checksum is refused, whatever the release says', () => {
  assert.ok(target, 'fixture requires a supported release platform');
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-tampered-'));
  try {
    const genuine = '#!/bin/sh\necho "genuine release"\n';
    const tampered = '#!/bin/sh\necho "replaced asset"\n';
    const script = launcherWith(dir, { [target]: sha256(genuine) });
    const cache = join(dir, 'scopelet', 'bin', version);
    // The release page serves a replaced binary and checksums that agree with it.
    const replaced = serving(dir, { [`scopelet-${target}`]: tampered, SHA256SUMS: `${sha256(tampered)}  scopelet-${target}\n` });
    const refused = run(script, dir, replaced.prelude);
    assert.equal(refused.status, 2);
    assert.match(refused.stderr, /Release checksum mismatch/);
    assert.equal(refused.stdout, '');
    assert.equal(existsSync(join(cache, 'scopelet')), false);
    // A replaced binary already in the cache, with a sidecar that agrees
    // with it, is not run either: it is downloaded again and checked.
    writeFileSync(join(cache, 'scopelet'), tampered, { mode: 0o700 });
    writeFileSync(join(cache, 'scopelet.sha256'), sha256(tampered));
    const repaired = run(script, dir, serving(dir, { [`scopelet-${target}`]: genuine }).prelude);
    assert.equal(repaired.status, 0, repaired.stderr);
    assert.equal(repaired.stdout, 'genuine release\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a failed download names the address and the cause', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-offline-'));
  try {
    // Node's fetch says only "fetch failed" and keeps the reason in `cause`.
    const prelude = join(dir, 'unreachable.mjs');
    writeFileSync(prelude, `globalThis.fetch = async () => {
      throw Object.assign(new TypeError('fetch failed'),
        { cause: Object.assign(new Error('getaddrinfo ENOTFOUND github.com'), { code: 'ENOTFOUND' }) });
    };`);
    const result = run(launcherWith(dir), dir, prelude);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /^scopelet setup: /);
    assert.match(result.stderr, /getaddrinfo ENOTFOUND github\.com/);
    assert.ok(result.stderr.includes(`${release}/SHA256SUMS`), result.stderr);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a download redirected off HTTPS is refused', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-redirect-'));
  try {
    const prelude = join(dir, 'redirect.mjs');
    writeFileSync(prelude, `globalThis.fetch = async () => {
      const response = new Response('anything');
      Object.defineProperty(response, 'url', { value: 'http://mirror.example/scopelet' });
      return response;
    };`);
    const result = run(launcherWith(dir), dir, prelude);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /redirected off HTTPS to http:\/\/mirror\.example\/scopelet/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a temporary left by a killed install does not block the next one, and old ones are swept', () => {
  assert.ok(target, 'fixture requires a supported release platform');
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-stale-'));
  try {
    const cache = join(dir, 'scopelet', 'bin', version);
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "fresh release"\n';
    const expected = sha256(content);
    // Process ids repeat, in a container often at once: the killed run and
    // this one are given the same id.
    writeFileSync(join(cache, 'scopelet.4242.tmp'), 'partial download');
    writeFileSync(join(cache, 'scopelet.4242.tmp.sha256'), 'partial');
    // One from hours ago is nobody's any more; a file that is not a
    // temporary of the launcher is not touched whatever its age.
    const old = new Date(Date.now() - 7_200_000);
    for (const name of ['scopelet.old.tmp', 'scopelet.old.tmp.sha256', 'notes.txt']) {
      writeFileSync(join(cache, name), 'left behind');
      utimesSync(join(cache, name), old, old);
    }
    const served = serving(dir, { SHA256SUMS: `${expected}  scopelet-${target}\n`, [`scopelet-${target}`]: content });
    const prelude = join(dir, 'same-pid.mjs');
    writeFileSync(prelude, `Object.defineProperty(process, 'pid', { value: 4242 });\nawait import(${JSON.stringify(served.prelude)});`);
    const result = run(launcherWith(dir), dir, prelude);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'fresh release\n');
    assert.equal(readFileSync(join(cache, 'scopelet.sha256'), 'utf8'), expected);
    assert.deepEqual(readdirSync(cache).sort(),
      ['notes.txt', 'scopelet', 'scopelet.4242.tmp', 'scopelet.4242.tmp.sha256', 'scopelet.sha256']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a scopelet in a relative PATH entry is never run', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-relative-'));
  try {
    const project = join(dir, 'project');
    mkdirSync(project);
    writeFileSync(join(project, 'scopelet'), `#!/bin/sh\nif [ "$1" = "--version" ]; then echo "scopelet ${version}"; else echo "PROJECT FILE RAN"; fi\n`, { mode: 0o700 });
    const cache = join(dir, 'scopelet', 'bin', version);
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "cached release"\n';
    writeFileSync(join(cache, 'scopelet'), content, { mode: 0o700 });
    writeFileSync(join(cache, 'scopelet.sha256'), sha256(content));
    const script = launcherWith(dir);
    for (const path of ['.', `.:${dir}`, `${dir}:`, `:${dir}`]) {
      const env = { ...process.env, XDG_CACHE_HOME: dir, PATH: path };
      delete env.SCOPELET_BIN;
      const result = spawnSync(process.execPath, ['--import', offline(dir), script, 'doctor'], { encoding: 'utf8', env, cwd: project, timeout: 5000 });
      assert.equal(result.status, 0, `${path}: ${result.stderr}`);
      assert.equal(result.stdout, 'cached release\n', path);
    }
    // An absolute entry is still used.
    const env = { ...process.env, XDG_CACHE_HOME: dir, PATH: project };
    delete env.SCOPELET_BIN;
    const found = spawnSync(process.execPath, [script, 'doctor'], { encoding: 'utf8', env, timeout: 5000 });
    assert.match(found.stdout, /PROJECT FILE RAN/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('verified cached release runs offline', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-cached-'));
  try {
    const cache = join(dir, 'scopelet', 'bin', version);
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "cached release"\n';
    writeFileSync(join(cache, 'scopelet'), content, { mode: 0o700 });
    writeFileSync(join(cache, 'scopelet.sha256'), sha256(content));
    const result = run(launcherWith(dir), dir, offline(dir));
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'cached release\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a relative XDG_CACHE_HOME is ignored for the home cache', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-xdg-'));
  try {
    const cache = join(dir, '.cache', 'scopelet', 'bin', version);
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "home cache"\n';
    writeFileSync(join(cache, 'scopelet'), content, { mode: 0o700 });
    writeFileSync(join(cache, 'scopelet.sha256'), sha256(content));
    const result = run(launcherWith(dir), dir, offline(dir), { XDG_CACHE_HOME: 'relative-cache', HOME: dir });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'home cache\n');
    assert.equal(existsSync('relative-cache'), false);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('musl Linux is told there is no release instead of being given a binary that cannot start', () => {
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-musl-'));
  try {
    const content = '#!/bin/sh\necho "glibc release"\n';
    const name = 'scopelet-x86_64-unknown-linux-gnu';
    const served = serving(dir, { SHA256SUMS: `${sha256(content)}  ${name}\n`, [name]: content });
    const prelude = join(dir, 'alpine.mjs');
    writeFileSync(prelude, `
      Object.defineProperty(process, 'platform', { value: 'linux' });
      Object.defineProperty(process, 'arch', { value: 'x64' });
      Object.defineProperty(process, 'report', { value: { getReport: () => ({ header: {} }) } });
      await import(${JSON.stringify(served.prelude)});`);
    const result = run(launcherWith(dir), dir, prelude);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /No release for musl libc/);
    assert.deepEqual(served.asked(), [`${release}/SHA256SUMS`]);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('interrupted install repairs its checksum sidecar without downloading the binary', () => {
  assert.ok(target, 'fixture requires a supported release platform');
  const dir = mkdtempSync(join(tmpdir(), 'scopelet-repair-'));
  try {
    const cache = join(dir, 'scopelet', 'bin', version);
    mkdirSync(cache, { recursive: true });
    const content = '#!/bin/sh\necho "recovered release"\n';
    const expected = sha256(content);
    writeFileSync(join(cache, 'scopelet'), content, { mode: 0o700 });
    const script = launcherWith(dir);
    const served = serving(dir, { SHA256SUMS: `${expected}  scopelet-${target}\n` });
    const repaired = run(script, dir, served.prelude);
    assert.equal(repaired.status, 0, repaired.stderr);
    assert.equal(repaired.stdout, 'recovered release\n');
    assert.equal(readFileSync(join(cache, 'scopelet.sha256'), 'utf8'), expected);
    assert.deepEqual(served.asked(), [`${release}/SHA256SUMS`]);
    assert.deepEqual(readdirSync(cache).sort(), ['scopelet', 'scopelet.sha256']);
    const cached = run(script, dir, offline(dir));
    assert.equal(cached.status, 0, cached.stderr);
    assert.equal(cached.stdout, 'recovered release\n');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
