import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync, cpSync, readFileSync, rmSync, mkdirSync, writeFileSync, existsSync, utimesSync } from 'node:fs';
import { createHash } from 'node:crypto';
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
test('real merge commits do not block releases or hide invalid ordinary commits', async () => {
  const root = mkdtempSync(join(tmpdir(), 'scopelet-merge-release-'));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  try {
    git('init', '-b', 'main');
    git('config', 'user.name', 'Release test');
    git('config', 'user.email', 'release@example.invalid');
    git('commit', '--allow-empty', '-m', 'chore: initial');
    git('checkout', '-b', 'feature');
    git('commit', '--allow-empty', '-m', 'feat: add capability');
    const feature = git('rev-parse', 'HEAD');
    git('checkout', 'main');
    git('merge', '--no-ff', 'feature', '-m', 'Merge pull request #1 from example/feature');
    const merge = { hash: git('rev-parse', 'HEAD'), message: 'Merge pull request #1 from example/feature' };
    const context = { cwd: root, logger, commits: [merge, { hash: feature, message: 'feat: add capability' }] };
    assert.equal(await plugin.analyzeCommits({}, context), 'minor');
    assert.equal(await plugin.analyzeCommits({}, { ...context, commits: [merge] }), null);
    git('commit', '--allow-empty', '-m', 'Merge pull request #2 from example/fake');
    const fake = { hash: git('rev-parse', 'HEAD'), message: 'Merge pull request #2 from example/fake' };
    await assert.rejects(plugin.analyzeCommits({}, { ...context, commits: [merge, fake] }), /Conventional Commit/);
  } finally { rmSync(root, { recursive: true, force: true }); }
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
    const setup=readFileSync(join(root,'skills/scopelet/references/setup.md'),'utf8');
    assert.match(setup,/installs Scopelet \*\*9\.8\.7\*\*/);
    assert.match(setup,/--tag v9\.8\.7 /);
    assert.match(readFileSync(join(root,'README.md'),'utf8'),/--tag v9\.8\.7 /);
    // The checksums pinned for the previous version do not follow the bump.
    assert.match(readFileSync(join(root,'skills/scopelet/scripts/scopelet.mjs'),'utf8'),
      /\/\/ pinned-checksums:start\nconst pinned = \{\};\n\/\/ pinned-checksums:end/);
    // Running it again for the same version changes nothing and still passes.
    execFileSync('python3',[join(root,'scripts/prepare_release.py'),'9.8.7']);
    // The same bundle packs to the same bytes, whenever it is packed.
    const packed=readFileSync(join(root,'dist/scopelet.skill'));
    const later=new Date(Date.now()+86_400_000);
    for (const file of ['SKILL.md','scripts/scopelet.mjs','references/setup.md']) utimesSync(join(root,'skills/scopelet',file),later,later);
    execFileSync('python3',[join(root,'scripts/check_skill.py'),'--pack']);
    assert.ok(readFileSync(join(root,'dist/scopelet.skill')).equals(packed),'the skill archive must be reproducible');
  } finally { rmSync(root,{recursive:true,force:true}); }
});
test('a release pins its binaries in the launcher the skill carries, and lists the skill', () => {
  const root=mkdtempSync(join(tmpdir(),'scopelet-release-'));
  try {
    for (const path of ['Cargo.toml','Cargo.lock','README.md','LICENSE','skills','scripts']) cpSync(path,join(root,path),{recursive:true});
    const dist=join(root,'dist');mkdirSync(dist);
    const targets=['aarch64-apple-darwin','x86_64-apple-darwin','aarch64-unknown-linux-gnu','x86_64-unknown-linux-gnu'];
    const sha=content=>createHash('sha256').update(content).digest('hex');
    for(const target of targets) {
      writeFileSync(join(dist,`scopelet-${target}`),`synthetic 9.8.7 ${target}`);
      writeFileSync(join(dist,`scopelet-${target}.version`),'scopelet 9.8.7\n');
    }
    // The plugin's prepare step, in the copy: it runs its scripts from the
    // working directory.
    execFileSync(process.execPath,['-e',"require('./scripts/release-plugin.cjs').prepare({}, {nextRelease:{version:'9.8.7'}}).catch(error=>{console.error(error);process.exit(1)})"],{cwd:root,stdio:'pipe'});
    const launcher=readFileSync(join(root,'skills/scopelet/scripts/scopelet.mjs'),'utf8');
    assert.match(launcher,/const version = '9\.8\.7'/);
    const sums=readFileSync(join(dist,'SHA256SUMS'),'utf8').trim().split('\n').map(line=>line.split(/\s+/));
    assert.deepEqual(sums.map(([,name])=>name),[...targets.map(target=>`scopelet-${target}`),'scopelet.skill']);
    for(const target of targets) {
      const digest=sha(`synthetic 9.8.7 ${target}`);
      assert.ok(sums.some(([hash,name])=>hash===digest&&name===`scopelet-${target}`),target);
      assert.ok(launcher.includes(`  '${target}': '${digest}',\n`),`${target} is not pinned`);
    }
    // The skill was packed after the pins were written, and its line is its hash.
    assert.equal(sums[4][0],sha(readFileSync(join(dist,'scopelet.skill'))));
    const packed=execFileSync('python3',['-c',"import sys,zipfile;sys.stdout.write(zipfile.ZipFile(sys.argv[1]).read('scopelet/scripts/scopelet.mjs').decode())",join(dist,'scopelet.skill')],{encoding:'utf8'});
    assert.equal(packed,launcher);
    execFileSync('python3',[join(root,'scripts/check_skill.py')]);
  } finally { rmSync(root,{recursive:true,force:true}); }
});
test('version synchronization fails when a documented version would be left behind', async () => {
  const { writeFileSync } = await import('node:fs');
  const drifted=[
    ['README.md', text=>text.replace('--tag v','--tag=v')],
    ['skills/scopelet/references/setup.md', text=>text.replace(/installs Scopelet \*\*([^*]+)\*\*/,'installs Scopelet $1')],
    ['skills/scopelet/references/setup.md', text=>text.replace('--tag v','--branch v')],
  ];
  for (const [name,drift] of drifted) {
    const root=mkdtempSync(join(tmpdir(),'scopelet-release-'));
    try {
      for (const path of ['Cargo.toml','Cargo.lock','README.md','skills','scripts']) cpSync(path,join(root,path),{recursive:true});
      const original=readFileSync(join(root,name),'utf8');
      const changed=drift(original);
      assert.notEqual(changed,original,`${name}: the drift must apply`);
      writeFileSync(join(root,name),changed);
      const cargo=readFileSync(join(root,'Cargo.toml'),'utf8');
      assert.throws(()=>execFileSync('python3',[join(root,'scripts/prepare_release.py'),'9.8.7'],{stdio:'pipe'}),
        error=>error.stderr.toString().includes(name),`${name}: a stale version must fail the release`);
      // Nothing is half-written: every file keeps the version it had.
      assert.equal(readFileSync(join(root,'Cargo.toml'),'utf8'),cargo);
      assert.equal(readFileSync(join(root,name),'utf8'),changed);
    } finally { rmSync(root,{recursive:true,force:true}); }
  }
});

test('release assets reject missing or mismatched matrix versions', async () => {
  const root=mkdtempSync(join(tmpdir(),'scopelet-assets-'));
  try {
    const dist=join(root,'dist');mkdirSync(dist);
    // The launcher these binaries are pinned in, at their version.
    const launcher=join(root,'skills/scopelet/scripts/scopelet.mjs');
    mkdirSync(join(root,'skills/scopelet/scripts'),{recursive:true});
    const unpinned=readFileSync('skills/scopelet/scripts/scopelet.mjs','utf8').replace(/const version = '[^']+'/,"const version = '0.2.0'");
    writeFileSync(launcher,unpinned);
    const targets=['aarch64-apple-darwin','x86_64-apple-darwin','aarch64-unknown-linux-gnu','x86_64-unknown-linux-gnu'];
    for(const target of targets) {
      writeFileSync(join(dist,`scopelet-${target}`),`synthetic ${target}`);
      writeFileSync(join(dist,`scopelet-${target}.version`),'scopelet 0.2.0\n');
    }
    const script=join(process.cwd(),'scripts/release_checksums.py');
    writeFileSync(join(dist,`scopelet-${targets[0]}.version`),'scopelet 0.1.3\n');
    assert.throws(()=>execFileSync('python3',[script,'0.2.0'],{cwd:root,stdio:'pipe'}));
    assert.equal(existsSync(join(dist,'SHA256SUMS')),false);
    assert.equal(readFileSync(launcher,'utf8'),unpinned);
    writeFileSync(join(dist,`scopelet-${targets[0]}.version`),'scopelet 0.2.0\n');
    // A launcher at another version is not given these binaries' hashes.
    writeFileSync(launcher,unpinned.replace("'0.2.0'","'0.1.9'"));
    assert.throws(()=>execFileSync('python3',[script,'0.2.0'],{cwd:root,stdio:'pipe'}));
    assert.equal(existsSync(join(dist,'SHA256SUMS')),false);
    writeFileSync(launcher,unpinned);
    execFileSync('python3',[script,'0.2.0'],{cwd:root});
    const sums=readFileSync(join(dist,'SHA256SUMS'),'utf8').trim().split('\n');
    assert.equal(sums.length,4);
    for(const target of targets) assert.equal(existsSync(join(dist,`scopelet-${target}.version`)),false);
    for(const line of sums) {
      const [hash,name]=line.split(/\s+/);
      assert.ok(readFileSync(launcher,'utf8').includes(`  '${name.replace('scopelet-','')}': '${hash}',\n`),name);
    }
    // The packed skill is listed after them, once, however often it is added.
    writeFileSync(join(dist,'scopelet.skill'),'packed skill');
    for(let i=0;i<2;i++) execFileSync('python3',[script,'--skill'],{cwd:root});
    const listed=readFileSync(join(dist,'SHA256SUMS'),'utf8').trim().split('\n');
    assert.equal(listed.length,5);
    assert.deepEqual(listed.slice(0,4),sums);
    assert.match(listed[4],/^[a-f0-9]{64}  scopelet\.skill$/);
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
