# Scopelet

Local, recoverable compression for **Codex and Claude Code**. Scopelet reduces
noisy command output before it enters model context, keeps original bytes for
recovery, and computes exact queries over large files. No extra model call, API
key or proxy.

## Install and enable automatic mode

Requires **Node.js 22.20+** for the `skills` installer. The bundled launcher itself
supports Node 18+. Release binaries support macOS Intel/ARM and Linux Intel/ARM
with Ubuntu 24.04-compatible glibc. Native Windows is not supported.

```sh
npx skills add maxgfr/scopelet --skill scopelet --global -a codex claude-code -y
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" install --agent all
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" doctor
```

The first command installs the skill. The second downloads its pinned release,
checks SHA-256, and installs the automatic hooks with **default mode enabled**.
The third reports the installed binary and `hooks_configured` for both agents.
The skill installer does **not** put a `scopelet` command on your PATH.

Restart active agent sessions. In Codex, review and trust the new hooks through
`/hooks` when prompted; `doctor` checks configuration, not interactive trust.
After activation, ordinary prompts use the hooks without `/scopelet` or
`$scopelet`. Use `--agent codex` or `--agent claude` to enable only one host.

For manual use only, run the first command and invoke `/scopelet <task>` in
Claude Code or `$scopelet <task>` in Codex. Rust is not required for this setup.
[Full setup and host coverage](skills/scopelet/references/setup.md).

## Choose the response style

| Mode | Behavior |
| --- | --- |
| `default` | Adaptive compression and brief, normal replies. Enabled on first install. |
| `caveman` | Same compression, with telegraphic replies that retain necessary information. |
| `off` | Disable automatic compression and its response preference. |

```sh
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" mode caveman
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" mode default
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" mode off
```

Mode changes apply at the next prompt. They change neither the model nor its
reasoning effort. Caveman is optional: shorter wording does not guarantee a
cheaper whole session. Saved documents use normal prose.

## What gets compressed

Codex hooks wrap recognized noninteractive shell commands, including tests,
builds, Python scripts and simple eligible `&&` chains. Claude Code hooks replace
Bash output when the host supplies a supported result shape. Other tools,
unsupported shell syntax and existing output wrappers pass through.

Outputs up to 2 KiB stay byte-exact. Larger outputs are replaced only when the
complete view saves at least 512 bytes and 20%, with a 4 KiB target per stream.
Scopelet first factors repeated JSON keys without dropping values, then selects
whole evidence units when needed. Repeated lines retain counts; omissions are
explicit; original captured bytes remain recoverable. Exit status survives.
Already persisted host previews pass through. [Contracts and limits](docs/design.md).

## Measured results

**52/52 Luna sessions passed:** 48 comparative sessions and four final-binary
checks, including Codex code mode. The comparison requested `gpt-5.6-luna`, low
effort, over three synthetic tasks with four repetitions per arm and no skill
invocation.

| Task | Default vs native | Caveman vs native | RTK vs native |
| --- | ---: | ---: | ---: |
| Small edit | +0.4% | -7.0% | +34.1% |
| Noisy command | **-40.7%** | **-41.5%** | -29.5% |
| JSONL aggregation | -16.0% | +1.9% | +10.4% |
| Combined task mix | **-23.4%** | **-21.5%** | -2.4% |

These are **whole-session logical input plus output tokens**, including cached
input, not billed cost or a universal savings guarantee. Compression activated
on noisy commands; differences on the other tasks combine concise instructions
with model variability. Caveman did not consistently beat default. RTK used its
documented Codex awareness instructions; Headroom is unmeasured because the
available adapter did not route Codex requests. No Claude Code model calls were
made in this campaign; that adapter has offline contract checks here.

[Full report and variance](docs/luna-auto-2026-09-09.md) ·
[Per-session evidence](bench/results/luna-auto-2026-09-09.json) ·
[Installation verification](bench/results/install-0.2.0-2026-09-09.json)

Earlier [released-0.1.3 comparisons](docs/fable-comparison-2026-09-09.md) found
strong noisy-command savings on Haiku, but none on Fable where Claude Code
already persisted large outputs. Headroom was the strongest general competitor
there. Those measurements concern different versions and integrations.

## Exact queries and recovery

For a standalone `scopelet` command, install with **Rust 1.88+**:

```sh
cargo install --git https://github.com/maxgfr/scopelet --tag v0.2.0 --locked
scopelet query --repo . --find validateToken --context 5
scopelet query --file events.jsonl --format jsonl --filter /status --equals '"failed"' --group /suite --output compact
scopelet run --auto -- npm test
```

Without Cargo, replace `scopelet` with
`node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs"`.
Keep native tools for small known files. Compose filtering, projection, grouping
and counting locally instead of sending an entire dataset to the model.

```sh
scopelet expand artifact:HASH --manifest
scopelet expand blob:HASH --start 30 --end 70
scopelet expand blob:HASH --raw > original.bin
```

A partial view cannot prove absence or an exhaustive count. Recover exact bytes
before editing omitted evidence. CLI display options are separate from response
preferences: default JSON views use 16 KiB; `--mode ultra` uses 4 KiB and may
abridge text. Budgets are bytes, not token estimates. [Query reference](skills/scopelet/references/queries.md).

Optional [codeindex](https://github.com/maxgfr/codeindex) and
[webindex](https://github.com/maxgfr/webindex) adapters add code relationships and
document extraction. Basic files, logs, JSON and repository queries need neither.

## Upgrade or remove

```sh
npx skills update scopelet --global -y
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" install --agent all
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" doctor
```

Reinstalling updates the pinned hook binary and preserves unrelated hooks.
Restart current sessions and review changed hooks in Codex. Update project-local
skill copies too if you use them; they can shadow the global installation.

Remove hooks before removing the skill:

```sh
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" uninstall --agent all
npx skills remove scopelet --global -a codex claude-code -y
```

Uninstall preserves configuration backups and cached originals. Clean old
artifacts separately with `clean --older-days 7`; references expire when their
snapshots are removed. [Paths, backups and cleanup](skills/scopelet/references/setup.md).

## Automatic releases from semantic commits

Push **Conventional Commits** to `main`; semantic-release determines the version
and publishes binaries, checksums and the installable skill after CI succeeds.

| Commit example | Version change |
| --- | --- |
| `fix: preserve exit status` | Patch |
| `feat: add a query operation` | Minor |
| `feat!: change the output contract` or a `BREAKING CHANGE:` footer | Major |
| `docs: clarify installation`, `test: cover recovery`, `ci: verify packages` | Patch |

Scopes work too: `fix(launcher): handle a missing cache`. Nonsemantic messages
fail release validation. Use semantic titles when squash-merging. Every valid
commit produces at least a patch; a push containing multiple commits produces
one release with the highest applicable version change. The generated
`chore(release): VERSION [skip ci]` commit avoids a release loop.

The [workflow](.github/workflows/release.yml) tests Linux/macOS and Rust 1.88,
sets the next version before compilation, then checks the versioned launcher
and both host installations on all four platforms. Cargo, the skill, its launcher
and README install tag are synchronized. Publication stops if the branch or
resolved version changed during the build. No npm or crates.io package is published.

## Development checks

Use Rust 1.88+, Python 3.9+ and Node 24.10+ for the development/release tooling.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 scripts/check_skill.py --pack
python3 -m unittest discover -s bench -p 'test_*.py'
npm ci --ignore-scripts
npm test
cargo build --release --locked
python3 scripts/check_install.py --binary target/release/scopelet
```

The installation check uses temporary host configurations, preserves unrelated
hooks, exercises default/caveman/off, and verifies uninstall. It makes no model
calls. [Benchmark reproduction](bench/README.md) requires explicit `--live` for
model sessions. [Verification history](docs/verification.md) preserves earlier
results and failures. [Research and influences](docs/optimization-research-2026-09-09.md).

MIT · [Issues and support](https://github.com/maxgfr/scopelet/issues)
