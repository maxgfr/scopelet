import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync, cpSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
const require = createRequire(import.meta.url);
const plugin = require('./release-plugin.cjs');
const logger = { log() {}, error() {} };
test('every commit releases, preserving feature and breaking-change semantics', async () => {
  for (const [message, expected] of [ ['docs: clarify install', 'patch'], ['ci: validate installation', 'patch'],
    ['fix: repair status', 'patch'], ['feat: automatic mode', 'minor'], ['feat!: change contract', 'major'], ['fix(api)!: change result shape', 'major'],
    ['feat: contract\n\nBREAKING CHANGE: incompatible output', 'major'] ]) {
    assert.equal(await plugin.analyzeCommits({}, { commits: [{ message }], logger, cwd: process.cwd() }), expected);
  }
  assert.equal(await plugin.analyzeCommits({}, { commits: [], logger }), null);
  for (const message of ['ordinary commit', 'fix:', 'feat: ', 'Fix: wrong type casing']) {
    await assert.rejects(plugin.analyzeCommits({}, { commits: [{ message }], logger }), /Conventional Commit/);
  }
  assert.equal(await plugin.analyzeCommits({}, {
    commits: [{message:'docs: update setup'}, {message:'feat: add mode'}, {message:'fix!: change output'}],
    logger, cwd:process.cwd(),
  }), 'major');
});
test('version synchronization changes release files without touching dependency versions', () => {
  const root=mkdtempSync(join(tmpdir(),'scopelet-release-'));
  try {
    for (const path of ['Cargo.toml','Cargo.lock','README.md','LICENSE','skills','scripts']) cpSync(path,join(root,path),{recursive:true});
    const before=readFileSync(join(root,'Cargo.lock'),'utf8');
    execFileSync('python3',[join(root,'scripts/prepare_release.py'),'9.8.7']);
    execFileSync('python3',[join(root,'scripts/check_skill.py'),'--pack']);
    const testEnv={...process.env};delete testEnv.NODE_TEST_CONTEXT;
    const launcherTests=execFileSync(process.execPath,['--test','scripts/launcher.test.mjs'],{cwd:root,env:testEnv,encoding:'utf8'});
    assert.match(launcherTests,/explicit compatible offline binary preserves arguments and exit status/);
    const after=readFileSync(join(root,'Cargo.lock'),'utf8');
    assert.equal(after.replace(/(name = "scopelet"\nversion = ")[^"]+/, '$1OLD'),before.replace(/(name = "scopelet"\nversion = ")[^"]+/, '$1OLD'));
    assert.match(readFileSync(join(root,'skills/scopelet/scripts/scopelet.mjs'),'utf8'),/const version = '9.8.7'/);
  } finally { rmSync(root,{recursive:true,force:true}); }
});

test('release assets reject missing or mismatched matrix versions', async () => {
  const { mkdirSync, writeFileSync, existsSync } = await import('node:fs');
  const root=mkdtempSync(join(tmpdir(),'scopelet-assets-'));
  try {
    const dist=join(root,'dist');mkdirSync(dist);
    const targets=['aarch64-apple-darwin','x86_64-apple-darwin','aarch64-unknown-linux-gnu','x86_64-unknown-linux-gnu'];
    for(const target of targets) {
      writeFileSync(join(dist,`scopelet-${target}`),`synthetic ${target}`);
      writeFileSync(join(dist,`scopelet-${target}.version`),'scopelet 0.2.0\n');
    }
    const script=join(process.cwd(),'scripts/release_checksums.py');
    writeFileSync(join(dist,`scopelet-${targets[0]}.version`),'scopelet 0.1.3\n');
    assert.throws(()=>execFileSync('python3',[script,'0.2.0'],{cwd:root,stdio:'pipe'}));
    assert.equal(existsSync(join(dist,'SHA256SUMS')),false);
    writeFileSync(join(dist,`scopelet-${targets[0]}.version`),'scopelet 0.2.0\n');
    execFileSync('python3',[script,'0.2.0'],{cwd:root});
    const sums=readFileSync(join(dist,'SHA256SUMS'),'utf8').trim().split('\n');
    assert.equal(sums.length,4);
    for(const target of targets) assert.equal(existsSync(join(dist,`scopelet-${target}.version`)),false);
  } finally {rmSync(root,{recursive:true,force:true});}
});


test('version resolution exposes the matrix version and rejects a publication race', async () => {
  const root=mkdtempSync(join(tmpdir(),'scopelet-next-version-'));
  const previousOutput=process.env.GITHUB_OUTPUT;
  const previousVersion=process.env.SCOPELET_RELEASE_VERSION;
  try {
    process.env.GITHUB_OUTPUT=join(root,'output');
    delete process.env.SCOPELET_RELEASE_VERSION;
    await plugin.verifyRelease({}, {nextRelease:{version:'0.2.0'}});
    assert.equal(readFileSync(process.env.GITHUB_OUTPUT,'utf8'),'version=0.2.0\n');
    process.env.SCOPELET_RELEASE_VERSION='0.2.0';
    await assert.rejects(plugin.verifyRelease({}, {nextRelease:{version:'0.2.1'}}), /changed after/);
    assert.equal(readFileSync(process.env.GITHUB_OUTPUT,'utf8'),'version=0.2.0\n');
  } finally {
    if(previousOutput===undefined)delete process.env.GITHUB_OUTPUT; else process.env.GITHUB_OUTPUT=previousOutput;
    if(previousVersion===undefined)delete process.env.SCOPELET_RELEASE_VERSION; else process.env.SCOPELET_RELEASE_VERSION=previousVersion;
    rmSync(root,{recursive:true,force:true});
  }
});

test('configured changelog preset renders feature and breaking-change release notes', async () => {
  const { generateNotes } = await import('@semantic-release/release-notes-generator');
  const config = require('../.releaserc.cjs').plugins.find(p => Array.isArray(p) && p[0] === '@semantic-release/release-notes-generator')[1];
  const notes = await generateNotes(config, {
    cwd: process.cwd(), logger,
    options: { repositoryUrl: 'https://github.com/maxgfr/scopelet.git' },
    lastRelease: { gitTag: 'v0.1.3' },
    nextRelease: { version: '0.2.0', gitTag: 'v0.2.0' },
    commits: [
      { hash: 'a'.repeat(40), message: 'feat: automatic compression' },
      { hash: 'b'.repeat(40), message: 'feat!: change output contract' },
    ],
  });
  assert.match(notes, /automatic compression/);
  assert.match(notes, /BREAKING CHANGES/);
  assert.match(notes, /change output contract/);
  assert.match(notes, /v0\.1\.3\.\.\.v0\.2\.0/);
});
