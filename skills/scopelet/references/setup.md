# Setup

Install globally for all three hosts (Node 22.20+ for the skills installer):

```sh
npx skills add maxgfr/scopelet --skill scopelet --global -a codex claude-code opencode -y
```

For automatic operation, activate the downloaded binary directly through the
launcher; a Cargo install or a `scopelet` command on PATH is not required:

```sh
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" install --agent all
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" doctor
```

Check `binary_installed` and the three `hooks_configured` values. Restart
existing sessions and review new hooks with Codex `/hooks`; configuration checks
cannot verify interactive trust. OpenCode loads the plugin at startup with no
trust step. First installation enables default mode.

Invoke `/scopelet <task>` in Claude Code or OpenCode, `$scopelet <task>` in Codex.
Explicit invocation accesses advanced queries; installed hooks run independently.
Run `node <installed-skill>/scripts/scopelet.mjs doctor` to check availability.
The launcher installs Scopelet **0.6.0** into the user's cache, downloading the
matching macOS/Linux release from github.com and verifying its SHA-256. Node 18+
is sufficient for the launcher itself. The launcher alone changes no agent
settings; `install` explicitly installs user hooks.

In the commands below, replace `scopelet` with
`node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs"` if it is not on PATH.
An independently installed binary is also supported:

```sh
brew install maxgfr/tap/scopelet
cargo install --git https://github.com/maxgfr/scopelet --tag v0.6.0 --locked
```

Homebrew is the one install that puts `scopelet` on PATH and needs no Node.
`scopelet install --agent all` then copies that binary to
`~/.config/scopelet/bin/scopelet`, which the hooks call, so re-run it after
`brew upgrade scopelet`. The formula does not carry the skill.

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
artifacts; `--older-days 0` removes all. Add `--max-size 500M` (K, M or G) to
then evict least recently used items until the cache fits; items modified in
the last hour stay. It prints `removed_items`, `removed_bytes`, `kept_bytes`,
`corrupt` (damaged artifacts removed) and `sessions_removed` (aged hook session
markers). An artifact from a newer release is kept, with every original, until
that release cleans it. References to removed snapshots expire.
Run cleanup when no Scopelet operation is using those artifacts. The `blobs` and
`artifacts` directories create local ignore markers to keep saved evidence out of
ordinary searches and Git staging, including when the cache is inside a repository.
Existing ignore rules and read-only caches are preserved; unwritable directories
may lack these markers; explicit no-ignore searches can still include it.

Remove the skill using `npx skills remove scopelet --global -a codex claude-code opencode -y`; remove
a Cargo install with `cargo uninstall scopelet`. The launcher cache can be
deleted separately. Remove automatic hooks first with `scopelet uninstall --agent all`.
The binary and local cache can be removed separately. Scopelet configures no
proxy and sends nothing anywhere: it keeps a local journal, never transmitted,
of its compression decisions in `<cache>/events/YYYY-MM.jsonl` (time, host,
profile, byte counts, reason, version and short hash prefixes; no content,
command or path). `scopelet stats [--days N]` summarizes it: compressions,
bytes saved, why outputs passed through and how often a compressed view was
expanded afterwards. `SCOPELET_EVENTS=0` turns the journal off; `clean` does
not remove it.

## Automatic installation and modes

```sh
scopelet install --agent all
scopelet doctor
scopelet mode caveman
scopelet mode default
scopelet mode off
scopelet uninstall --agent all
```

Install accepts `claude`, `codex`, `opencode`, or `all`. It copies the resolved
binary to `$XDG_CONFIG_HOME/scopelet/bin/scopelet` (otherwise `~/.config/scopelet`),
merges user hooks, and backs up replaced configuration bytes. `SCOPELET_CONFIG_DIR`
overrides that root. Reinstall after upgrading the binary: `doctor` compares the
pinned copy with the running binary (`binary_current`, `binary_version`,
`running_version`) and says when hooks still run an older one. The session
guidance names the pinned binary's absolute path for recovery, so `expand`
works without `scopelet` on the PATH. Existing sessions need restarting; in
Codex review the hooks with `/hooks` when prompted.

OpenCode has no hooks file: install writes a self-contained plugin to
`plugin/scopelet.js` under `OPENCODE_CONFIG_DIR`, otherwise
`$XDG_CONFIG_HOME/opencode` (`~/.config/opencode`). The plugin calls the pinned
binary asynchronously from `tool.execute.after` for the `bash` tool, with the
binary's own size thresholds written in at installation, and adds the mode text to
the system prompt on every step, so a mode change applies immediately there.
Install refuses to replace a plugin file it did not write, and uninstall removes
only its own file; backups land in the Scopelet config directory like the
others.

Mode changes apply at the next prompt (the next model step in OpenCode). `off`
leaves hooks installed but disables compression; uninstall removes only
matching Scopelet hooks and keeps backups.

Default asks for relevant code and existing checks to be inspected before a
routine edit is verified, then reports outcome and validation in 1–3 short
sentences. Caveman targets 30 words for routine replies, retaining failures,
qualifications, numbers, negation and necessary next actions in the user's
language. Required information or requested detail can exceed these targets;
saved documents use normal prose. These are preferences, not output truncation
or changes to the model's reasoning effort.

OpenCode's plugin replaces the `bash` tool's `output` string in place when the
same thresholds apply; other tools, MCP results and failures pass through, and
any plugin or binary error leaves the native output untouched. OpenCode caps
long output before the plugin sees it and writes the whole result to its
`tool-output` directory; the view keeps that "Full output saved to" line, so
`expand` recovers what OpenCode handed over and the file holds the rest.
Claude Code uses `PostToolUse.updatedToolOutput` for Bash results with known
stdout/stderr fields. Other fields survive unchanged. Failure events without a
replaceable output, images and unknown envelopes pass through. MCP tool results
pass through too: Claude Code's hook documentation does not say how a
PostToolUse replacement applies to an MCP result's content, and Scopelet does
not rely on unverified behavior. Codex uses
`PreToolUse.updatedInput` for recognized Bash calls: cargo test/check/clippy/build,
pytest, python3 scripts, package-manager tests and single-file cat. Quoted
arguments, `2>&1`, `cd DIR &&`, `&&`, `|| true`, display and test variable
prefixes and pipes into read-only filters are understood (see below); shell
expansions, other control operators, interactive flags and existing wrappers
pass through. This is not interception of all host tools or conversation history.
Codex's hook requires its documented `allow` rewrite decision; it does not set
sandbox, escalation, permission rules or permission mode. Checked against the
Codex 0.160 source, then with its own test harness (mock model, no network):
`allow` only carries the rewritten input, which then goes through Codex's
normal approval and sandbox policy. Under an untrusted policy, and for an
escalated on-request call, the rewritten command raised the approval prompt and
did not run before it was answered; only a `PermissionRequest` hook approves a
call. Wrapping a command never skips a prompt the native command would have
needed. Other policy hooks
must remain enabled. Interactive commands should always use native tools.

Small automatic outputs do not open the cache. Codex leaves a plain `cat` of a
known regular file up to 2 KiB native; missing paths, unknown working directories
and larger files retain the normal command path. The host still reads the file
and enforces its permissions, so a subsequent file change is not hidden.

Compression leaves outputs up to 2 KiB intact (16 KiB for a recognized file
read). Larger outputs are replaced only if the complete replacement saves at
least 20% and 512 bytes, with a 4 KiB target per stream, or the recognized
command's target in compact-v3. Repetitions have counts; selected lines stay exact; omissions and
immutable recovery references are explicit. A host-truncated input cannot be
restored to bytes the compressor never received. Existing persisted-output
previews and Scopelet output pass through. Capture is bounded at 32 MiB per
stream; interruption or overflow is reported. Storage/compression failures in an
automatic run return captured native output without rerunning the command.

Codex commands are parsed, not matched on raw characters: single quotes,
double quotes without expansions and backslash escapes are understood. Up to
eight recognized commands may be joined by `&&`, preceded by `cd DIR &&` or
followed by `|| true`; `2>&1` and redirections to `/dev/null` are kept; a
command may pipe into read-only filters (`grep`, `rg`, `sort`, `uniq`, `cut`,
`tr`, `nl`, `cat`, `jq`, print-only `sed -n`). A pipeline already ending in
`head`, `tail`, `wc` or `grep -c`/`-l`/`-q` stays native. Variable prefixes are
limited to display, locale and test knobs (`CI`, `NO_COLOR`, `RUST_BACKTRACE`,
`LC_*`...). Anything else (expansions, `;`, `&`, other redirections, `tee`,
`xargs`, `awk`) runs natively. Composed commands run through `/bin/sh -c`
unchanged, so short-circuiting and exit status are the shell's.
Interactive/background tool calls also pass through when the host exposes
those flags.

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

Hooks recognize builds, tests and checks (`cargo test|check|clippy|build|nextest|doc`,
`cargo fmt --check`, `pytest`, `python -m pytest|unittest|mypy`, Python scripts,
`npm`/`pnpm`/`yarn` test/build/lint/typecheck, `bun test`, `make
test|check|build|lint`, `go test|build|vet`, `ruff`, `mypy`, Gradle/Maven/.NET
test or build, `node --test`, and `jest`, `vitest run`, `tsc`, `eslint`,
`prettier --check` or `playwright test` through `npx` (also `-y`), `bunx`,
`yarn`, `pnpm exec` or
`uv run`); searches and listings (`rg`, `grep`, `find` without
`-delete`/`-exec`/`-ok`/`-fprint`, `ls -R`, `tree` without `-o`); Git `diff`, `log` (with
`-p`), `show`, `status`, `blame` and `grep` with no option before the
subcommand other than `--no-pager` or `-C DIR` (`-c` can name a program) and
none that pages, runs a program or writes a file (`--paginate`, `--ext-diff`,
`--textconv`, `--output`, `grep -O`); logs (`docker
logs`/`kubectl logs` without `-f`, `jq` on a file); and file reads (`cat`,
`nl`, `head`/`tail` without `-f`, print-only `sed -n`). Scopelet itself and
`rtk` are recognized by the program they run, not by a substring.
In compact-v3 each kind has its own byte target: 4 KiB for tests, 8 KiB for
searches, Git and logs, 16 KiB for file reads, which also stay exact up to
16 KiB so an ordinary source file is read whole. Unsupported syntax still runs
natively. Recognition
permits wrapping; it does not guarantee compression. Outputs are replaced only
when the complete view clears the existing savings gate. Rejected compression
now avoids opening the cache as well as leaving the output unchanged.

Range expansion can create disposable `line-index-v1` cache files. They carry
local ignore markers and are cleaned with their originals or when old. Index
failures do not prevent recovery; original blob hashes and source-line
boundaries are verified before an index is used.
