# Benchmarks

Two offline probes measure the binary that ships. Neither calls a model, needs
an API key, or talks to the network. Both refuse to overwrite an existing output
directory: completed evidence is never replaced in place.

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

## Tokens: what does each grammar cost?

`tokens.py` counts what the byte probes only estimate: the tokens a view costs
the model. It compresses every content fixture, and a cargo run over twelve
test binaries, in compact-v4, in compact-v5 and in v5 with each part of its
grammar alone (`SCOPELET_V5_GRAMMAR=labels`, `header` or `tables`), then counts
the raw output and each view with `POST /v1/messages/count_tokens`, the free
and exact count of the model's tokenizer. Counts are net of the message frame
(a message holding `.`, less its one token: the API refuses an empty one). It
refuses to run without `--live`, reads the key only from `ANTHROPIC_API_KEY`,
and a count that fails is `null` in the report with its error, never zero.

```sh
cargo build --release --locked
ANTHROPIC_API_KEY=... python3 bench/tokens.py --live   # bench/results/tokens.json
```

On the 24 cases (`results/tokens.json`), compact-v5 costs 6.2% fewer view
tokens than v4 for `claude-opus-5-5` and `claude-haiku-5-5` alike (they share
a tokenizer here), and no case costs more: labels without `input:` save 3.4%,
the short header 1.0%, CSV tables 1.8% (25% on a 1000-row JSON array). Labels
matter most on line-heavy views (a `log -p` 8.5%, `kubectl` JSON 9.2%).
Dropping the explanation of cargo `totals` would save 14 of 926 tokens on the
workspace run, so it stays. `test_tokens.py` checks the counting, the summary
and the `--live` guard with a stand-in tokenizer.

## Live sessions: does a session cost fewer tokens?

`live.py` is the only benchmark that calls a model, and it refuses to run
without `--live`: each session is a real, billed `claude -p` run. It answers
the question the byte probes cannot: with the same task, does a session with
Scopelet read fewer tokens or make fewer calls, and does it still get the task
right?

Twelve synthetic tasks live in `fixtures/live/tasks.json`, one per output
profile, each built deterministically by `live.py`:

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

Each task is checked on the result, not on the wording: tests must pass with
the test file identical to a fresh build (run with a private bytecode cache),
an answer must contain the exact id, path, `path:line`, subject or count.
`test_live.py` checks that each task has one right answer, which its check
accepts and a near miss does not.

Each arm runs every task in a fresh copy of its repository with its own
temporary `CLAUDE_CONFIG_DIR` and Scopelet config and cache. `off` installs
nothing; any other arm installs its binary with `scopelet install --agent
claude` and pins its compact version. Repetitions are paired: every round runs
every arm on every task, and the arm order rotates between rounds. `HOME` is
kept, because Claude Code's OAuth credentials live in the login keychain;
`--isolate-home` also replaces it when an API key is configured instead.

```sh
cargo build --release --locked
python3 bench/live.py --live --runs 3 --arm off \
  --arm v3=/path/to/scopelet-0.7.2@3 --arm v4=target/release/scopelet@4
```

An arm is `off` or `NAME=BINARY@VERSION[:PARTS]`; `PARTS` is passed as
`SCOPELET_V5_GRAMMAR`, so one campaign can compare v5 with its grammar alone
(`@5:labels,header,tables`) and v5 whole (`@5`).

Each session records input, cache-creation, cache-read and output tokens,
turns, tool calls by name, cost and the models actually served (from
`modelUsage` and each message) from the stream-json result; a figure the
host did not report is `null`, never zero. `--transcripts DIR` keeps each
session's raw stream-json, gzipped as `ROUND-TASK-ARM.jsonl.gz`, which
`nap.py` replays. A session that fails, times out or answers wrong stays in
the denominator. The report goes to `bench/results/live-<UTC time>.json` with
every record, per-task and overall medians and:

- `cost_per_success`: the billed cost of every run, failures included, over
  its successes (runs of unknown cost are left out of both and counted);
- `paired`, against `--baseline` (v4 by default): per (task, round) pair, the
  deltas of cost, total input and turns; their means; the delta of cost per
  success; each with a 95% interval from 10,000 bootstrap draws over tasks
  (a task's pairs stay together; fixed seed, standard library only).

Cost per success, not tokens, is the measure: in Claude Code the prompt
cache is most of the bill and tool outputs a few percent, so a turn more
costs more than a smaller view saves. `publish.py` replaces `bench/results/` on a release, live
reports included: a report describes the binaries it names, recorded by hash.
`test_live.py` checks the parsing, the checks, the fixtures and the `--live`
guard from recorded transcripts, without calling a model.

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

