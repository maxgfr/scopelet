const fs = require('node:fs');
const { execFileSync } = require('node:child_process');

// Require Conventional Commits; every valid commit releases at least a patch.
exports.analyzeCommits = async (config, context) => {
  if (!context.commits.length) return null;
  for (const { message } of context.commits) {
    const header = message.split(/\r?\n/, 1)[0];
    if (!/^[a-z][a-z0-9-]*(?:\([^()\r\n]+\))?!?: \S.*$/.test(header)) {
      throw new Error(`Expected a Conventional Commit (type(scope): description): ${header}`);
    }
  }
  const { analyzeCommits } = await import('@semantic-release/commit-analyzer');
  return await analyzeCommits({ preset: 'conventionalcommits' }, context) || 'patch';
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
