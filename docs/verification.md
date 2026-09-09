# Verification

Scopelet passes its correctness checks and reduces session tokens on some tested
workloads. **It does not provide a universal token reduction.** Several small
code tasks became more expensive. Use it selectively; ultra deliberately loses
visible detail and requires recovery when that detail matters.

## Correctness and distribution

- 74 Rust integration tests: queries, exact byte recovery, freshness, malformed
  input, path/span validation, budgets, cache cleanup, capture throughput and
  process behavior.
- 65 offline benchmark-harness tests, including independent grading, cache usage,
  tool-adoption false positives and immutable fixtures.
- 4 launcher tests and 5 release automation tests pass locally. The launcher
  also passed on Node 18.20.8 before this change; release tooling requires Node 24.10+.
- Real codeindex 2.29.1/2.30.0 and webindex 1.18.10/1.19.4 adapter checks.
- Rust 1.88 minimum checked in CI; formatting and Clippy pass.
- For the published version, [Linux and macOS CI](https://github.com/maxgfr/scopelet/actions/workflows/ci.yml) pass. All four release targets ran their own `--version` and
  offline `bench`: Linux x64/ARM64 and macOS Intel/ARM64.
- Installation using `npx skills add maxgfr/scopelet -a codex claude-code --copy -y`
  created both agent bundles; both installed launchers passed the offline check.

## Installed automatic hooks with Fable 5.1

The [installed 0.2.1 smoke evidence](../bench/results/fable-installed-0.2.1-2026-09-09.json)
records two Claude Code 2.1.266 sessions requesting `claude-fable-5-1`, high
effort, with real user hooks and no skill invocation. Both saved worker fixes
pass external grading and execute the failing check before the passing check.
The reduced fixture initially failed a harness hash check: the grader used the
full fixture's hash despite receiving the reduced fixture's frozen hash. That
initial failure remains in the report; regrading the saved workspace after the
harness correction required no model call. Regression coverage rejects both an
incorrect worker and subsequent fixture tampering.

The inline successful result reaches Fable as 3,922 bytes instead of 10,634,
including the final success line and omission footer. Recovery returns the
host-supplied capture; Claude already removed the process's final newline.
Failure events retain native output. Default-mode final replies were verbose
under the user's existing instructions. No native control or repetitions were
run, so these checks cannot establish session token savings.

The large-output trace exposed a host interaction: Claude supplies
`persistedOutputPath` and `persistedOutputSize` before rendering its 2 KiB
preview. Recompressing that stdout caused the host to truncate the compact
view, hiding its final lines and omission footer. The adapter now skips those
persisted envelopes before opening its store. This preserves the host's native
recovery path and avoids redundant storage. The replacement shape follows the
[Claude Code hook contract](https://code.claude.com/docs/en/hooks#posttooluse-decision-control).

A third Fable session used the patched release build and a private relocated copy
of the user's hooks in caveman mode. The external grader and failing/passing
sequence pass. Its persisted preview contains no Scopelet compression; a separate
inline sample is compressed with its exact `retries=0` warning and “do not enable
retries” instruction preserved in both the view and final response. Caveman
context is observed, but the final response remains verbose. This is a response
preference, not a strict output-token limit. The three calls report $1.30255025
at the provider's list prices; this is not necessarily the subscription charge.
Raw traces and personal settings remain private; only inspected summaries are
published.

## Earlier verification

The [proportional small-task evaluation](proportional-2026-09-09.md) records
27 additional Luna trials and three Fable calls (one design review, two smokes).
An initial candidate correctness failure remains in the report; the refined
candidate passes its separate checks. Small automatic outputs avoid cache setup,
and known small Codex file reads stay native. Their mechanics are covered by
boundary, exact-stream, exit-status and cache-absence tests. Aggregate small-task
tokens are effectively unchanged from 0.2.2 in the refined exploratory sample.

A [33 MiB command stress test](../bench/results/stress-2026-09-09.json) completed
its final side effect and retained exit code 0. Scopelet saved the first 32 MiB,
reported incomplete capture and returned a 761-byte response under a 1 KiB budget.
Excess output did not kill the command. Unix descendant/cancellation regressions
are covered separately; escaped descendants can survive cleanup but cannot hold
capture open indefinitely.

Independent reviews used Claude Fable 5.1 high, Claude Opus 5 high for design,
and separate Codex agents including GPT-5.6-Luna. Reproductions led to fixes for
transactional JSONL ingestion, multiline search, process drainage, capture limits,
quadratic text copies, oversized pagination, output budgets, cache cleanup,
adapter spans and document freshness. An additional final Opus review timed out
without a result; it is not counted as a successful review. A later Fable/Opus
round led to the capture-throughput, cleanup, ultra-abridgement and manifest
budget fixes recorded in [the 2026-09-09 follow-up](followup-2026-09-09.md),
together with two more live campaigns whose spread is reported there.

## Direct comparison and repeated skill follow-up

The [direct competitor pilot](direct-comparison-2026-09-09.md) records 51 completed
cells across native/concise controls, Scopelet, Caveman, Ponytail, RTK and Headroom.
The [24-run skill follow-up](skill-followup-2026-09-09.md) records two repetitions
per cell and a subsequent cache-search correction. All 75 comparative runs
passed their external functional graders; this is not a broad quality guarantee.
Codex/Headroom routing was unverified and its three planned cells are unmeasured.
The old and new campaigns have different host-context controls and are reported
separately. The final cache fix is validated separately from those frozen runs.

## Independent comparison with the released 0.1.3 (Haiku 4.5, partial Fable 5.1)

The [2026-09-09 comparison](fable-comparison-2026-09-09.md) ran the released
0.1.3 binary and skill against native, a concision instruction and the four
competitors in their documented integration modes (plugins with hooks, RTK's
PreToolUse hook, the Headroom proxy): 54 runs on Haiku 4.5 at high effort and
40 completed runs on Fable 5.1, all passing the external grader. Scopelet cut
the noisy command by 51% to 56% and JSONL aggregation by 19% on Haiku, cost
35% more on the tiny edit, and saved nothing on Fable, where Claude Code
2.1.266 already persists large outputs. The cache markers held in every run.
The decision is to specialize Scopelet rather than present it as a general
saver; Headroom was the stronger general competitor.

After the comparison, a [fresh installed-skill smoke](../bench/results/installed-smoke-claude-2026-09-09.json)
in real Claude Code 2.1.266 (user settings and hooks loaded, Haiku 4.5, high
effort) used the installed `~/.claude/skills/scopelet` launcher, wrapped both
check runs in ultra, fixed the worker behavior, passed the immutable grader,
kept the cache markers and left cached evidence out of a native `rg --hidden`
search. The local CI-equivalent (format, Clippy, 57 Rust tests, offline bench,
skill check, 56 harness tests, 4 launcher tests, adapter checks) passed on the
merged tree.

## Earlier live agent measurements

The [machine-readable results](../bench/results/2026-09-09.json) retain every
campaign, including exploratory and unfavorable runs. They contain fixture,
binary, skill and raw-transcript hashes, per-session usage and acceptance status.
Raw transcripts remain local because they can contain host/account metadata.

The agents were Codex CLI 0.153.4 with GPT-5.6-Luna (low reasoning), and Claude Code
2.1.263 with Haiku 4.5 (`claude-haiku-4-5-20251001`). Workspaces were separate,
synthetic git repositories. Each task had an external pristine grader; the JSONL
source and noisy check script were protected by hash checks. The visible JSONL
shape check did not reveal expected aggregate counts.

Tasks: (1) find/fix cache expiry through its application call path; (2) aggregate
1,000 JSONL records; (3) fix a known small whitespace helper; (4) run a failing
check emitting 1,200 distinct progress lines, repair a worker-limit bug, and rerun
it. Native baselines had ordinary shell/file tools. Task 1 also had a stronger
control that explicitly encouraged batched search/read and local computation.

Totals below are **logical input + reported output tokens**, including cached
input and all turns. They are not dollar estimates. For Codex, input already
includes cached input. For Claude, logical input adds uncached, cache-read and
cache-creation input. Reported output already includes reasoning where supplied;
it is never added again. Missing fields stay unknown.

Codex, frozen binary and skill, one run per cell:

| Task | Native | Default | Ultra | Default change | Ultra change |
|---|---:|---:|---:|---:|---:|
| Find cache bug | 141,357 | 163,249 | 288,700 | +15.5% | +104.2% |
| JSONL aggregation | 151,948 | 74,941 | 117,176 | **−50.7%** | **−22.9%** |
| Small known helper | 128,212 | 282,885 | 132,801 | +120.6% | +3.6% |
| Noisy command | 160,594 | 252,891 | 122,194 | +57.5% | **−23.9%** |

All these tasks passed. Actual Scopelet query/run calls were inspected for the
highlighted comparisons. Task 1's batched native control used 203,890 tokens.
The results support selective use, not mandatory wrapping of every tool call.

Claude's initial campaigns used `--setting-sources ''`, which unintentionally
hid project skills. Those runs test supplied CLI instructions and remain in the
results, but **do not validate project-skill activation**. A direct initialization
probe established that `--setting-sources project` discovers Scopelet without
changing user settings. The harness now uses that option and permits the Skill
tool.

With discovery enabled, the noisy-command comparison used 287,458 tokens natively,
137,315 in default (−52.2%), and 122,135 in ultra (−57.5%); all passed and both
Scopelet arms executed the wrapper twice. On JSONL, Haiku ignored the requested
skill/CLI despite its presence. Its default/ultra totals of 463,079/171,854 versus
216,653 native are therefore **adoption failures**, not evidence for Scopelet's
JSON performance. Discovery alone does not establish activation.

The final Claude trial invoked the skill natively with `/scopelet`, following the
[Claude Code skill interface](https://code.claude.com/docs/en/skills). On the same
noisy-command task:

| Claude, native skill invocation | Total tokens | Change vs native tools | Grade |
|---|---:|---:|---|
| Native tools | 174,168 | — | pass |
| `/scopelet`, default | 141,086 | **−19.0%** | pass |
| `/scopelet`, ultra | 127,439 | **−26.8%** | pass |

Both treatment sessions executed Scopelet twice. The native command expands the
skill before the model works; a separate Skill tool event is not required. An
initial activation probe also caught Haiku trying to run a native binary through
Node. The entrypoint now explicitly distinguishes a native `SCOPELET_BIN` from
the Node launcher.

All **55/55 sessions** across the five retained campaigns passed functional
grading, including 20 exploratory sessions. This does not convert ignored skill
instructions into successful adoption. The final 3-run native-invocation trial
is the Claude skill comparison; earlier discovery/CLI trials remain diagnostic.

The first 20-run campaign was exploratory: binary/skill files changed while it
ran. Later campaigns freeze both before starting. Initially, the adoption counter
also mistook skill-file paths for invocations; published accounting was recomputed
from retained tool events using command-position parsing, excluding help, prose,
comments and heredoc bodies. It remains a conservative shell parser, with manual
checks for highlighted results.

These are single samples in a host with preinstalled skill context and a fixed
arm order. Model variation, instruction overhead, cache behavior and instruction
adherence can dominate small tasks. No significance, general quality equivalence,
quota reduction or universal saving is established. The project includes the
harness so these claims can be tested on other workloads and repeated sessions.

## Published-release installation

[Version 0.1.0](https://github.com/maxgfr/scopelet/releases/tag/v0.1.0) was published
by the gated release workflow. Fresh launcher setup downloaded the macOS ARM64
release and verified its checksum. The globally installed Codex and Claude skills
then each executed that published binary through their Node launcher and saved
the independently checked answer `{"count":3}`. Both exited 0. These are two
additional installation smoke sessions, separate from the 55 measurement runs.
[Their accounting and binary hash](../bench/results/release-smoke-2026-09-09.json)
are retained. The agent subprocesses used a workspace-local `SCOPELET_CACHE_DIR`
for sandbox-compatible result storage and the default verified binary cache.

## Repeated noisy-command task

The later four runs also passed external grading. Together with the eight
scheduled runs, there are 67 retained measurement sessions across these and the
original campaigns; they are not 67 identical or statistically independent trials.

| Campaign | Codex native | Codex ultra | Change | Claude native | Claude ultra | Change |
|---|---:|---:|---:|---:|---:|---:|
| Original noisy task / native Claude skill | 160,594 | 122,194 | −23.9% | 174,168 | 127,439 | −26.8% |
| Scheduled A | 165,718 | 141,153 | −14.8% | 257,009 | 126,993 | −50.6% |
| Scheduled B | 229,885 | 222,757 | −3.1% | 258,265 | 155,259 | −39.9% |
| Morning recheck | 210,159 | 250,219 | **+19.1%** | 259,153 | 127,668 | **−50.7%** |

Rows span different frozen binaries and skill revisions; the original Codex and
Claude results came from separate campaigns. Do not pool them as estimates from
one fixed configuration. The direction changes for Codex, so a reliable saving
has not been demonstrated there. Claude improved on this fixture in these trials;
that does not establish gains on other workloads or universal noninferiority.

[Scheduled accounting](../bench/results/followup-2026-09-09.json) and
[morning accounting](../bench/results/recheck-2026-09-09.json) preserve every run,
model-reported usage, adoption and frozen-input hashes. All treatment sessions
used the CLI. The latest Codex run invoked Scopelet three times, including a
literal search for `worker_count|limit` that found no match and led to a native
`rg` retry. The two command captures were used correctly. The skill and CLI help
now explicitly say that `--find` is literal and can be repeated for alternatives;
this instruction clarification has not been evaluated for a token-saving effect.

The morning binary (`79a34ca2…`) was frozen before the final reserved-temp cleanup
fix; the skill was frozen before that literal-search clarification. Both differences
are independently verified locally, and neither is represented as having been
live-benchmarked by this campaign. Host-global skills were still visible to Codex
and loaded in the treatment; this remains an attribution limitation.


Version 0.1.1 passed the native-target test/release workflow on all four targets.
The refreshed global skill bundles matched the repository byte for byte. Two
new real-agent installation smokes each ran the downloaded 0.1.1 launcher,
queried JSONL and saved the correct count. Its macOS ARM64 SHA-256 matched the
published release manifest. [Smoke accounting](../bench/results/release-smoke-0.1.1.json)
is separate from the 67 measurement sessions above.

## Published 0.1.3 installation check

[Release 0.1.3](https://github.com/maxgfr/scopelet/releases/tag/v0.1.3) passed
[all four platform builds and packaging](https://github.com/maxgfr/scopelet/actions/runs/34319902344)
and [main CI](https://github.com/maxgfr/scopelet/actions/runs/34319900274).
Both globally installed skill bundles matched all five source files. Their
launchers downloaded/ran 0.1.3; the macOS ARM64 binary matched published SHA-256
`0d2e57b5a9545ea195190e22bfd015cad398988aa970532df8ee3ed180d5a432`.

[Two final installed-agent smokes](../bench/results/release-smoke-0.1.3.json)
used the real installed Codex and Claude Code skills, without copying a project
skill or setting SCOPELET_BIN. Both wrapped the failing and successful checks in
ultra, fixed the worker behavior and passed the immutable external grader.
Cache exclusion markers existed and native `rg --hidden` did not re-ingest cached
evidence. These are final-release functional checks, not a token-saving comparison.
The unreleased 0.1.2 tag remains unchanged after a Linux-only benchmark-test path
assertion failed; 0.1.3 corrects that assertion.

## Automatic mode

The automatic-mode work adds contract tests for byte-exact small outputs,
recoverable diagnostic selection, complete JSON schema factoring, installation
coexistence/idempotence, mode changes and automatic-run failure fallback.
`bench/auto.py` is a separate Luna-only pilot; historical comparisons above are
unchanged. It never calls Claude Code. Results and treatment limits are reported
in the [automatic-mode report](luna-auto-2026-09-09.md): 48/48 primary passes
and 4/4 final-binary smoke passes, including Codex code mode. Default reduces
logical session tokens 23.4% over this task mix; the small edit is +0.4%.

## Installation and subsequent semantic releases

The README setup was exercised through `skills` 1.5.25 in fresh project and global
installations. The global canonical path and Claude discovery link were checked,
then the published 0.2.0 launcher downloaded and verified its binary. Both host
hook configurations, idempotent reinstall, mode changes, preservation of unrelated
hooks, offline bench and uninstall passed in temporary configurations.
[Installation evidence](../bench/results/install-0.2.0-2026-09-09.json).

The published version exposed launcher tests pinned to 0.1.3. Fixtures now read
the Cargo version, forbid unintended network access in offline tests, and run
again after a synthetic version change. Each release matrix also checks the
versioned launcher and installation for both hosts before publication. Semantic
commit headers are required; valid documentation/test/CI commits still produce
a patch, while features and breaking changes retain minor/major semantics.
