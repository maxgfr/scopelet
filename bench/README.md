# Agent comparisons

`run.py` provides synthetic fixtures, immutable external graders, native controls,
provider usage normalization and process-group timeouts. `compare.py` adds full
competitor skills, manual RTK wrapping and an explicit Headroom proxy adapter.
No provider calls occur without `--live`. Output directories must be empty:
completed evidence is never overwritten.

Read the [declared protocol](../docs/direct-comparison-plan.md) and
[completed pilot](../docs/direct-comparison-2026-09-09.md) before interpreting results.

## Dependencies and frozen inputs

Install/authenticate Codex and Claude Code normally. The recorded pilot used
Codex 0.153.4 with GPT-5.6-Luna (low) and Claude Code 2.1.263 with Haiku 4.5.
The Codex model is configured in `run.py`; the Claude model and effort are
`compare.py` options (`--model`, default `claude-fable-5-1`; `--effort`, default
`high`; `--claude-bin` to pin the executable). Compare within an agent/model,
not across the two. Host instructions can still affect results. Runs use
temporary isolated Git workspaces, project skills and scoped
environment/configuration overrides. `CLAUDE_CODE_EFFORT_LEVEL` and
`ANTHROPIC_*` routing variables are removed from every child environment.
Competitor hooks are loaded only per run (`--plugin-dir` for Caveman/Ponytail,
`--settings` for the RTK hook); no global agent settings change. Plugin marker
files (`~/.claude/.caveman-active`, `~/.claude/.ponytail-active`) and RTK's
`~/.local/share/rtk/hook-audit.log` are written under the real `HOME` because an
isolated `CLAUDE_CONFIG_DIR` loses OAuth authentication; remove them afterwards.

Clone the competitors outside this repository and check out these revisions:

| Repository | Revision |
|---|---|
| headroomlabs-ai/headroom | e67b3c8a29443a60d6b0018fb22f525c5cd7e709 |
| juliusbrussee/caveman | 15581d14007fd01fb3f132016741962f34936ca2 |
| dietrichgebert/ponytail | 356918eba965ee1eac64bd3a7f0dd02108350de5 |
| rtk-ai/rtk | 8e9aa04cb2afb189747fac4e36bec2254ddd0564 |
| maxgfr/scopelet (original pilot) | d7f114c0e6ce44f34856ee1bd395a59d138f66f7 (v0.1.1) |

For the Haiku pilot use the full `skills/caveman` and `skills/ponytail`
directories (`--caveman-skill`, `--ponytail-skill`); for the Fable comparison
pass the whole checkouts as plugins (`--caveman-plugin`, `--ponytail-plugin`;
each must contain `.claude-plugin/plugin.json`). Build RTK with
`cargo build --release --locked` in its checkout (the pinned commit declares
crate version 0.42.4). Headroom 0.37.0 was installed from the pinned checkout in
a Python 3.13 virtual environment. Use the published Scopelet 0.1.1 binary and
tagged skill for the original pilot, and the published 0.1.3 binary plus the
v0.1.3 skill for the Fable comparison. Report hardware/build changes.

The harness copies binaries, complete skills and Python prefix files into each
campaign's `frozen/` directory before starting. Headroom's executable/venv stays
external; record its source revision and do not change the installation during a
campaign. SHA-256 hashes and CLI versions are recorded automatically. Supply
`--source-commits-json` with the revisions above as an additional provenance field.

## Run the matrix

Set these shell variables to the corresponding local paths: `COMPARE_SCOPELET`,
`COMPARE_SKILL`, `COMPARE_CAVEMAN`, `COMPARE_PONYTAIL`, `COMPARE_RTK` and
`COMPARE_HEADROOM`. Native executable variables must point to files; skill
variables must point to directories containing `SKILL.md`.

```sh
python3 bench/compare.py --dry-run --tasks task4,task2,task3 --out bench/runs/plan

python3 bench/compare.py --live --agents claude --tasks task4,task2,task3 \
  --scopelet-binary "$COMPARE_SCOPELET" --scopelet-skill "$COMPARE_SKILL" \
  --caveman-skill "$COMPARE_CAVEMAN" --ponytail-skill "$COMPARE_PONYTAIL" \
  --rtk-binary "$COMPARE_RTK" --headroom-binary "$COMPARE_HEADROOM" \
  --headroom-prefix-json '["python3","bench/headroom_proxy.py","--headroom","{headroom}"]' \
  --out bench/runs/comparison-claude

python3 bench/compare.py --live --agents codex --tasks task4,task2,task3 \
  --arms native,concise,scopelet,scopelet-ultra,scopelet-caveman,caveman,ponytail,rtk \
  --scopelet-binary "$COMPARE_SCOPELET" --scopelet-skill "$COMPARE_SKILL" \
  --caveman-skill "$COMPARE_CAVEMAN" --ponytail-skill "$COMPARE_PONYTAIL" \
  --rtk-binary "$COMPARE_RTK" --out bench/runs/comparison-codex
```

Default seed is 20260909 and default repetitions are one. Increase repetitions
and declare the new protocol before measuring. Random order does not make a
single repetition statistically reliable. The revised-skill follow-up uses
`--arms concise,scopelet-ultra --repetitions 2 --seed 20260910` and a separately
frozen 0.1.2 skill/binary.

## Fable 5.1 comparison

The [declared protocol](../docs/fable-comparison-plan.md) runs Claude Code
2.1.266 with `claude-fable-5-1` at high effort: nine arms, three tasks, two
repetitions, seed 20260911, 900 s timeout. Preflights use the same command with
`--tasks task3 --repetitions 1` and a separate output directory.

```sh
python3 bench/compare.py --live --agents claude --tasks task4,task2,task3 \
  --repetitions 2 --seed 20260911 --timeout 900 \
  --model claude-fable-5-1 --effort high --claude-bin /opt/homebrew/bin/claude \
  --scopelet-binary "$COMPARE_SCOPELET" --scopelet-skill skills/scopelet \
  --caveman-plugin "$COMPARE_CAVEMAN_CHECKOUT" --ponytail-plugin "$COMPARE_PONYTAIL_CHECKOUT" \
  --rtk-binary "$COMPARE_RTK" --rtk-integration hook \
  --headroom-binary "$COMPARE_HEADROOM" \
  --headroom-prefix-json '["python3","bench/headroom_proxy.py","--headroom","{headroom}"]' \
  --source-commits-json '{"caveman":"15581d14...","rtk":"8e9aa04c...","headroom":"e67b3c8a...","ponytail":"356918eb...","scopelet":"v0.1.3"}' \
  --out bench/runs/fable-comparison-20260909
```

`--rtk-integration hook` injects RTK's documented PreToolUse hook through
`--settings` and keeps the native prompt; `manual` reproduces the pilot's
wrapper guidance. Per-run results record the model observed in the `init`
event, every model in `modelUsage`, `model_mismatch`, turns, API duration,
Claude Code's list-basis cost estimate, hook lifecycle events, RTK audit
actions, tool errors, the task4 check sequence and cache-marker presence.
`scripts/summarize_traces.py <campaign>` prints a local audit of ordered tool
calls; it is for reading raw traces, not for publishing.

## Proxy verification

`headroom_proxy.py` is the portable version of the pilot's private adapter, with
an explicit executable path and a new fail-closed routing check. It launches a
local token-mode proxy, configures only the child Claude process and enables
Headroom's recovery MCP. It returns 125 if the agent succeeds without increasing
the proxy request counter. Agent failures preserve their status. All children
remain in the campaign process group for timeout cleanup. Set
`SCOPELET_HEADROOM_MODE=cache` only for a separately labeled cache-mode study.

Codex routing was not verified with this environment's authentication after four
preflight attempts, so the portable adapter rejects Codex rather than producing
false Headroom measurements. This is an environment-specific unmeasured cell,
not an upstream compatibility verdict. The original pilot adapter hashes are
retained in the results; this portable adapter has a different hash.

## Inspect and export

```sh
python3 -m unittest discover -s bench -p 'test_*.py'
python3 scripts/export_comparison.py bench/runs/comparison-claude \
  bench/runs/comparison-codex --out bench/results/my-comparison.json
```

Reports retain failed runs, unknown usage, wall-clock duration, cache fields,
fixture hashes and adoption counters. The external grader checks the final
workspace against pristine checks and rejects modified fixtures. Tool counts do
not prove required command order, skill compliance or semantic fidelity. Review
raw traces when those matter. A returned view's mode can differ from the requested
arm, and an exact expansion can intentionally use default mode.

Raw transcripts, prompts, generated workspaces and proxy logs remain under
ignored `bench/runs/`. Export only inspected summaries. Logical session input
includes cached input; it is not a price calculation. Competitor compression
estimates and byte reductions are separate measurements.

## Automatic-mode pilot: Codex Luna only

`auto.py` fixes `gpt-5.6-luna` at low effort and compares ordinary task prompts
without invoking a skill. It uses the immutable synthetic fixtures and external
functional grader from `run.py`. Four repetitions across three tasks and five
arms form 60 planned cells. Headroom remains unmeasured because the available
proxy adapter rejects Codex; RTK uses its upstream Codex awareness document.
Missing integrations do not consume model sessions or become native results.

```sh
python3 bench/auto.py --out bench/runs/luna-plan
python3 bench/auto.py --live --binary target/release/scopelet \
  --rtk /absolute/path/to/pinned/rtk \
  --rtk-awareness /absolute/path/to/pinned/rtk-awareness-full.md \
  --out bench/runs/luna-auto --max-attempts 48
```

The executable and competitor instructions are frozen before measurement.
Each attempt is persisted before launch, failures remain in reports, and missing
usage stops the campaign for inspection. Reports include raw stream hashes,
reported input/output/cache usage, task grade, commands and adoption evidence.
Four available arms consume 48 sessions; any extra validation must keep the
whole first-phase total under 60. No Claude CLI, auxiliary model, or API proxy
fallback is allowed. Local Codex authentication is copied into each private
fixture and deleted before archiving; raw traces remain local under `bench/runs`.

Export inspected, path-free summaries with typed completed-tool audits:

```sh
python3 bench/export_auto.py bench/runs/luna-auto bench/results/luna-auto.json
```

Use `--tasks task4 --arms default caveman --repetitions 1 --max-attempts 2`
for a separate final-binary smoke; add `--code-mode` for Codex's experimental
code mode. These are separate treatments, never pooled into the primary matrix.
The maximum applies per invocation; count every campaign toward the user budget.

## Proportional small-edit checks

`proportional.py` compares native, a frozen released binary and a frozen candidate
on Python normalization, JavaScript nullish defaults and a JSON configuration
edit. Identical prompts, independent graders, preserved fixtures and raw usage
make correctness failures visible alongside tokens. The JSON grader distinguishes
zero from false; the JavaScript grader checks values beyond the visible test.
Two repetitions produce 18 sessions, run at most two at a time. A missing-usage
batch stops the campaign. Raw workspaces and authentication stay private; local
auth copies are removed before workspace archival.

```sh
python3 bench/proportional.py --released /path/to/released/scopelet \
  --candidate target/release/scopelet --out bench/runs/proportional-plan
python3 bench/proportional.py --live --released /path/to/released/scopelet \
  --candidate target/release/scopelet --out bench/runs/proportional-live
```

Use a new directory for every revision. Keep earlier failures and distinguish
candidate hashes; do not pool revised candidates into an earlier comparison.
This small sample is exploratory, not a claim of universal savings.

## Engine and compact-v2 performance

`performance.py` compares frozen release binaries offline, with five warmups
and thirty measurements per cell by default. It alternates native process
invocations of baseline v1, candidate v1 and candidate v2. Cold means an empty
application cache; the OS page cache is not flushed. It reports wall time,
maximum process RSS, output hashes, failures and retained cache bytes. V1 output
mismatches fail the campaign. `--stress` adds a capture just below 32 MiB.

```sh
python3 bench/performance.py --baseline /path/to/baseline --candidate target/release/scopelet \
  --stress --out bench/runs/performance-new
```

`performance_live.py` is a separate Luna-only pilot: five tasks, three arms
(native, frozen baseline, candidate v2), two repetitions, 30 attempts maximum.
It uses `gpt-5.6-luna` at low effort, isolated synthetic workspaces and scoped
hooks. No retry or model fallback occurs. Quota rejection or missing usage stops
the campaign; failed sessions remain in the report. Existing harness usage
normalization and independent graders are reused. Global skills are disabled.
The native arm uses native tools; the two Scopelet arms receive the same brief
availability instruction and retain their respective hooks. The recovery task
provides native text versus a saved compact view backed by immutable originals.
Raw traces and temporary authentication stay local; authentication is removed
before archiving a workspace. Reports do not equate logical tokens with bills.

```sh
python3 bench/performance_live.py --baseline /path/to/baseline --candidate target/release/scopelet \
  --out bench/runs/luna-performance-plan
python3 bench/performance_live.py --live --baseline /path/to/baseline --candidate target/release/scopelet \
  --out bench/runs/luna-performance-new
```

The rollout gate for v2 is no functional regression and at least 10% fewer
whole-session logical input plus output tokens than baseline on the combined
four evidence-heavy tasks. An incomplete campaign does not pass this gate.
V1 stays the default unless that complete comparison supports switching.
