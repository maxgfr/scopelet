const fs = require('node:fs');
const { execFileSync } = require('node:child_process');

// Retain conventional major/minor semantics; every other commit gets a patch.
exports.analyzeCommits = async (config, context) => {
  if (!context.commits.length) return null;
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
