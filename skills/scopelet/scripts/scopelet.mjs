#!/usr/bin/env node
import { createHash, randomUUID } from 'node:crypto';
import { spawnSync, spawn } from 'node:child_process';
import { mkdir, readFile, readdir, stat, writeFile, rename, chmod, rm } from 'node:fs/promises';
import { homedir, constants } from 'node:os';
import { accessSync, statSync, constants as files } from 'node:fs';
import { delimiter, isAbsolute, join } from 'node:path';

const version = '1.2.11';
// The SHA-256 of this version's release binaries, written here when the
// release is cut (`scripts/release_checksums.py`). This file reaches the user
// through the skill, apart from the release: a binary replaced on the
// release page afterwards cannot match. Empty between a version bump and its
// release; the release's own SHA256SUMS is then asked instead.
// pinned-checksums:start
const pinned = {
  'aarch64-apple-darwin': '14da0a2d904462454013a2fd0e8350167b3e5da37106b3367b88c478c7576172',
  'x86_64-apple-darwin': '763964b39eb25554a233b131cdf5223227ceb102e8f79b598ea3fae7b56709ee',
  'aarch64-unknown-linux-gnu': 'b2526a1fcaa6c9f89e96ab487b11da742a1737b7d5e9cf81d5fc854b863d38a4',
  'x86_64-unknown-linux-gnu': '137315f19a941d94e5dbe6d7a2edd44c95981b332ec495941d5e522ae3a7ad3d',
};
// pinned-checksums:end
const targets = { 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu', 'linux-arm64': 'aarch64-unknown-linux-gnu' };
const target = targets[`${process.platform}-${process.arch}`];
// The binary is kept under the user's cache directory. A relative
// XDG_CACHE_HOME is invalid and ignored, as the specification says.
const cacheHome = isAbsolute(process.env.XDG_CACHE_HOME || '') ? process.env.XDG_CACHE_HOME : join(homedir(), '.cache');
const root = join(cacheHome, 'scopelet', 'bin', version);

async function bytes(url, limit) {
  let response;
  // Redirects are followed here, one at a time, so that none of them leaves
  // HTTPS on the way to the asset, not only the last.
  for (let hops = 0, at = url; ; hops++) {
    try {
      response = await fetch(at, { redirect: 'manual', signal: AbortSignal.timeout(120_000) });
    } catch (error) {
      // fetch says only "fetch failed": the reason (an unresolved host, a
      // refused connection, a proxy or TLS error) is in its cause.
      const reason = error.cause?.message || error.cause?.code || error.message;
      throw new Error(`Download failed: ${reason} (${url})`);
    }
    const location = response.headers.get('location');
    if (response.status < 300 || response.status > 399 || !location) break;
    if (hops === 10) throw new Error(`Download failed: too many redirects (${url})`);
    at = new URL(location, at).href;
    if (!at.startsWith('https://')) throw new Error(`Download refused: redirected off HTTPS to ${at}`);
  }
  if (!response.ok) throw new Error(`Download failed: HTTP ${response.status} (${url})`);
  const chunks = []; let length = 0;
  for await (const chunk of response.body) {
    length += chunk.length;
    if (length > limit) throw new Error('Release download exceeds size limit');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

// Temporaries are named at random: a process id repeats (at once in a
// container), and one left by a killed install would fail the exclusive
// create of every later run that drew the same id.
async function saveChecksum(path, expected) {
  const temporary = `${path}.${randomUUID()}.tmp.sha256`;
  try {
    await writeFile(temporary, expected, { flag: 'wx', mode: 0o600 });
    await rename(temporary, `${path}.sha256`);
  } finally { await rm(temporary, { force: true }); }
}

// Temporaries of installs that were killed, an hour or more ago.
async function sweep() {
  try {
    for (const name of await readdir(root)) {
      if (!/^scopelet\..+\.tmp(\.sha256)?$/.test(name)) continue;
      const file = join(root, name);
      if (Date.now() - (await stat(file)).mtimeMs > 3_600_000) await rm(file, { force: true });
    }
  } catch {}
}

const hash = buffer => createHash('sha256').update(buffer).digest('hex');

// The file's hash, or undefined when there is none. Any other error is said,
// so a failed download after it does not hide the local cause.
async function hashOf(path) {
  try {
    return hash(await readFile(path));
  } catch (error) {
    if (error.code !== 'ENOENT') process.stderr.write(`scopelet setup: cached release unreadable (${error.message})\n`);
  }
}

// The `scopelet` on the PATH, searched in its absolute entries only: a
// relative or empty entry is the working directory, a project whose files
// are never run.
function onPath() {
  for (const dir of (process.env.PATH || '').split(delimiter)) {
    if (!isAbsolute(dir)) continue;
    const file = join(dir, 'scopelet');
    try {
      accessSync(file, files.X_OK);
      if (statSync(file).isFile()) return file;
    } catch {}
  }
}

async function binary() {
  // Explicit override is useful for offline installs and independent tests.
  const override = process.env.SCOPELET_BIN;
  const candidate = override || onPath();
  const check = candidate ? spawnSync(candidate, ['--version'], { encoding: 'utf8', timeout: 5000 }) : {};
  if (check.status === 0 && check.stdout.trim() === `scopelet ${version}`) return candidate;
  if (override) {
    // A missing file, a hung binary and another version are different fixes.
    const found = check.error ? check.error.message
      : check.stdout.trim() ? `it reports "${check.stdout.trim().split('\n')[0]}"`
      : `--version exited with ${check.status ?? check.signal}`;
    throw new Error(`SCOPELET_BIN must point to scopelet ${version} (${override}: ${found})`);
  }
  if (!target) throw new Error('No release for this platform; install with Cargo and rerun.');
  await mkdir(root, { recursive: true, mode: 0o700 });
  const path = join(root, 'scopelet');
  const name = `scopelet-${target}`;
  const base = `https://github.com/maxgfr/scopelet/releases/download/v${version}`;
  let expected = pinned[target];
  if (expected) {
    // The cached binary is checked against the pinned hash itself.
    if (await hashOf(path) === expected) return path;
  } else {
    let cached;
    try { cached = (await readFile(`${path}.sha256`, 'utf8')).trim(); } catch {}
    if (/^[a-f0-9]{64}$/.test(cached) && await hashOf(path) === cached) return path;
    const manifest = (await bytes(`${base}/SHA256SUMS`, 16384)).toString('utf8');
    const line = manifest.split('\n').find(line => line.trim().split(/\s+/)[1] === name);
    if (!line) throw new Error(`Release checksum missing for ${name}`);
    expected = line.split(/\s+/)[0];
    if (!/^[a-f0-9]{64}$/.test(expected)) throw new Error('Invalid release checksum');
    if (await hashOf(path) === expected) {
      await saveChecksum(path, expected);
      return path;
    }
  }
  // The release binaries are linked against glibc: on musl (Alpine) one
  // downloads, verifies, then fails to start with a bare ENOENT.
  if (process.platform === 'linux' && !process.report?.getReport?.().header?.glibcVersionRuntime) {
    throw new Error('No release for musl libc (Alpine and the like); install with Cargo and rerun.');
  }
  await sweep();
  process.stderr.write(`Installing Scopelet ${version} (${target}) in user cache...\n`);
  const content = await bytes(`${base}/${name}`, 32 * 1024 * 1024);
  if (hash(content) !== expected) throw new Error('Release checksum mismatch');
  const temporary = `${path}.${randomUUID()}.tmp`;
  try {
    await writeFile(temporary, content, { flag: 'wx', mode: 0o700 });
    await chmod(temporary, 0o700);
    await rename(temporary, path);
    await saveChecksum(path, expected);
  } finally { await rm(temporary, { force: true }); }
  return path;
}

try {
  const child = spawn(await binary(), process.argv.slice(2), { stdio: 'inherit' });
  child.on('error', error => { process.stderr.write(`${error.message}\n`); process.exitCode = 2; });
  child.on('exit', (code, signal) => { process.exitCode = code ?? (128 + (constants.signals[signal] || 1)); });
  // Passed on to the binary, which handles each of them once however often
  // it arrives (a terminal also delivers an interrupt to it directly).
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, () => child.kill(signal));
} catch (error) {
  process.stderr.write(`scopelet setup: ${error.message}\n`);
  process.exitCode = 2;
}
