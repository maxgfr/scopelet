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
| `default` | Adaptive compression; routine edits use existing checks, then 1–3 short outcome/validation sentences. Enabled on first install. |
| `caveman` | Same compression; routine replies target 30 words, exceeding that when needed to retain meaning or meet the request. |
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

Small automatic outputs skip cache setup. Codex also leaves a plain `cat` of a
known regular file up to 2 KiB native. Small edits keep their original bytes and
still require verification; shortening a response never justifies skipping checks.

## Measured results

**Current 0.3.0 comparison:** 42 attempted sessions across three synthetic tasks, with two repetitions per configuration. Codex requested Luna at low effort; Claude Code used Haiku 4.5. All 41 graded main sessions pass. One RTK/Codex attempt lost its grade to a benchmark telemetry error; its tokens remain in the totals. Three integration preflights and eight targeted follow-ups bring the campaign to 53 attempts, below the 60-session cap.

| Host / task | Scopelet vs native | RTK vs native | Headroom vs native |
|---|---:|---:|---:|
| Luna / Small edit | -27.2% | +0.9% | unmeasured |
| Luna / Noisy command | -32.1% | +11.0% | unmeasured |
| Luna / JSONL aggregation | -17.6% | +41.0% | unmeasured |
| Haiku / Small edit | +0.2% | -9.1% | -32.3% |
| Haiku / Noisy command | +1.8% | +4.8% | -3.0% |
| Haiku / JSONL aggregation | -0.3% | -0.7% | +0.6% |

These are **whole-session logical input plus output tokens**, including cached input, not billed cost. Two repetitions are exploratory: the report shows individual values, durations, available cost estimates and failures. On Haiku, Scopelet’s reported cost is 3.0% below native and Headroom’s is 27.1% above native, so token savings do not directly predict cost savings. Headroom/Codex remains unmeasured because authenticated routing through the available adapter is unverified.

Scopelet’s value is strongest on noisy Codex output in this sample. It does not consistently beat native tools or Headroom across hosts and tasks. RTK’s specialized `test` wrapper is effective locally, while its automatic hook does not cover the custom Python test command used here. No new Rust runtime optimization is claimed; confirmed fixes improve the benchmark and its evidence.

[Current report and limitations](docs/current-comparison-2026-09-10.md) · [Per-session evidence](bench/results/current-comparison-2026-09-10.json) · [Local compression and recovery](bench/results/current-content-2026-09-10.json)

Earlier measurements remain available: [Luna automatic-mode pilot](docs/luna-auto-2026-09-09.md), [Fable comparisons](docs/fable-comparison-2026-09-09.md), [small-edit evaluation](docs/proportional-2026-09-09.md), and [0.3.0 engine rollout](docs/performance-2026-09-10.md). They concern different versions or integrations and are not pooled into the current results.

## Exact queries and recovery

For a standalone `scopelet` command, install with **Rust 1.88+**:

```sh
cargo install --git https://github.com/maxgfr/scopelet --tag v0.3.1 --locked
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
results and failures. [Engine and compact-v2 measurements](docs/performance-2026-09-10.md).
[Research and influences](docs/optimization-research-2026-09-09.md).

MIT · [Issues and support](https://github.com/maxgfr/scopelet/issues)

## Manual skill invocation

These skills run when explicitly invoked: `scopelet`. Use `$name` in Codex or `/name` in Claude Code and OpenCode (with the plugin namespace when installed as a Claude plugin).

The skill bundle disables implicit selection in Codex and Claude Code. OpenCode V2 reads `metadata.opencode/autoinvoke: "false"`. For OpenCode V1, merge these entries into `permission.skill` in `~/.config/opencode/opencode.json` or the project configuration; retain unrelated permissions:

```json
{
  "permission": {
    "skill": {
      "scopelet": "deny"
    }
  }
}
```

On OpenCode 1.18.30, these rules hide the skills from the agent and reject skill-tool loading, while explicit `/name` commands remain available. Installation with `skills add` does not apply this OpenCode V1 configuration.
