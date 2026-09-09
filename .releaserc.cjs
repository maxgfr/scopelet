module.exports = {
  branches: ['main'],
  tagFormat: 'v${version}',
  plugins: [
    './scripts/release-plugin.cjs',
    ['@semantic-release/release-notes-generator', { preset: 'conventionalcommits' }],
    ['@semantic-release/git', {
      assets: ['Cargo.toml', 'Cargo.lock', 'skills/scopelet/SKILL.md',
        'skills/scopelet/scripts/scopelet.mjs', 'skills/scopelet/references/setup.md', 'README.md'],
      message: 'chore(release): ${nextRelease.version} [skip ci]'
    }],
    ['@semantic-release/github', {
      assets: ['dist/scopelet-*', 'dist/scopelet.skill', 'dist/SHA256SUMS'],
      successComment: false, failComment: false, failTitle: false,
      releasedLabels: false
    }]
  ]
};
