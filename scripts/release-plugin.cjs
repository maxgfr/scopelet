const fs = require('node:fs');
const { execFileSync } = require('node:child_process');

// Require Conventional Commits; every valid change releases at least a patch.
exports.analyzeCommits = async (config, context) => {
  const commits = context.commits.filter(({ message, hash }) => {
    const header = message.split(/\r?\n/, 1)[0];
    if (/^[a-z][a-z0-9-]*(?:\([^()\r\n]+\))?!?: \S.*$/.test(header)) return true;
    // Git-generated merge messages are metadata. Check parentage instead of
    // trusting a "Merge ..." prefix that an ordinary commit could also use.
    if (typeof hash === 'string' && /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(hash)) {
      const parents = execFileSync('git', ['show', '-s', '--format=%P', hash, '--'], {
        cwd: context.cwd, encoding: 'utf8'
      }).trim().split(/\s+/).filter(Boolean);
      if (parents.length > 1) return false;
    }
    throw new Error(`Expected a Conventional Commit (type(scope): description): ${header}`);
  });
  if (!commits.length) return null;
  const { analyzeCommits } = await import('@semantic-release/commit-analyzer');
  return await analyzeCommits({ preset: 'conventionalcommits' }, { ...context, commits }) || 'patch';
};
exports.verifyRelease = async (config, { nextRelease }) => {
  const version = nextRelease.version;
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Expected a stable semantic version');
  if (process.env.SCOPELET_RELEASE_VERSION && process.env.SCOPELET_RELEASE_VERSION !== version)
    throw new Error('Release version changed after the matrix build');
  if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\n`);
};
exports.prepare = async (config, { nextRelease }) => {
  execFileSync('python3', ['scripts/prepare_release.py', nextRelease.version], { stdio: 'inherit' });
  execFileSync('python3', ['scripts/check_skill.py', '--pack'], { stdio: 'inherit' });
  execFileSync('python3', ['scripts/release_checksums.py', nextRelease.version], { stdio: 'inherit' });
};
