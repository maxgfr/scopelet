# Benchmarks

| Probe | Question | Calls a model | Report |
|---|---|---|---|
| `content.py` | Does the view keep the facts? | no | `results/content.json`, every release |
| `performance.py` | How fast, how much memory? | no | `results/performance.json`, every release |
| `tokens.py` | What does a view cost in tokens, against the raw output and a reference binary? | token counting only (free) | `results/tokens*.json` |
| `live.py` | Does a session cost less per successful task? | yes, billed | `results/live-*.json` |
| `nap.py` | Does a view change the agent's next action? | yes, billed | `results/nap-*.json` |

The two offline probes measure the binary that ships on every release, need no
API key and never talk to the network. The other three refuse to run without
`--live` and read a key only from `ANTHROPIC_API_KEY` (or a Claude Code login
for `live.py`). Every probe refuses to overwrite an existing output: completed
evidence is never replaced in place, and a failed measurement stays in its
report with its error, never as zero.

Scopelet 1.0 has one compact format: compact-v4's selection written with
compact-v5's labels, short header and CSV tables, the error-location rule on
top. The probes compare the binary under test with the raw output and, where
it matters, with a reference binary such as the 0.11.1 release, whose default
was compact-v4; there are no version or grammar switches left to measure.

The reports of 16 October 2026 (`live-20261016-*.json`, `nap-20261016.json`,
`tokens.json`) are kept as they were: they measured the 0.10.0 and 0.11.0
binaries, still selectable by version then, and name each binary by hash
(`bfd3db2f62e3…` for the cost-per-success campaign and its NAP replay,
`36d3d332a060…` for the v4/v5 pairing, `45225dac4122…` for the token probe).
In short: on the twelve live tasks every arm answered every task, and no
arm's cost per successful task differed from compact-v4's beyond noise,
Scopelet off included; compact-v5 showed 6.1% fewer view tokens than v4 on the
0.10.0 fixtures; every compressed view changed the next action on 5 of 8
replayed steps. The details are below.

Earlier live-agent campaigns (Codex Luna, Claude Haiku and Fable pilots against
Caveman, Ponytail, RTK and Headroom, on Scopelet 0.1.x–0.3.x with the manual
skill) were removed once the engine, the default presentation and the
installation model had all changed. Their numbers described binaries that no
longer exist. Whole-session token accounting depends on the host, the model and
the task; measure your own setup before quoting a percentage.

## Content: does the view keep the facts?

`content.py` runs `scopelet compress` on eight realistic outputs: a 136 KB
progress log with two diagnostics at the end, a retry loop hiding one fatal
error, the same log with CRLF endings, a receipt buried in the middle, a
1000-row JSON array, a 1000-row JSONL stream, a 12 KB single line and a 32-byte
output. It adds the real tool outputs in `fixtures/` (cargo, tsc, pytest, go,
Java, Jest, ESLint, Git diffs and logs, kubectl and gh JSON, a service log, rg
with context and two Rust sources), described by `fixtures/manifest.json`:
facts that must be visible, a minimum reduction (`null` means the output must
pass through), and pending facts that a planned change should reveal, which
are reported but not enforced. Each real output is compressed with its
manifest profile (`--profile`), as the hooks would. `tests/content_gate.rs`
reads the same files.
For every fixture it records bytes in and out, cold and warm timings,
whether every fact is visible in the view, and whether the original bytes come
back through the artifact manifest and source blob. It exits non-zero when a
fact or a byte is lost.

```sh
cargo build --release --locked
python3 bench/content.py --scopelet target/release/scopelet --out bench/runs/content
```

`tests/content_gate.rs` mirrors the same fixtures byte for byte and pins them to
the SHA-256 values recorded in `bench/results/content.json`, so a compressor
change that silently drops a diagnostic fails `cargo test`. The README
reduction table is checked against the binary by `scripts/check_readme.py`.

## Performance: how fast, how much memory?

`performance.py` measures one or more binaries on the same synthetic workloads:
a 3 MB log, a 12,000-row JSONL table, an aggregate query over it, a 400-file
repository search, a paged recovery from a stored original and, with
`--stress`, a stream just under the 32 MiB capture limit. Each case runs with a
cold application cache and a warm one, five warmups and thirty measured
repetitions by default. Wall time and peak resident size come from
`/usr/bin/time`. The OS page cache is not flushed.

```sh
python3 bench/performance.py --binary release=target/release/scopelet --stress \
  --out bench/runs/performance
```

To compare a candidate against a released build, pass two arms. They alternate
order every repetition, and every arm must produce identical bytes on every
case: a candidate that changes the output fails the run instead of looking
faster.

```sh
python3 bench/performance.py --binary released=/path/to/scopelet-0.5.0 \
  --binary candidate=target/release/scopelet --stress --out bench/runs/candidate
```

## Publishing a release's figures

`publish.py` runs both probes on one binary, replaces everything under
`bench/results/` with the two fresh reports (`content.json`,
`performance.json`) and rewrites the README speed table between its
`speed-table` markers. Only the shipped version's numbers are kept: an older
version's figures are deleted, not archived.

The release workflow runs it on the macOS arm64 runner against the versioned
release binary, and the release commit carries the two reports and the README
table it produced. Every published version therefore ships with its own
figures, measured on the same class of machine each time. Run it locally to
preview a change; the numbers that count are the release's.

```sh
cargo build --release --locked
python3 bench/publish.py --scopelet target/release/scopelet
python3 -m unittest discover -s bench -p 'test_*.py'
```

Timings are descriptive: other activity on the machine was not controlled, and
only the medians are quoted. `tests/content_gate.rs` reads
`bench/results/content.json`, so a publish that changes a fixture fails the
build until the Rust mirror is updated too.

## Tokens: what does a view cost?

`tokens.py` counts what the byte probes only estimate: the tokens a view costs
the model. It compresses every content fixture, a cargo run over twelve test
binaries, a 23 KB `find .` listing with `node_modules` and a 48 KB source file
as the hooks would (`scopelet compress --profile`), with the binary under test
(`single`) and, given `--baseline BINARY`, with a reference binary in its own
default format (`baseline`), each in its own cache. It then counts the raw
output and each view with `POST /v1/messages/count_tokens`, the free and exact
count of the model's tokenizer. Counts are net of the message frame (a message
holding `.`, less its one token: the API refuses an empty one). The summary,
per model, covers the cases every view counted: `single_vs_raw` and, with a
baseline, `single_vs_baseline` (tokens saved, percent, and the cases where the
view costs more). It refuses to run without `--live`, reads the key only from
`ANTHROPIC_API_KEY`, never overwrites its output, and a count that fails is
`null` in the report with its error, never zero.

```sh
cargo build --release --locked
ANTHROPIC_API_KEY=... python3 bench/tokens.py --live \
  --baseline /path/to/scopelet-0.11.1 --out bench/results/tokens-<date>.json
```

Flags: `--live` (required), `--scopelet BINARY` (default
`target/release/scopelet`), `--baseline BINARY`, `--model MODEL` (repeatable;
default `claude-opus-5-5` and `claude-haiku-5-5`), `--out PATH` (default
`bench/results/tokens.json`, which must not exist).

The kept `results/tokens.json` measured the 0.10.0 grammar parts, before the
format was unified: on its 26 cases compact-v5 cost 11.8% fewer view tokens
than v4 for both models, and no case cost more. Most of that gap was the
listing built for `artifacts` (4,169 tokens in v4, 1,630 in v5), a part 1.0
dropped. On the 24 cases measured for 0.10.0, v5 saved 6.1%: labels without
`input:` 3.4%, the short header 1.0%, CSV tables 1.8% (25% on a 1000-row JSON
array), `artifacts` nothing there and `outline` cost 0.1%. The three parts
that only rewrite v4's selection are the ones 1.0 keeps. Dropping the
explanation of cargo `totals` would save 14 of 926 tokens on the workspace
run, so it stays. `test_tokens.py` checks the counting, the summary and the
`--live` guard with a stand-in tokenizer.

## Live sessions: does a session cost fewer tokens?

`live.py` is the only benchmark that calls a model, and it refuses to run
without `--live`: each session is a real, billed `claude -p` run. It answers
the question the byte probes cannot: with the same task, does a session with
Scopelet read fewer tokens or make fewer calls, and does it still get the task
right?

Sixteen synthetic tasks live in `fixtures/live/tasks.json`, each built
deterministically by `live.py`:

| Task | What the session faces |
|---|---|
| `fix_test` | an off-by-one in a Python module whose 302-test suite prints a long report |
| `find_receipt` | one receipt id in a 20,000-line log |
| `count_failures` | failed events of one suite in a 5,000-line JSONL file |
| `review_diff` | the behavior change in a commit that also rewrites a large `package-lock.json` |
| `search_symbol` | the definition of a function called 1,800 times across 300 files (`rg`) |
| `git_log_find` | the commit, under a misleading subject, that flipped a sign (`git log -p`, 30 commits) |
| `listing_artifacts` | the project's own `.py` files in a tree full of `node_modules`, `.venv`, `build`, `target`, `__pycache__` |
| `large_file_fix` | a bug in one function of a 43 KB module, with tests that must pass |
| `stack_trace` | the deepest `Caused by` of a nested Java failure in a log with handled exceptions |
| `json_api` | open issues with one exact label in 200 minified GitHub-style objects |
| `build_warnings` | the one file with an error among 2,000 compiler warnings |
| `pytest_many` | two failing boundary cases among 600 tests (`unittest -v`; pytest is not assumed) |
| `build_warnings_medium` | the `path:line` of the one error among 300 warnings, each with its own `-->` (18 KB) |
| `test_report_medium` | one failing boundary case among 150 verbose tests, fixed until they pass (11 KB) |
| `grep_context` | the one `charge_card` call of 200 whose total, two lines above, holds a refund (`grep -rn -C2`, 23 KB) |
| `diff_medium` | the one behavior change in an uncommitted `git diff` of reworded docstrings and an equivalent rewrite (16 KB) |

The last four force views: their prompts impose the command as written,
without a pipe or redirection, and each output lies between 2 KB (above every
exact-size threshold) and 30 KB (below the size at which Claude Code saves an
output to a file and shows only a preview, which the hook leaves alone).

Each task is checked on the result, not on the wording: tests must pass with
the test file identical to a fresh build (run with a private bytecode cache),
an answer must contain the exact id, path, `path:line`, subject or count, and
none of the near misses a check lists as `forbidden` (the lookalike file, the
neighbouring line). `test_live.py` checks that each task has one right answer,
which its check accepts and a near miss does not, and that the four
view-forcing tasks print between 2 and 30 KB.

Each arm runs every task in a fresh copy of its repository with its own
temporary `CLAUDE_CONFIG_DIR` and Scopelet config and cache, and no
inherited `SCOPELET_*` setting. `off` installs nothing; any other arm installs
its binary with `scopelet install --agent claude`, which then shows that
binary's own default format. Repetitions are paired: every round runs
every arm on every task, and the arm order rotates between rounds. `HOME` is
kept, because Claude Code's OAuth credentials live in the login keychain;
`--isolate-home` also replaces it when an API key is configured instead.

```sh
cargo build --release --locked
cp target/release/scopelet /tmp/scopelet-single   # frozen for the campaign
python3 bench/live.py --live --runs 3 --model haiku --baseline v4old \
  --arm off --arm v4old=/path/to/scopelet-0.11.1 --arm single=/tmp/scopelet-single \
  --transcripts /tmp/transcripts --out bench/results/live-<date>-single-haiku.json
```

An arm is `off` or `NAME=BINARY`. Flags: `--live` (required), `--arm`
(repeatable; default `off` alone), `--task NAME` (repeatable; default all),
`--runs N` (paired rounds, default 3), `--model` (default `haiku`),
`--baseline NAME` (the arm the others are paired against, default `v4old`),
`--transcripts DIR`, `--timeout SECONDS` (default 900), `--isolate-home` and
`--out PATH` (default `bench/results/live-<UTC time>.json`, which must not
exist).

Each session records input, cache-creation, cache-read and output tokens,
turns, tool calls by name, cost and the models actually served (from
`modelUsage` and each message) from the stream-json result; a figure the
host did not report is `null`, never zero. `views_shown` counts the Scopelet
views and repeat references the model was actually shown: tool results in the
stream with a `[scopelet v5 `, `v4` or `compact-vN` header at a line start
(not the hook's raw response, not a quoted marker or a capture note), so a
campaign can tell whether it exercised the views at all. `--transcripts DIR` keeps each
session's raw stream-json, gzipped as `ROUND-TASK-ARM.jsonl.gz`, which
`nap.py` replays. A session that fails, times out or answers wrong stays in
the denominator. The report goes to `bench/results/live-<UTC time>.json` with
every record, per-task and overall medians and:

- `cost_per_success`: the billed cost of every run, failures included, over
  its successes (runs of unknown cost are left out of both and counted);
- `paired`, against `--baseline` (`v4old` by default): per (task, round) pair, the
  deltas of cost, total input and turns; their means; the delta of cost per
  success; each with a 95% interval from 10,000 bootstrap draws over tasks
  (a task's pairs stay together; fixed seed, standard library only).

Cost per success, not tokens, is the measure: in Claude Code the prompt
cache is most of the bill and tool outputs a few percent, so a turn more
costs more than a smaller view saves.

A release commit takes only `content.json` and `performance.json` from
`publish.py` (which, run locally, empties `bench/results/` first); live,
token and NAP reports stay until replaced on purpose. A report describes the
binaries it names, recorded by hash.
`test_live.py` checks the parsing, the checks, the fixtures and the `--live`
guard from recorded transcripts, without calling a model.

The reports below measured binaries from before 1.0, when the compact
version and v5's parts were still selectable; their arms name those versions.

`results/live-20261012-haiku.json` (Claude Code 2.1.296, Haiku, the first
four tasks, three paired rounds, 36 sessions) compared Scopelet off, 0.7.2's
compact-v3 and compact-v4: every task was done correctly in every arm, and no
session saving rose above run-to-run noise. One extra turn cost about 20,000
input tokens, the fixed context of a session about 37,000, and the paired
median ratio of v4 to off was 1.01 (from 0.50 to 1.53).

`results/live-20261016-v4-v5-haiku.json` pairs compact-v4 with compact-v5 (one
binary, haiku, 3 rounds of the four tasks). Both arms answered 12 of 12
correctly with a median of 3 tool calls, but v5's median total input was
higher (77,714 against 61,035 tokens): `count_failures` fell 17%, `review_diff`
held, `fix_test` and `find_receipt` rose. Paired sessions that took the same
tool path differ by under 0.3% (38,291 and 38,178; 61,124 and 61,037), so the
gap is the model choosing other paths, which v4 also did in its third round;
a 6% smaller view is below what twelve sessions can resolve. By the rule set
before the run (no higher median input or tool calls), v5 stays opt-in and v4
the default. The tasks run Python tests, so no `totals` line was involved.

### Cost per success: twelve tasks, four arms (October 2026)

`results/live-20261016-cps-haiku.json` runs the twelve tasks three times with
four arms of one frozen binary: `off`, `v4`, `v5g` (v5 with its grammar only,
`@5:labels,header,tables`) and `v5` (whole). Every session was served by
`claude-haiku-5-5`, every cost is known, and all 144 answered correctly.

| Arm | Cost | Cost per success | Delta vs v4, 95% interval | Turns | Tool calls |
|---|---:|---:|---|---:|---:|
| off | $0.0799 | $0.002220 | −$0.000114 [−0.000457, +0.000240] | 133 | 97 |
| v4 | $0.0840 | $0.002334 | | 163 | 127 |
| v5g | $0.0779 | $0.002164 | −$0.000170 [−0.000425, +0.000050] | 146 | 110 |
| v5 | $0.0823 | $0.002286 | −$0.000049 [−0.000335, +0.000261] | 160 | 124 |

`results/live-20261016-cps-sonnet.json` repeats v4 against v5g twice with
`claude-sonnet-5-5`: 48 of 48 correct, v5g +$0.00087 per success
[−0.00074, +0.00271], no difference either way.

The rule set before the runs made an arm the default only if, in haiku, the
upper bound of its cost-per-success delta against v4 was below zero, with as
many successes, a mean NAP at most 2 points under v4's, no more median turns,
and no significant loss in sonnet. No arm passes the first condition, so v4
stays the default and v5 stays opt-in with all its parts.

What the sessions show:

- A turn costs the same in every arm (median input per turn 19,324 tokens
  without Scopelet, 19,635 with v4): the session guidance is a few hundred
  tokens. Cost follows turns, and `off` took significantly fewer than v4
  (−0.83 per session [−1.39, −0.28]).
- The model saw few views: over 36 sessions per arm, 5 (v4) to 10 (v5g)
  results were Scopelet views (counted by hand then; `views_shown` now
  records it per session). It narrows most commands itself (`grep`,
  `head`, `| tail`), and 8 to 11 results per arm were over Claude Code's
  inline limit: Claude saves those to a file and shows a 2 KB preview, which
  the hook leaves alone in every arm (see `docs/design.md`). A failing test at
  the end of a long run is not in that preview (`pytest_many`).
- After a v5 listing folded by `artifacts`, the model wrote that the output
  "looks truncated and decorated" and counted again (`listing_artifacts`: 14
  tool calls in v5 against 4 without Scopelet).
- In `build_warnings`, a view under the 4 KB test budget keeps the
  `error[E0308]` line but not the `--> src/ledger/settle.rs:88:17` below it:
  that location ranks with the first occurrence of every distinct warning and
  loses to them. v4 took 13 tool calls there, `off` 6. In 1.0 a location
  right below the first diagnostic of its template ranks with it
  (`build_warnings_medium` forces that view).
- In this environment `find` resolves to `fd` (sessions keep the user's
  shell), so every arm met `fd` errors and retried: noise shared by all arms.


## Next action: does a view change what the agent does next?

`nap.py` measures next-action preservation, after CoACT, without a trained
model. It replays the `off` transcripts `live.py --transcripts` kept: at each
Bash or Grep result, the conversation up to that result is rebuilt (the task
prompt, assistant turns without their thinking blocks, earlier results as
recorded) and the result is shown in up to three forms, each view produced by
the real hook (`scopelet hook claude` on a simulated `PostToolUse`, after the
task prompt as `UserPromptSubmit`): `raw`, the view of the reference binary
given with `--baseline` (`baseline`, such as the 0.11.1 release) and the view
of the binary under test (`single`, `--scopelet`). Steps where no condition
changed the output (below the profile's
exact-size threshold) are skipped. Each form goes to `POST /v1/messages` with
a short agent prompt and Bash, Read, Edit, Grep, Glob and Write tools, the
shared prefix cached; sampling cannot be set on current models, so each form
is drawn three times.

The next action is decomposed into its tool, program, paths, search pattern
and line range, or a final answer judged by the task's own check. A form's
NAP at a step is the share of its draws equal to raw's most frequent action;
raw's own share is the noise floor. Input tokens are the API's exact usage. A
failed call is `null` with its error, never zero. It refuses to run without
`--live`, reads the key only from `ANTHROPIC_API_KEY` and never writes into
an existing output directory. `test_nap.py` checks the rebuilding, the
decomposition, the scoring and the guards with a stand-in client.

```sh
python3 bench/live.py --live --arm off ... --transcripts /tmp/tx
ANTHROPIC_API_KEY=... python3 bench/nap.py --live --transcripts /tmp/tx \
  --scopelet /tmp/scopelet-single --baseline /path/to/scopelet-0.11.1 --out /tmp/nap
```

Flags: `--live` (required), `--transcripts DIR` (required), `--scopelet
BINARY` (default `target/release/scopelet`), `--baseline BINARY`, `--arm NAME`
(whose transcripts to replay, default `off`), `--model` (default
`claude-haiku-5-5`), `--draws N` (default 3), `--workers N` (default 4) and
`--out DIR` (required, must not exist). The report names each condition's
binary by hash.

`results/nap-20261016.json` (before 1.0: four forms, raw, compact-v4, v5 with
its grammar only and v5 whole) replays the 36 `off` sessions of the haiku
campaign with `claude-haiku-5-5`. Of their large results, 8 were saved by
Claude Code (persisted; every arm shows the same preview) and one Grep was cut
by the host; 8 steps remain, 3 draws each, no failed call:

| Form | View bytes | Mean input tokens | NAP | Coarse NAP |
|---|---:|---:|---:|---:|
| raw | 161,595 | 11,799 | 0.92 | 1.00 |
| v4 | 43,538 | 4,098 | 0.38 | 0.46 |
| v5 grammar only | 39,578 | 3,927 | 0.38 | 0.46 |
| v5 | 20,372 | 2,487 | 0.38 | 0.42 |

Every form kept the next action of `fix_test` (3 steps: the failure is in
every view). On `listing_artifacts` (3 steps) and `git_log_find` (1), the raw
output let the model answer at once; with any view it ran one more command
first. v5 changes the next action no more often than v4 (condition (c) holds),
but every compressed view changes it on 5 of 8 steps, consistent with the extra
turns of the live sessions. Eight steps are few: this measures a
direction, not a rate.
