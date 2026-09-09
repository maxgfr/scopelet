# Verification

Scopelet passes its correctness checks and reduces session tokens on some tested
workloads. **It does not provide a universal token reduction.** Several small
code tasks became more expensive. Use it selectively; ultra deliberately loses
visible detail and requires recovery when that detail matters.

## Correctness and distribution

- 52 Rust integration tests: queries, exact byte recovery, freshness, malformed
  input, path/span validation, budgets, cache cleanup, capture throughput and
  process behavior.
- 18 offline benchmark-harness tests, including independent grading, cache usage,
  tool-adoption false positives and immutable fixtures.
- 4 launcher tests, passing on Node 18.20.8 and the local Node 26.8.1 runtime.
- Real codeindex 2.29.1/2.30.0 and webindex 1.18.10/1.19.4 adapter checks.
- Rust 1.88 minimum checked in CI; formatting and Clippy pass.
- [Linux and macOS CI](https://github.com/maxgfr/scopelet/actions/workflows/ci.yml) pass. All four release targets ran their own `--version` and
  offline `bench`: Linux x64/ARM64 and macOS Intel/ARM64.
- Installation using `npx skills add maxgfr/scopelet -a codex claude-code --copy -y`
  created both agent bundles; both installed launchers passed the offline check.

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

## Live agent measurements

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
