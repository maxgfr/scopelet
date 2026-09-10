# Setup

Install globally for both agents (Node 22.20+ for the skills installer):

```sh
npx skills add maxgfr/scopelet --skill scopelet --global -a codex claude-code -y
```

For automatic operation, activate the downloaded binary directly through the
launcher; a Cargo install or a `scopelet` command on PATH is not required:

```sh
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" install --agent all
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" doctor
```

Check `binary_installed` and both `hooks_configured` values. Restart existing
sessions and review new hooks with Codex `/hooks`; configuration checks cannot
verify interactive trust. First installation enables default mode.

Invoke `/scopelet <task>` in Claude Code or `$scopelet <task>` in Codex.
Explicit invocation accesses advanced queries; installed hooks run independently.
Run `node <installed-skill>/scripts/scopelet.mjs doctor` to check availability.
The launcher installs Scopelet **0.3.7** into the user's cache, downloading the
matching macOS/Linux release from github.com and verifying its SHA-256. Node 18+
is sufficient for the launcher itself. The launcher alone changes no agent
settings; `install` explicitly installs user hooks.

In the commands below, replace `scopelet` with
`node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs"` if it is not on PATH.
An independently installed binary is also supported:

```sh
cargo install --git https://github.com/maxgfr/scopelet --tag v0.3.7 --locked
```

Optional adapters are external dependencies, not bundled copies:

```sh
npm install -g @maxgfr/codeindex@2.30.0
brew install maxgfr/tap/webindex
scopelet doctor
```

Basic repository searches, files, JSON and commands work without them. The
`code`, `url` and `document` sources explain when their adapter is absent. If a
network sandbox blocks setup, allow github.com and release-asset hosts, or
install the release manually. A checksum/version mismatch is a failure; do not
substitute an unverified binary.

Cache: `SCOPELET_CACHE_DIR`, otherwise `$XDG_CACHE_HOME/scopelet`, otherwise
`~/.cache/scopelet`. `scopelet clean --older-days 7` removes old originals and
artifacts; `--older-days 0` removes all. References to removed snapshots expire.
Run cleanup when no Scopelet operation is using those artifacts. The `blobs` and
`artifacts` directories create local ignore markers to keep saved evidence out of
ordinary searches and Git staging, including when the cache is inside a repository.
Existing ignore rules and read-only caches are preserved; unwritable directories
may lack these markers; explicit no-ignore searches can still include it.

Remove the skill using `npx skills remove scopelet --global -a codex claude-code -y`; remove
a Cargo install with `cargo uninstall scopelet`. The launcher cache can be
deleted separately. Remove automatic hooks first with `scopelet uninstall --agent all`.
The binary and local cache can be removed separately. Scopelet configures no
telemetry or proxy.

## Automatic installation and modes

```sh
scopelet install --agent all
scopelet doctor
scopelet mode caveman
scopelet mode default
scopelet mode off
scopelet uninstall --agent all
```

Install accepts `claude`, `codex`, or `all`. It copies the resolved binary to
`$XDG_CONFIG_HOME/scopelet/bin/scopelet` (otherwise `~/.config/scopelet`), merges
user hooks, and backs up replaced configuration bytes. `SCOPELET_CONFIG_DIR`
overrides that root. Reinstall after upgrading the binary. Existing sessions
need restarting; in Codex review the hooks with `/hooks` when prompted.
Mode changes apply at the next prompt. `off` leaves hooks installed but disables
compression; uninstall removes only matching Scopelet hooks and keeps backups.

Default asks for relevant code and existing checks to be inspected before a
routine edit is verified, then reports outcome and validation in 1–3 short
sentences. Caveman targets 30 words for routine replies, retaining failures,
qualifications, numbers, negation and necessary next actions in the user's
language. Required information or requested detail can exceed these targets;
saved documents use normal prose. These are preferences, not output truncation
or changes to the model's reasoning effort.

Claude Code uses `PostToolUse.updatedToolOutput` for Bash results with known
stdout/stderr fields. Other fields survive unchanged. Failure events without a
replaceable output, images and unknown envelopes pass through. Codex uses
`PreToolUse.updatedInput` for simple Bash calls: cargo test/check/clippy/build,
pytest, python3 scripts, package-manager tests and single-file cat. Shell
expansions, pipelines, unsupported control operators, interactive flags and
existing wrappers pass through. Simple `&&` lists are supported as described below. This is not interception of all host tools or conversation history.
Codex's hook requires its documented `allow` rewrite decision; it does not set
sandbox, escalation, permission rules or permission mode. Other policy hooks
must remain enabled. Interactive commands should always use native tools.

Small automatic outputs do not open the cache. Codex leaves a plain `cat` of a
known regular file up to 2 KiB native; missing paths, unknown working directories
and larger files retain the normal command path. The host still reads the file
and enforces its permissions, so a subsequent file change is not hidden.

Compression leaves outputs up to 2 KiB intact. Larger outputs are replaced only
if the complete replacement saves at least 20% and 512 bytes, with a 4 KiB target
per stream. Repetitions have counts; selected lines stay exact; omissions and
immutable recovery references are explicit. A host-truncated input cannot be
restored to bytes the compressor never received. Existing persisted-output
previews and Scopelet output pass through. Capture is bounded at 32 MiB per
stream; interruption or overflow is reported. Storage/compression failures in an
automatic run return captured native output without rerunning the command.

A Codex AND-list (`cmd && cmd`) of at most eight individually recognized simple
commands is supported. It is reconstructed from quoted argv in `/bin/sh`;
short-circuiting and the final process status are preserved. Other shell control
operators and pipelines still pass through. Interactive/background tool calls
also pass through when the host exposes those flags.

To upgrade a global installation, run `npx skills update scopelet --global -y`,
then invoke the updated launcher with `install --agent all` and `doctor` again.
Restart active sessions and review changed Codex hooks. Update any project-local
copies too; they can shadow the global skill.

## Compact version and additional commands

Compact-v3 is the default. Set `SCOPELET_COMPACT_VERSION=1` or `2` in the agent
process environment to restore an earlier presentation for installed hooks;
unset it or set `3` to use v3. Direct CLI calls can override it with
`--compact-version 1|2|3`. V2 added partial JSON tables and broader diagnostic
coverage; v3 adds a leaner envelope; see [queries.md](queries.md) for exact
recovery semantics.
The JSON query interface and old saved artifacts remain compatible.

Codex also recognizes simple `rg`/`grep`, Git diff/log/show/status without forced
pagination, `go test`, `node --test`, and package-manager build/lint/typecheck
scripts. Shell pipelines and unsupported syntax still run natively. Recognition
permits wrapping; it does not guarantee compression. Outputs are replaced only
when the complete view clears the existing savings gate. Rejected compression
now avoids opening the cache as well as leaving the output unchanged.

Range expansion can create disposable `line-index-v1` cache files. They carry
local ignore markers and are cleaned with their originals or when old. Index
failures do not prevent recovery; original blob hashes and source-line
boundaries are verified before an index is used.
