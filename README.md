<p align="center">
  <strong>Scopelet</strong><br>
  Your agent reads a 17 KB test log to learn one thing. Send it the one thing.
</p>

<p align="center">
  <a href="https://github.com/maxgfr/scopelet/releases"><img src="https://img.shields.io/github/v/release/maxgfr/scopelet?style=flat&color=blue" alt="Release"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT-green?style=flat" alt="MIT"></a>
  <a href="#install"><img src="https://img.shields.io/badge/hosts-Claude_Code_%2B_Codex_%2B_OpenCode-orange?style=flat" alt="Claude Code, Codex and OpenCode"></a>
  <a href="#the-numbers"><img src="https://img.shields.io/badge/no-extra_model_call-lightgrey?style=flat" alt="No extra model call"></a>
  <a href="https://skills.sh/maxgfr/scopelet"><img src="https://skills.sh/b/maxgfr/scopelet"></a>
</p>

<p align="center">
  <a href="#see-it">See it</a> ·
  <a href="#why-you-want-this">Why</a> ·
  <a href="#install">Install</a> ·
  <a href="#the-numbers">Numbers</a> ·
  <a href="#nothing-is-lost">Recovery</a> ·
  <a href="#exact-queries">Queries</a> ·
  <a href="#what-gets-compressed">Scope</a> ·
  <a href="docs/design.md">Contracts</a>
</p>

---

## See it

Your test suite prints 481 results. One of them matters.

<table>
<tr>
<th width="50%">Without Scopelet · 17,549 bytes</th>
<th width="50%">With Scopelet · 760 bytes</th>
</tr>
<tr>
<td valign="top">

```
> app@1.4.2 test
> jest --runInBand

PASS  src/modules/module0.test.js
PASS  src/modules/module1.test.js
PASS  src/modules/module2.test.js
        ... 237 more PASS lines ...
FAIL  src/auth/session.test.js
  ● session › refreshes an expiring token

    expect(received).toBe(expected)

    Expected: 1735689600
    Received: 1735689599

      at Object.<anonymous> (src/auth/session.test.js:42:31)
PASS  src/modules/module240.test.js
        ... 239 more PASS lines ...

Test Suites: 1 failed, 480 passed, 481 total
Tests:       1 failed, 1327 passed, 1328 total
Time:        84.113 s
```

</td>
<td valign="top">

```
[scopelet compact-v3 artifact:90da6b1f… scan_complete=true;
 recover: scopelet expand ID --find TEXT (or --manifest)]
input:1-3
 > app@1.4.2 test
 > jest --runInBand

input:4 similar=480 last=492 PASS  src/modules/module0.test.js
input:244-252
 FAIL  src/auth/session.test.js
   ● session › refreshes an expiring token

     expect(received).toBe(expected)

     Expected: 1735689600
     Received: 1735689599

       at Object.<anonymous> (src/auth/session.test.js:42:31)
input:493-496

 Test Suites: 1 failed, 480 passed, 481 total
 Tests:       1 failed, 1327 passed, 1328 total
 Time:        84.113 s
[scopelet display_complete=true omitted_units=0; gaps are
 omitted, not evidence of absence]
```

</td>
</tr>
</table>

The failure kept its expected value, its received value and its stack frame.
The 480 passes became one line that still says there were 480 and where the
last one was. `display_complete=true` means nothing at all was dropped: every
line of that log is either shown or folded into a counted group.

This happens automatically, on the machine, with no second model call, no API
key and no proxy. The original 17,549 bytes are still on disk, and the header
tells the agent how to get any of them back.

## Why you want this

Agents read far more than they write. Test output, build logs, `git diff`,
JSON dumps: all of it enters the context window verbatim, and most of it is
the same line five hundred times. That is the part of the bill nobody looks
at, and it is also what pushes a session into compaction and makes the agent
forget what it was doing.

Scopelet sits on the hook your agent already has. When a command prints
something large, it replaces the output with a bounded view before the model
sees it, keeps the exact original bytes in a local content-addressed store,
and puts the recovery command in the first line. The agent can always ask for
the rest. It usually does not need to.

What it is not: it does not call another model to summarize, it does not
intercept API traffic, it does not rewrite your conversation history, and it
does not touch small outputs at all.

## Install

Requires **Node.js 22.20+** for the `skills` installer. The bundled launcher
itself supports Node 18+. Release binaries cover macOS Intel/ARM and Linux
Intel/ARM with Ubuntu 24.04-compatible glibc. Native Windows is not supported.

```sh
npx skills add maxgfr/scopelet --skill scopelet --global -a codex claude-code opencode -y
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" install --agent all
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" doctor
```

The first command installs the skill. The second downloads its pinned release,
checks SHA-256, and installs the automatic hooks for Claude Code, Codex and
OpenCode with **default mode enabled**. The third reports the installed binary
and `hooks_configured` for all three hosts. The skill installer does **not**
put a `scopelet` command on your PATH.

Restart active agent sessions. In Codex, review and trust the new hooks through
`/hooks` when prompted; `doctor` checks configuration, not interactive trust.
OpenCode picks its plugin up at startup with no trust step. After activation,
ordinary prompts use the hooks without `/scopelet` or `$scopelet`. Use
`--agent claude`, `--agent codex` or `--agent opencode` to enable one host.

### Automatic or manual

Scopelet is **automatic by default**: the hooks compress on every large output,
with no invocation and no decision from the model. Manual is always available,
and both switches are yours.

- **Skip the hooks entirely.** Run only the first command and invoke
  `/scopelet <task>` in Claude Code or OpenCode, `$scopelet <task>` in Codex.
  Rust is not required for this setup.
- **Stop automatic compression later.** `mode off` keeps the hooks installed and
  idle; `uninstall --agent all` removes them and leaves the skill.
- **Hide the skill from the model.** The shipped skill is model-invocable, so an
  agent can reach the queries below on its own. To make it explicit-only, add
  `disable-model-invocation: true` to `SKILL.md` for Claude Code, set
  `allow_implicit_invocation: false` in `agents/openai.yaml` for Codex, and set
  `metadata.opencode/autoinvoke: 'false'` for OpenCode. Reinstalling restores
  the shipped default.

[Full setup and host coverage](skills/scopelet/references/setup.md).

## The numbers

### Smaller

Every row is a real command output through `scopelet compress` at the default
4 KiB budget, cold cache, thirty repetitions. Reproduce with
`bench/content.py`; the fixtures are pinned by SHA-256 and by
`tests/content_gate.rs` in CI, and `scripts/check_readme.py` fails the build if
any figure below drifts from what the binary produces.

| What the command printed | Bytes in | Bytes to the model | Kept |
| --- | ---: | ---: | ---: |
| 480 passing tests, one failure | 17,549 | 760 | **95.7%** |
| A retry loop hiding one fatal error | 28,039 | 421 | **98.5%** |
| 1000 progress lines, then two diagnostics | 136,063 | 516 | **99.6%** |
| The same log with CRLF endings | 137,031 | 472 | **99.7%** |
| A receipt buried in the middle of a log | 136,193 | 634 | **99.5%** |
| A 1000-row JSON array | 171,834 | 4,061 | **97.6%** |
| A 1000-row JSONL stream | 171,832 | 3,984 | **97.7%** |
| One 12 KB line with no newline | 12,000 | 1,288 | **89.3%** |
| A 32-byte command output | 32 | 32 | untouched |

Small outputs are the last row on purpose. Under 2 KiB nothing happens at all,
and above it a view is only substituted when it saves at least 512 bytes and
20% including its own metadata. Scopelet declining to act is a normal outcome.
[Benchmark reproduction](bench/README.md) ·
[what is verified](docs/verification.md).

### Faster

<!-- speed-table:start -->
Median over 30 runs, cold application cache, macOS arm64, Scopelet
0.5.1, measured by `bench/publish.py` on the binary that shipped.

| Operation | Median |
| --- | ---: |
| Compress a 136 KB log | **5 ms** |
| Compress a 3 MB log | **15 ms** |
| Repository query over 400 files | **57 ms** |
| Compress a 32 MiB stream | **62 ms** |
<!-- speed-table:end -->

These figures are replaced on every release: `bench/publish.py` measures the
new binary, writes the two reports under `bench/results/` and rewrites this
table. No older version's numbers are kept anywhere in the repository.

Every stored item is a synced temporary renamed into place, and every read is
checked against its content hash, so an interrupted write yields a missing or
rejected item, never a wrong one. Sending the same output twice reuses the
stored original by address instead of reading and re-hashing it.

> [!IMPORTANT]
> **Bytes are not tokens, and none of the numbers above is a bill.** Earlier
> live-agent campaigns found no functional regression but could not measure a
> token saving above their own run-to-run noise, and they ran against binaries
> that no longer ship, so they were removed rather than kept as stale evidence.
> Whole-session cost depends on your host, model and task. Measure your own
> setup before you tell anyone a percentage.
> [What is verified](docs/verification.md) ·
> [benchmark reproduction](bench/README.md).

## Nothing is lost

Every view links the artifact that holds the original bytes. Reading them back
does not involve the model.

```sh
scopelet expand artifact:HASH --find 'Received:' --context 3
scopelet expand artifact:HASH --manifest
scopelet expand blob:HASH --start 240 --end 260
scopelet expand blob:HASH --raw > original.log
```

Artifact IDs hash the complete dataset and blob IDs hash the original bytes, so
recovery is byte-exact or it fails loudly. `--find` searches the saved
originals, including evidence a query had already filtered out. A partial view
cannot prove absence or an exhaustive count: recover the exact bytes before
editing something you only saw a summary of.

## Exact queries

Filtering, grouping and counting a large file locally beats sending the file.

```sh
scopelet query --repo . --find validateToken --context 5
scopelet query --file events.jsonl --format jsonl \
  --filter /status --equals '"failed"' --group /suite --output compact
scopelet run --auto -- npm test
```

For a standalone `scopelet` command, install with **Rust 1.88+**:

```sh
cargo install --git https://github.com/maxgfr/scopelet --tag v0.5.1 --locked
```

Without Cargo, replace `scopelet` with
`node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs"`. Keep native tools
for small known files. Default JSON views use 16 KiB; `--mode ultra` uses 4 KiB
and may abridge text. Budgets are bytes, not token estimates.
[Query reference](skills/scopelet/references/queries.md).

Optional [codeindex](https://github.com/maxgfr/codeindex) and
[webindex](https://github.com/maxgfr/webindex) adapters add code relationships
and document extraction. Files, logs, JSON and repository queries need neither.

## What gets compressed

Codex hooks wrap recognized noninteractive shell commands, including tests,
builds, Python scripts and simple eligible `&&` chains. Claude Code hooks
replace Bash output when the host supplies a supported result shape. The
OpenCode plugin replaces the `bash` tool's output from `tool.execute.after`
under the same thresholds. Other tools, unsupported shell syntax, existing
output wrappers and outputs the host has already persisted all pass through
untouched. Small automatic outputs skip
cache setup entirely, and Codex leaves a plain `cat` of a known regular file up
to 2 KiB native.

Inside a view, `compact-v3` reads each line the way a terminal would, dropping
colour codes and keeping only the final state of a progress bar. Then:

| Notation | Meaning |
| --- | --- |
| `input:42 text` | line 42, shown as displayed |
| `input:10-14 repeat=5 text` | lines 10 to 14 are all this line |
| `input:4 similar=480 last=492 text` | 480 lines share this shape, first at 4, last at 492 |
| `input:7 text_truncated bytes=12000 prefix…` | one line too big for the budget, cut on a character boundary |
| `input:40-44` then indented lines | lines 40 to 44 verbatim, one space of indent each |

Line numbers are always absolute positions in the original. Markers are
metadata: they are never bytes claimed to appear in your output. Selection
keeps the first and last line, then the first occurrence of each distinct
diagnostic, then repeats, warnings and the lines around a failure, filling
from both ends so a final summary survives a flood of errors.
`--compact-version 1|2` still produce the earlier formats byte for byte.
[Contracts and limits](docs/design.md).

## Response style

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
reasoning effort. Shorter wording does not guarantee a cheaper whole session,
and shortening a reply never justifies skipping verification. Saved documents
use normal prose.

## Upgrade or remove

```sh
npx skills update scopelet --global -y
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" install --agent all
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" doctor
```

Reinstalling updates the pinned hook binary and preserves unrelated hooks.
Restart current sessions and review changed hooks in Codex. Update
project-local skill copies too if you use them; they can shadow the global
installation.

Remove hooks before removing the skill:

```sh
node "$HOME/.agents/skills/scopelet/scripts/scopelet.mjs" uninstall --agent all
npx skills remove scopelet --global -a codex claude-code opencode -y
```

Uninstall preserves configuration backups and cached originals. Clean old
artifacts separately with `clean --older-days 7`; references expire when their
snapshots are removed.
[Paths, backups and cleanup](skills/scopelet/references/setup.md).

## License

MIT · [Issues and support](https://github.com/maxgfr/scopelet/issues)
