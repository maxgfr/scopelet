# Audit of Scopelet 1.2.5

Scope: the whole repository at `a4c0ccc` (release 1.2.5): the Rust core and
CLI (`src/`, 13,000 lines), its 20 integration test files, the Node launcher,
the release scripts and the benchmark tooling. 30 commits follow it on `main`,
one topic each. Nothing in `.releaserc.cjs`, `package.json`, the lockfiles or
`bench/results/` was changed. The workflows were left alone by the audit
itself; one line in each was changed afterwards, on the maintainer's decision
(see "Release pipeline").

## Summary

1. The codebase is in good shape: a small dependency set with no known Rust
   advisory, contracts written down in `docs/design.md`, and a suite that was
   fully green before any change (244 Rust tests, 79 Python, 10 Node, clippy
   with `-D warnings`, the README figures re-measured by a script).
2. 23 bugs are fixed, each with a test that fails without the fix. The two
   that matter most: the safe-command classifier could be bypassed with an
   abbreviated long option (`sort --out=F`, `git grep --open=vim`), and a
   Claude Code session start could run a `scopelet` file shipped by the
   project when `PATH` holds a relative entry.
3. Six optimizations are measured, all with unchanged output. The largest:
   `go test` output from parallel tests took 1.3 s per 4,000 tests
   (quadratic, past the hook's 5 s timeout on larger runs) and now takes
   19 ms; every command wrapped for Codex returns about 10 ms sooner.
4. One finding came out of the measurements rather than the review: parallel
   Go tests had their failures located at another test's line. It is fixed.
5. What is left is mostly decisions, listed at the end: how the launcher
   trusts its checksum file, advisories in the release tooling's npm tree,
   and a few behaviours whose intent is ambiguous.

## Baseline

Run in a clean worktree before any change, all passing:
`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`cargo test --locked` (244 tests), `cargo run --locked -- bench`,
`python3 scripts/check_skill.py --pack`,
`python3 -m unittest discover -s bench -p 'test_*.py'` (79 tests, 6 skipped
until a release binary exists), `npm test` (10 tests),
`cargo build --release --locked` and `scripts/check_readme.py`.
`cargo audit` reports nothing on the 82 crates of `Cargo.lock`.

At the tip the same commands pass with 252 Rust tests, 88 Python tests and 14
Node tests, as do `scripts/check_install.py`, `scripts/check_adapters.py`
(codeindex 2.31.16 and webindex 1.37.0, newer than the versions CI pins) and
the content probe that the release job runs.

## Release pipeline

Pushing the audit showed that no release could be published any more, whatever
the commit: GitHub retired the `macos-14` runner image in November 2026
([actions/runner-images#13518](https://github.com/actions/runner-images/issues/13518)).
The `test (macos-14)` job of the Release run was cancelled within a second,
before any step, with "The configuration 'macos-14-arm64-us-default' is not
supported", and the version, build and publish jobs were skipped. The Ubuntu
job, which runs the whole suite, and the MSRV job passed on the audit's
commits.

`ci.yml` and `release.yml` asked for `macos-14` (the test matrix, and the
`aarch64-apple-darwin` build that also measures the README speed table). Both
now ask for `macos-15`, next to the `macos-15-intel` leg that was already
there. The published speed figures will come from that image from the next
release on.

## Bugs fixed

Each test was run against the code without the fix and failed, then against
the fix and passed.

### Core and CLI

| Commit | Where | Issue | Fix | Test |
| --- | --- | --- | --- | --- |
| `f9dfb45` | `src/commands.rs` (`has_flag`, Git's unsafe options) | Long options were compared by exact name. `getopt_long` and Git accept any unambiguous prefix, so `… \| sort --out=F` (writes a file), `sort --compress-prog=P` (runs a program), `tail --fol log` (never ends) and `git grep --open=vim` (opens a pager) passed the read-only checks and were wrapped by the Codex hook. `rg --hostname-bin` was not excluded either. | A prefix counts as every option it could shorten; `--hostname-bin` joins `--pre`. `git diff --text` stays recognized. | `tests/profiles.rs`, `options_that_write_run_or_wait_are_never_recognized` |
| `b1979dd` | `src/commands.rs` (`invokes_wrapper`) | Outside the strict shell subset, any word named `scopelet` or `rtk` marked the line as an output wrapper: `rg rtk src/*.rs` or `ls $HOME/.config/scopelet` was never compressed in Claude Code and OpenCode. | The line is split into its commands and only the program each runs is checked. | `tests/profiles.rs`, `command_lines_are_classified_and_wrappers_found_by_words` |
| `89ba16b` | `src/integration.rs` (`path_binary`) | The upgrade check at a Claude Code session start, and `doctor`, joined every `PATH` entry with `scopelet`, including `.` and empty entries. A repository shipping a `scopelet` file had it executed with `--version`. | Only absolute `PATH` entries are considered. | `tests/automatic.rs`, `a_relative_path_entry_never_runs_a_project_binary` |
| `791c623` | `src/events.rs` (`stats`) | `stats --days 18446744073709551615` subtracted the window from the clock unchecked and panicked (an abort in release builds). | `checked_sub`; such a window means the whole journal. | `tests/automatic.rs`, `stats_count_compressions_pass_throughs_and_later_expansions` |
| `d242f00` | `src/pipeline.rs`, `src/query.rs` | `project` keys records by the pointers themselves and `group` falls back to that literal key, but `filter` did not: `project /status` then `filter /status` silently returned nothing. The streamed JSONL aggregate lacked the fallback too, so the same data failed or answered depending on the execution path. | One resolver shared by `project`, `group`, `filter` and the streamed path. | `tests/query_shortcuts.rs`, `project_then_filter_uses_the_projected_key`; `tests/performance_contracts.rs`, `optimized_queries_equal_materialized_datasets` |
| `33a85a1` | `src/model.rs` (`Dataset::incomplete`) | Every unreadable or oversize file pushed the same note again. Sixteen of them filled the note cap and the reasons recorded afterwards (scan stopped at its cap, scan interrupted) were dropped. | A note already present is not added again. | `tests/repo_scan.rs`, `an_incomplete_scan_says_each_reason_once` |
| `1aa564e` | `src/store.rs` (`read_bounded`) | The path was opened before checking it is a regular file. Opening a FIFO blocks until a writer appears and the signal handlers restart the call: `query --file <fifo>` hung until SIGKILL. | The type is checked before the open and again after it. | `tests/contracts.rs`, `a_fifo_source_is_refused_instead_of_blocking` |
| `6c962d6` | `src/store.rs` (`resolve`, `content_hash_name`) | `artifact:<64 CAPITAL hex>` was returned as typed, unlike a bare hash or a prefix: "integrity mismatch" on a case-insensitive filesystem, "not found" elsewhere. Cleanup also removed a foreign file with a capital hex name as a corrupt cache item. | A cache item's name is the lowercase hex the store writes; a reference is lowercased. | `tests/recovery_ids.rs`, `references_resolve_from_hashes_prefixes_and_last`; `tests/independent.rs`, `clean_preserves_unrecognized_temporary_names_in_both_directories` |
| `590ddc9` | `src/store.rs` (`clean_with`) | `last` is published through a temporary in the cache root, where cleanup never looked: one left by a killed write stayed forever. | The root's reserved temporaries are reaped; every other file there is left alone. | `tests/independent.rs`, `clean_tolerates_foreign_files_and_reaps_stale_temporaries` |
| `6dd8221` | `src/commands.rs` (`profile`, `known_tool`) | Only `--watch`, `--pdb` and `--interactive` kept a command native. `jest --watchAll`, `tsc -w`, `gradle test --continuous`/`-t`, `pytest -f`/`--looponfail`, `dotnet watch test` and `playwright test --ui`/`--debug` were wrapped, and the buffered capture shows nothing until its timeout. | Those forms are excluded, per tool where a letter means something else (`jest -w`, `jest -t`, `npm test -w`). | `tests/profiles.rs`, `writing_waiting_or_unknown_commands_are_not_recognized` |
| `1789d57` | `src/main.rs` (`expand`) | The journal line was written before the expansion ran, so an unknown `--source`, a line range on an artifact or an invalid pattern counted as an expansion and raised the rate that flags a budget as tight. | Journaled once the evidence was read. | `tests/automatic.rs`, `a_failed_expansion_is_not_counted_as_one` |
| `d455f3d` | `src/compress.rs` (`failures`) | With `go test -v`, parallel tests print every `=== RUN` first and stream their logs later under `=== CONT`/`=== NAME`. A failure took the first location printed after its own `=== RUN`: on a real run `TestAlpha` and `TestBeta` were both sent to a third test's line. A rerun that panicked could take a neighbour's frame. | A location belongs to the test named by the marker above it, and to the tests that one is nested in. | `tests/compact.rs`, `failures_of_parallel_go_tests_point_at_their_own_log_line` (output captured from Go 1.27) |
| `2160b09` | `src/process.rs` (`capture`) | `scopelet run -- nosuchcommand` failed with `start command (check doctor for missing adapters)`, for any command and without its name. | The error names the program; adapters keep their own hint. | `tests/contracts.rs`, `a_command_that_cannot_start_is_named` |

### Launcher, release scripts, benchmarks

| Commit | Where | Issue | Fix | Test |
| --- | --- | --- | --- | --- |
| `71ab585` | `skills/scopelet/scripts/scopelet.mjs` | Temporaries were named after the process id and created exclusively. Ids repeat (at once in a container): one left by a killed install failed later runs with `EEXIST`. | Random names. | `scripts/launcher.test.mjs`, "a temporary left by a killed install does not block the next one" |
| `223074f` | `scopelet.mjs` | A failed download printed Node's bare `fetch failed`, without the cause or the address. A wrong `SCOPELET_BIN` gave one message for a missing file, a hung binary and another version. An unreadable cache was silently treated as empty. | Each error says what was found. | `scripts/launcher.test.mjs`, "a failed download names the address and the cause", "explicit binary errors say what was found" |
| `b3c6140` | `scripts/prepare_release.py` | The versions documented in `README.md` and `setup.md` were rewritten unchecked: once their wording drifted, a release would ship documenting the previous version. | Each file must name the new version afterwards; nothing is written otherwise. | `scripts/release.test.mjs`, "version synchronization fails when a documented version would be left behind" |
| `60ffe82` | `bench/live.py` (`session`) | A timed-out session recovered its partial stream, then parsed an empty string: zero views and no tools reported for a stream that showed some. | The partial stream is parsed; usage and cost stay unknown. | `bench/test_live.py`, `test_a_timed_out_session_keeps_what_its_stream_showed` |
| `d5b4314` | `bench/live.py` | A transcript was opened exclusively only after its billed session, so reusing a transcripts directory failed once the session was paid, and its record was lost. | Every planned path is checked before round 0. | `bench/test_live.py`, `test_a_campaign_refuses_existing_transcripts_before_any_session` |
| `bb65f0c` | `bench/live.py` (`paired`) | Pairs with an unknown cost left the cost-per-success delta without being counted. | `pairs_known` and `pairs_unknown`, as each mean delta has. | `bench/test_live.py`, `test_arms_are_paired_by_task_and_round` |
| `17d8c04` | `bench/live.py`, `bench/nap.py` | The growing report was rewritten in place after every billed session: an interrupt during the write left truncated JSON. | Temporary file renamed into place. | `bench/test_live.py`, `ReportTests` |
| `3ffa25c` | `bench/publish.py` | New reports were installed before the README markers were checked: missing markers left new reports beside the old table, after both probes had run. | Markers are checked first; zero repetitions refused. | `bench/test_publish.py`, two tests |
| `3579d9b` | `bench/content.py`, `bench/performance.py` | `TimeoutExpired` escaped, so a hung binary ended the probe instead of being recorded as a failure. | A timeout is a run that exited 124. | `bench/test_content.py`, `bench/test_performance.py` |
| `a0bf2b3` | `bench/nap.py` (`bash_action`) | Commands were split on `\|` before their quotes were read (`grep -E 'error\|warn' f` gave the pattern `'error`) and the first non-option word was the pattern (`grep -A 2 X f` gave `2`). Two steps that differed only by their pattern could count as the same action. | Split outside quotes; an option's value is skipped. | `bench/test_nap.py`, `test_a_search_pattern_is_the_pattern_not_a_flag_value_or_half_an_alternation` |

## Optimizations

Measured on macOS arm64 with release builds, the 1.2.5 binary and the new one
run alternately on the same inputs. The machine was not idle, so medians and
minima are both given where they differ. Outputs were compared byte for byte.

| Commit | What | Before | After | How measured |
| --- | --- | ---: | ---: | --- |
| `7d9eb24` | Locating test failures. Each Go failure searched back for its `=== RUN` line, then forward for a location; pytest built a string per section for every summary line. | 1,304 ms for 4,000 parallel Go tests (0.5 MB); 85 ms for 2,000 pytest failures | 19 ms; 22 ms | Minimum of 9 runs. 600 random runner outputs give identical views. A debug-build test bounds growth: 12,000 tests took 220 s before, under 1 s now. |
| `b25e3cc` | Command capture polled for the exit every 10 ms. It now wakes when a stream ends. | `run --auto -- true` 16.4 ms; `cat` of 200 KB 22.2 ms; a shell exiting 3: 20.0 ms | 5.7 ms; 11.3 ms; 13.6 ms (bare `true`: 3.1 ms) | Median of 60 runs; same output and exit status as the bare command. |
| `c0963b9` | `stats` scanned the whole journal for each compression. | 977 ms on 20,000 compressions and 10,000 expansions | 17 ms | Median of 5; identical report. |
| `ea2d278` | Every line of a diff searched all its file sections. | 109 ms for a 2 MB diff of 3,000 files | 27 ms | Minimum of 9; identical view. |
| `f1af3ec` | Paging a blob cloned the whole text and dataset at each bisection step; searching a blob read and hashed it twice. | 30 MB original: default page 83 ms, 1 MiB page 133 ms, `--find` 69 ms | 42 ms, 70 ms, 46 ms | Median of 21; identical output and artifact. |
| `fcbe3f3` | The PostToolUse hook rewrote the session state file on every small result. | 3.66 ms per small result (minimum 2.44) | 3.37 ms (minimum 2.29) | 1,500 runs each. A small gain, on the path every tool call takes. |

No regression on ordinary compression: `bench/performance.py --stress` reports
no output mismatch between the two binaries over its 22 cases, and CPU time
(user + system, minimum of 41 runs) is 40.9 ms against 40.9 for a 32 MiB
stream, 25.0 against 24.1 for 100,000 lines and 12.8 against 12.6 for a 3 MB
log.

Measured and left alone, because the gain would not be visible:

- Relevance anchors make one pass per term: a 6 MB output with 32 prompt terms
  goes through the hook in 60 ms.
- A repository search scans a matching file more than once: 6,000 files
  (10 MB) are searched in 105 ms.
- A wrapped Codex run lists the sessions directory to resolve a 16-character
  key: no measurable cost at 5,000 files, 23 ms at 50,000.
- The launcher re-hashes the cached binary and asks the `scopelet` on `PATH`
  for its version on every call: 2 to 7 ms of a 35 ms call that Node's own
  start dominates.

## Behaviour that changed

All are consequences of the fixes above.

- **Commands that are no longer wrapped or recognized**: abbreviated long
  options of the excluded ones, `rg --hostname-bin`, and the watch, loop and
  UI forms listed above. They run natively, as the contract already said.
- **Commands now compressed** in Claude Code and OpenCode: lines outside the
  strict grammar that merely mention `scopelet` or `rtk`.
- **`failed=` line of Go test views**: parallel tests and reruns get their own
  location; a test that logged nothing gets none where it could get a
  neighbour's.
- **`stats`**: failed expansions are no longer counted, so `expand_rate` can
  be lower than 1.2.5 reported on the same journal.
- **Queries**: `filter` after `project` now matches; a JSONL key spelled like a
  pointer (`"/state"`) is found by `group` and `filter` on the streamed path.
- **Artifact identity of an incomplete scan**: with two or more unreadable
  files the dataset carries one note instead of several, so the same scan
  gets a different artifact ID than under 1.2.5.
- **`query --file <fifo>`** fails with "not a regular file" instead of
  blocking.
- **Session state file**: it is no longer rewritten by small tool results, so
  its modification time is not refreshed by them. `clean --older-days N`
  could remove the state of a session that produced nothing but small outputs
  and no new prompt terms for N days; it would lose its repeat memory and
  anchors, nothing else.
- **Messages**: the error for a command that cannot start; the launcher's
  setup errors.
- **Benchmark reports**: `cost_per_success_delta` gains two fields; a timed-out
  session's record carries the tools and views its stream showed; a hung
  binary appears as exit 124. Strict next-action figures from a future replay
  are not strictly comparable with the stored reports, which were computed
  with the old pattern reading. Stored reports were not touched.
- **`scripts/prepare_release.py`** fails where it used to pass silently.

## Found, not fixed

### Critical

None.

### Important

1. **The launcher's checksum does not protect against a replaced release
   asset.** `scopelet.mjs` takes the expected hash from `SHA256SUMS`,
   downloaded from the same release as the binary. This catches corruption,
   not substitution. Recommendation: have the release write the four hashes
   into the launcher, which is itself distributed through the skill. It needs
   a change of order in `release.yml`, where the version is prepared before
   the binaries exist.
2. **39 advisories in the release tooling's npm tree** (2 high: `undici`,
   `brace-expansion`; `npm audit`). They are development dependencies of
   semantic-release, installed with `--ignore-scripts` and never shipped.
   Recommendation: `npm audit fix` in a pull request of its own, so a release
   that breaks is traced to it. `@semantic-release/github` 12.0.10 and
   `conventional-changelog-conventionalcommits` 10 are available.
3. **`run --auto` answers a program that cannot start with exit 2 and a JSON
   error**, where a shell gives 127 or 126 and "command not found"
   (`src/main.rs`, the `process::capture(...)?` of the automatic run). An
   agent on Codex sees a different failure than without the hook. See the
   first open question.
4. **Installing rewrites the host's `settings.json` with its keys sorted.**
   `serde_json` is built without `preserve_order`, so every install,
   uninstall or mode change reorders the user's file (a backup is kept).
   Enabling the feature also changes how JSON records are serialized, hence
   artifact IDs, so it is a decision, not a fix.
5. **`GOOS` and `GOARCH` are allowed in front of a wrapped command**
   (`safe_env`), yet a foreign value makes `go test` run
   `go_$GOOS_$GOARCH_exec` from `PATH`: the list's own rule is "never one
   that loads or runs a program".
6. **18 compatible crate updates are pending** (`cargo update --dry-run`:
   `clap` 4.6.7, `ignore` 0.4.34, `serde_json` 1.0.154, `libc` 0.2.191 and
   others). Not
   applied: no 1.88 toolchain was available to check the minimum supported
   version that CI enforces.

### Minor

7. `expand --find` on a document artifact probably fails with "saved source
   is not UTF-8" when the original is a PDF: the original bytes and the
   extracted text share one snapshot label (`src/adapters.rs`,
   `src/recovery.rs`). From reading; not reproduced.
8. A repository scan can exceed its 128 MiB cap by one file (up to 32 MiB):
   the cap is tested before the file is added (`src/sources.rs`).
9. The line-offset index (`src/line_index.rs`) re-derives every offset to
   validate what it cached, so it appears to cost more than it saves. From
   reading; removing it is a design change.
10. An empty text source yields a record with `start_line: 1, end_line: 0`.
    It still counts the file, so it was left. See the open questions.
11. Without `-v`, a Go parent test whose subtest failed gets no location,
    while with `-v` it gets the subtest's. With `-v`, a failure is located at
    its first log line, which can be a `t.Log` above the failing assertion.
12. Launcher: musl Linux gets the glibc binary and a bare `ENOENT`; SIGINT
    reaches the child twice from a terminal; `SCOPELET_CACHE_DIR` is
    documented for the cache but the launcher only reads `XDG_CACHE_HOME`; a
    redirect to plain HTTP is not refused; temporaries of a killed install
    are never removed.
13. `release_checksums.py` does not cover `dist/scopelet.skill`; the skill
    archive is not reproducible (timestamps); GitHub Actions are pinned by
    tag, not by commit.
14. Reinstalling under another configuration directory or binary path leaves
    the earlier hook entries, which then run twice; uninstall does not remove
    them. A symlinked `settings.json` blocks installation (deliberate).
15. Two concurrent hooks of one context can lose a state update (documented
    as best effort). A subagent's context never starts a new turn, so it gets
    one loop note for its whole life.
16. When stdout and stderr of one result are identical and large, the second
    becomes "identical to output already shown" in the same result.
17. The OpenCode plugin starts the binary at every model step for guidance
    that only depends on the mode file.
18. `rg -z` starts decompressors; it is recognized as a plain search.
19. `bench/tokens.py` and `bench/nap.py` carry two copies of an API client
    that retry differently; a `Retry-After` given as a date would raise.
    `tokens.py --live` without `--out` always refuses, as its README says.
20. A journal line with a reason unknown to this version is dropped from
    `stats` without a count.
21. Views rank a line as "diagnostic" for the quarter-budget rule when it is
    only a rare line of a folded record (`Tiers`, `diagnostics`). From
    reading; not verified.

### Investigated and dropped

- A table projection reserving too narrow a header (`compress.rs`): not
  reproduced over 120 generated responses. The CSV form it writes is always
  shorter than the JSON form its rows were chosen on, which leaves the room.
- The 250 ms wait after a timeout: deliberate, it is when the process group
  is killed.
- A failed modification-time update making `put` fail on an existing item:
  needs a cache owned by another user; not reproduced.

## Open questions

1. **Exit status when a wrapped program cannot start.** Keep exit 2 with a
   JSON error, or answer as a shell does (127 not found, 126 not executable,
   the message on stderr)? I recommend the shell's convention for `run
   --auto`, since that path stands in for the native command.
2. **Pinned hashes in the launcher.** Is a release asset replaced after
   publication part of the threat model? If so, item 1 above is the fix.
3. **`GOOS`/`GOARCH`.** Remove them from the allowed prefixes (cross-compiled
   `go build` then runs natively), or accept the exception and document it?
4. **Release tooling.** Should `npm audit fix` and `cargo update` go in as
   their own pull requests, with the MSRV job as the check for the second?
5. **Empty files.** Should an empty file be a record at all, and if so with
   which line range?
6. **Key order of host settings.** Is preserving the user's order worth a
   change of artifact serialization (or a second JSON type for settings)?
7. **Go locations.** Should a failure point at its first log line or its
   last, and should a parent take its subtest's location without `-v` too?
8. **The line-offset index.** Keep it for a future cheaper validation, or
   remove it?

## How this was verified

- Every commit that touches Rust was preceded by the whole gate (format,
  clippy, all Rust tests, `scopelet bench`, the skill check, the Python and
  Node suites). Commits that touch only Python or the launcher were preceded
  by the suites that read those files.
- The fix that changes views (`d455f3d`) and the tip were also checked with a
  release build, `scripts/check_readme.py`, the content probe,
  `scripts/check_install.py` and `scripts/check_adapters.py`. The README
  figures and the hashes pinned in `tests/content_gate.rs` did not move.
- Every commit is signed.
