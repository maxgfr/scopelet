#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { spawnSync, spawn } from 'node:child_process';
import { mkdir, readFile, writeFile, rename, chmod, rm } from 'node:fs/promises';
import { homedir, constants } from 'node:os';
import { join } from 'node:path';

const version = '0.4.0';
const targets = { 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu', 'linux-arm64': 'aarch64-unknown-linux-gnu' };
const target = targets[`${process.platform}-${process.arch}`];
const root = join(process.env.XDG_CACHE_HOME || join(homedir(), '.cache'), 'scopelet', 'bin', version);

async function bytes(url, limit) {
  const response = await fetch(url, { signal: AbortSignal.timeout(120_000) });
  if (!response.ok) throw new Error(`Download failed: HTTP ${response.status} (${url})`);
  const chunks = []; let length = 0;
  for await (const chunk of response.body) {
    length += chunk.length;
    if (length > limit) throw new Error('Release download exceeds size limit');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

async function saveChecksum(path, expected) {
  const temporary = `${path}.${process.pid}.tmp.sha256`;
  try {
    await writeFile(temporary, expected, { flag: 'wx', mode: 0o600 });
    await rename(temporary, `${path}.sha256`);
  } finally { await rm(temporary, { force: true }); }
}

async function binary() {
  // Explicit override is useful for offline installs and independent tests.
  const override = process.env.SCOPELET_BIN;
  const candidate = override || 'scopelet';
  const check = spawnSync(candidate, ['--version'], { encoding: 'utf8', timeout: 5000 });
  if (check.status === 0 && check.stdout.trim() === `scopelet ${version}`) return candidate;
  if (override) throw new Error(`SCOPELET_BIN must point to scopelet ${version}`);
  if (!target) throw new Error('No release for this platform; install with Cargo and rerun.');
  await mkdir(root, { recursive: true, mode: 0o700 });
  const path = join(root, 'scopelet');
  const hash = buffer => createHash('sha256').update(buffer).digest('hex');
  try {
    const cached = (await readFile(`${path}.sha256`, 'utf8')).trim();
    if (/^[a-f0-9]{64}$/.test(cached) && hash(await readFile(path)) === cached) return path;
  } catch {}
  const name = `scopelet-${target}`;
  const base = `https://github.com/maxgfr/scopelet/releases/download/v${version}`;
  const manifest = (await bytes(`${base}/SHA256SUMS`, 16384)).toString('utf8');
  const line = manifest.split('\n').find(line => line.trim().split(/\s+/)[1] === name);
  if (!line) throw new Error(`Release checksum missing for ${name}`);
  const expected = line.split(/\s+/)[0];
  if (!/^[a-f0-9]{64}$/.test(expected)) throw new Error('Invalid release checksum');
  let existingHash;
  try { existingHash = hash(await readFile(path)); } catch {}
  if (existingHash === expected) {
    await saveChecksum(path, expected);
    return path;
  }
  process.stderr.write(`Installing Scopelet ${version} (${target}) in user cache...\n`);
  const content = await bytes(`${base}/${name}`, 32 * 1024 * 1024);
  if (hash(content) !== expected) throw new Error('Release checksum mismatch');
  const temporary = `${path}.${process.pid}.tmp`;
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
  process.on('SIGINT', () => child.kill('SIGINT'));
  process.on('SIGTERM', () => child.kill('SIGTERM'));
} catch (error) {
  process.stderr.write(`scopelet setup: ${error.message}\n`);
  process.exitCode = 2;
}
