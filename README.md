# Scopelet

Compute before you send. A Rust CLI and a small skill for **Codex and Claude Code**
that select, filter and aggregate evidence locally before it enters model context.

## Latest results: automatic mode on Codex Luna

**52/52 sessions passed:** 48 comparative sessions plus four final-binary checks,
including Codex code mode. The comparison used `gpt-5.6-luna`, low effort,
three synthetic tasks and four repetitions per arm, without invoking the skill.

| Task | Default vs native | Caveman vs native | RTK vs native |
| --- | ---: | ---: | ---: |
| Small edit | +0.4% | -7.0% | +34.1% |
| Noisy command | **-40.7%** | **-41.5%** | -29.5% |
| JSONL aggregation | -16.0% | +1.9% | +10.4% |
| Combined task mix | **-23.4%** | **-21.5%** | -2.4% |

These are whole-session logical input plus output tokens, including cached
input, **not billed cost or a universal savings guarantee**. The compressor
activated on noisy commands; the other tasks' differences combine concise
instructions with model variability. Caveman does not consistently beat default.
RTK used its documented Codex awareness instructions. Headroom is unmeasured
because the available adapter did not route Codex requests. No Claude Code model
calls were made in this campaign.

See [full results, variance and limitations](docs/luna-auto-2026-09-09.md) and
[per-session measurements](bench/results/luna-auto-2026-09-09.json).

## Installation

```sh
npx skills add maxgfr/scopelet -a codex claude-code -y
```

Invoke **`/scopelet` in Claude Code** or **`$scopelet` in Codex**, followed by your
task. For tiny known-file edits, use native tools without activating the skill.
Add **“ultra for this session”** to opt into aggressive display limits. The skill's Node 18+ launcher downloads
a pinned, SHA-256 checked release for macOS/Linux, Intel or ARM. Linux releases target Ubuntu 24.04 or compatible glibc environments. Native Windows
is not currently supported. Manual use needs no proxy, model API key or hook.
For automatic operation without skill invocation, enable the hooks below.

For a standalone CLI (Rust 1.88+):

```sh
cargo install --git https://github.com/maxgfr/scopelet --tag v0.2.0 --locked
scopelet doctor
scopelet query --repo . --find validateToken --context 5
scopelet run -- npm test
```

## Automatic mode

After installing the new binary, enable automatic compression once:

```sh
scopelet install --agent all
scopelet mode default
```

Hooks then operate without invoking the skill. `scopelet mode caveman` selects
minimal telegraphic replies; `scopelet mode off` disables automatic intervention.
Codex may request hook trust through `/hooks`; restart current sessions after
installation. See [host coverage and removal](skills/scopelet/references/setup.md#automatic-installation-and-modes).

The compressor first factors repeated JSON column names while keeping all
values, then selects whole evidence units when needed. Small outputs stay
byte-exact. Omissions are explicit and original bytes remain recoverable.

## Query and recovery

Compose operations to answer a question without sending the whole input:

```sh
scopelet query --spec - <<'JSON'
{"version":1,"source":{"type":"file","path":"events.jsonl","format":"jsonl"},"operations":[{"op":"filter","pointer":"/status","equals":"failed"},{"op":"group","pointer":"/suite"}]}
JSON
```

This returns exact counts by suite, source snapshots and coverage metadata.
Search, JSON Pointer projection, filtering, counting, grouping, deduplication,
keyword ranking and exact line reads share the same pipeline. Small known files
usually deserve a direct read; a capable `rg`/`jq`/Python workflow can be cheaper.

| Mode | Default view | Behavior |
|---|---:|---|
| default | 16 KiB | Keeps selected passages and JSON records exact; pages whole records |
| ultra | 4 KiB | Also abridges large text passages, with explicit omitted-line counts |

Budgets are **bytes, not token estimates**. Both modes can omit results from the
visible page. Check `scan_complete` and `display_complete`; a partial view cannot
prove absence. The full result and original bytes remain in local, hashed
artifacts. Expand a result, recover an exact source span, or redirect original
bytes to a file:

```sh
scopelet expand artifact:HASH --offset 10
scopelet expand artifact:HASH --manifest
scopelet expand blob:HASH --start 30 --end 70
scopelet expand blob:HASH --raw > original.bin
```

An oversized record reports `blocked_record` instead of repeating a page forever.
Querying a saved artifact checks local source hashes; explicit expansion still
recovers the immutable old snapshot. Cache data persists until explicit cleanup
with `scopelet clean --older-days 7`.

Optional adapters reuse existing tools:

```sh
npm install -g @maxgfr/codeindex@2.30.0
brew install maxgfr/tap/webindex
```

**codeindex** supplies definitions, callers and impact; **webindex** extracts
web pages and local documents. Native repository/file/log operations need neither.
Adapter results disclose their syntactic or extraction limits. See the complete
[query reference](skills/scopelet/references/queries.md) and
[installation reference](skills/scopelet/references/setup.md).

The design draws from [Headroom](https://github.com/headroomlabs-ai/headroom),
[Caveman](https://github.com/juliusbrussee/caveman),
[Ponytail](https://github.com/dietrichgebert/ponytail) and
[RTK](https://github.com/rtk-ai/rtk). Recoverable compression already exists;
Scopelet's focus is composing local operations and making coverage explicit.
See the [pinned four-tool comparison](docs/competitive-review-2026-09-09.md),
[scientific review](docs/scientific-review-2026-09-09.md) and
[design contracts](docs/design.md). These projects are references, not bundled
runtime dependencies. There is no additional LLM call in Scopelet's runtime.

## Earlier measurements

The [direct competitor pilot](docs/direct-comparison-2026-09-09.md) contains
**51 completed runs across both agents and three tasks; all passed the external
functional checks**. Scopelet does not win overall: RTK had the lowest observed
usage on noisy commands in Codex, and Headroom in all three Claude tasks.
On the noisy task, Scopelet ultra used 131,587 tokens in Codex versus native
161,394, and 127,807 in Claude versus native 288,348. These are single-run session
measurements, including cached input, not reliable savings guarantees.
Stacking full Caveman with Scopelet did not consistently help. Codex/Headroom
routing was unverified and its three cells are explicitly unmeasured.

The [2026-09-09 comparison with the released 0.1.3](docs/fable-comparison-2026-09-09.md)
repeats the matrix with the competitors in their documented integration modes
(plugin hooks, RTK's PreToolUse hook, the Headroom proxy): **54 runs on Haiku 4.5
at high effort plus 40 completed runs on Fable 5.1, all passing the external
grader**. On Haiku, Scopelet cut the noisy-command session by 51% (default) and
56% (ultra) and JSONL aggregation by 19%, but cost 35% more on the tiny edit.
On Fable it saved nothing, because Claude Code 2.1.266 now persists large tool
outputs itself. Headroom was the stronger general competitor. The decision is
to specialize Scopelet, not to present it as a general token saver.

Use Scopelet for noisy commands whose output the host does not truncate and for
exact aggregation of large structured files. Keep direct tools for tiny files
and existing RTK wrappers for commands they already handle. Read the
[verification summary](docs/verification.md) and
[full reproduction instructions](bench/README.md): instructions, extra calls
and recovery can cost more tokens than they save. The [24-run skill follow-up](docs/skill-followup-2026-09-09.md)
records both repetitions and the final cache-exclusion fix.

## Verification

To reproduce checks and the small agent experiment:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 scripts/check_skill.py
npm ci --ignore-scripts
npm test
python3 -m unittest discover -s bench -p 'test_*.py'
cargo run --locked -- bench                      # offline bytes + correctness
cargo build --release --locked
python3 bench/run.py --binary target/release/scopelet --dry-run
python3 bench/run.py --binary target/release/scopelet --live  # calls installed agents
python3 bench/run.py --binary target/release/scopelet --live --tasks task4 --arms baseline,default,ultra --out bench/runs/commands
```

The live harness compares native baseline, the two Scopelet modes and a batched
shell control on synthetic, independently graded tasks. It records model-reported
usage, cache accounting, tool adoption, failures, fixture hashes and timings.
Raw runs stay local under `bench/runs/`; publish only inspected summaries.


## Automatic releases

Every push to `main` runs verification before semantic-release publishes the
macOS/Linux binaries, checksums and installable skill. Conventional feature and
breaking-change commits produce minor and major versions; all other commits,
including ordinary messages and documentation, produce at least a patch.
A push containing several commits produces one release covering those commits.
The generated version commit uses `[skip ci]` to avoid a release loop.

The release workflow resolves the version before its four-platform build and
refuses to publish if the branch moved or the resulting version changed. The
release commit synchronizes Cargo, the skill and its pinned launcher, so the
tag's sources reproduce the released binary version. `GITHUB_TOKEN` needs
contents write permission, and branch rules must permit the workflow's version
commit. No npm package or crates.io package is published.

MIT. [Issues and support](https://github.com/maxgfr/scopelet/issues).
