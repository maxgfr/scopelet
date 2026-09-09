# Fable 5.1 comparison protocol

Declared before measurement on 2026-09-09. Previous campaigns (the [51-run
pilot](direct-comparison-2026-09-09.md) and the [24-run follow-up](skill-followup-2026-09-09.md))
ran Claude Code with Haiku 4.5 and established no general Scopelet advantage:
Headroom was cheapest on the three Claude tasks, RTK on the noisy Codex task,
and traces showed unnecessary reads, default-mode queries in ultra arms and a
`rg --hidden` search that re-ingested the workspace-local cache. The 0.1.3 cache
markers were validated by byte replay and two smokes, never by a token campaign.

## Question

On Claude Fable 5.1 at high effort, does Scopelet reduce whole-session tokens at
equal functional quality against the native workflow, a one-sentence concision
instruction, and the four competitors? The output is a decision among continue,
specialize, reuse a competitor, or stop. A flattering ranking is not a goal;
unfavorable results are retained and reported.

## Matrix

Claude Code only (`/opt/homebrew/bin/claude`, 2.1.266), `--model claude-fable-5-1
--effort high` in every arm. Nine arms, three tasks, two repetitions: 54 runs,
shuffled with seed 20260911, 900 s timeout per run (Fable high is slower than
Haiku; declared before measurement), sequential execution (parallel runs would
confound provider cache and load). Tasks: task4 (noisy failing check, real
error, fix through the call path, rerun), task2 (exact JSONL aggregation),
task3 (small known-file edit). No treatment changes during the campaign. No
run is dropped for failure, timeout, adoption defect or unfavorable result.

Two repetitions remain a pilot. No numerical noise threshold is invented; the
spread of both repetitions is reported per cell.

## Arms and integration modes

| Arm | Integration | Difference from the Haiku pilot |
|---|---|---|
| native | unchanged baseline prompt | none |
| concise | baseline plus the existing one-sentence `CONCISE` instruction | none |
| scopelet / scopelet-ultra | published 0.1.3 binary (macOS ARM64 SHA-256 `0d2e57b5a9545ea195190e22bfd015cad398988aa970532df8ee3ed180d5a432`, verified against the release `SHA256SUMS`), the v0.1.3 skill copied as a project skill, explicit `/scopelet` activation, `SCOPELET_BIN` and `SCOPELET_CACHE_DIR` inside the workspace | binary and skill are the released 0.1.3 (the follow-up measured a 0.1.2 candidate); the in-workspace cache is kept on purpose to test the cache markers under Fable |
| scopelet-caveman | scopelet-ultra plus the Caveman plugin (below) and explicit activation | plugin hooks instead of a copied skill |
| caveman | upstream checkout loaded with `--plugin-dir`; its SessionStart and UserPromptSubmit hooks inject the full ruleset; prompt prefixed with `/caveman:caveman` (plugin skills are namespaced); `CAVEMAN_DEFAULT_MODE=full`, `DO_NOT_TRACK=1` | the pilot copied the skill and used `/caveman` without hooks |
| ponytail | upstream checkout with `--plugin-dir`; SessionStart, SubagentStart and UserPromptSubmit hooks; prompt prefixed with `/ponytail:ponytail`; `PONYTAIL_DEFAULT_MODE=full` | the pilot copied the skill without hooks; Ponytail's own harness documents the SessionStart hook as a contamination risk for baselines, which is why the hook is only loaded in its arm |
| rtk | the PreToolUse `Bash` hook that `rtk init` documents, injected per run through `--settings` as `<frozen rtk> hook claude`; native prompt, no manual wrapper; frozen binary directory prepended to `PATH` because rewrites call bare `rtk`; `RTK_HOOK_AUDIT=1`, per-run `RTK_DB_PATH`/`RTK_RECALL_DB`, `RTK_TELEMETRY_DISABLED=1` | the pilot used a manual `rtk test` wrapper in the prompt (still available as `--rtk-integration manual`) |
| headroom | existing `bench/headroom_proxy.py` adapter: fresh local proxy per run in token mode with the recovery MCP, fail-closed exit 125 when the proxy request counter does not rise; `HEADROOM_BEACON=off`, `HEADROOM_TELEMETRY=off`, `HEADROOM_OUTPUT_SHAPER` absent, per-run workspace/config directories and request log | none; the default cache mode is not measured, as in the pilot |

Integration modes differ by design (automatic hooks, manual activation, proxy,
plugin). A lower number is an observation for that integration, not a universal
ranking.

Known coverage facts recorded before measurement: `rtk hook check` rewrites
`cat`, `ls`, `git`, `rg` and `pytest` commands but reports no rewrite for
`python3 checks.py`, for the `head -c 2048` preview, or for piped commands. The
Caveman proxy does not compress in `claude -p` under OAuth (upstream issue
#1020); its counters are expected at zero and it is a preflight only, reported
as unmeasured. The Headroom `coding` savings profile stays active alongside the
explicit `--mode token`, which the CLI flag controls.

## Pins

Default-branch heads on 2026-09-09, identical to the README pins, recloned
into `bench/runs/competitors-20260909/` (git-ignored) and frozen by the harness
into each campaign's `frozen/` directory with hashes.

| Project | Pinned commit | Nearest release tag | Note |
|---|---|---|---|
| Caveman | `15581d14007fd01fb3f132016741962f34936ca2` | v2.6.0 = `b82c0ad4` | plugin manifest `.claude-plugin/plugin.json` verified |
| RTK | `8e9aa04cb2afb189747fac4e36bec2254ddd0564` (`develop`) | v0.48.0 = `fde0a8f1` | `Cargo.toml` declares 0.42.4, so the pilot's frozen `rtk 0.42.4` was consistent with this commit; rebuilt with `cargo build --release --locked`, new hash recorded |
| Headroom | `e67b3c8a29443a60d6b0018fb22f525c5cd7e709` | v0.37.0 = `32d7ca45` | existing venv `headroom, version 0.37.0`, executable SHA-256 recorded in the campaign |
| Ponytail | `356918eba965ee1eac64bd3a7f0dd02108350de5` | v4.9.0 = `0a4dd63a` | plugin manifest verified |
| Scopelet | tag v0.1.3 | v0.1.3 | `skills/` and `src/` at HEAD are byte-identical to the tag |

## Model and effort verification

`CLAUDE_CODE_EFFORT_LEVEL`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY` and
`ANTHROPIC_AUTH_TOKEN` are removed from every child environment (the Headroom
adapter sets its own base URL for its child only). The `init` event's `model`
must equal `claude-fable-5-1`, and `claude-fable-5-1` must appear in
`modelUsage`; otherwise `model_mismatch` is recorded and the run is kept but
flagged. Claude Code also bills a small auxiliary Haiku usage in `modelUsage`
(observed 899 input tokens in a one-turn probe); it is recorded per model, not
treated as a mismatch, and included in whole-session totals because the session
pays for it. Effort is not observable from provider output: it is recorded as
requested, with `thinking_tokens` and `fast_mode_state` as indirect indicators.

An isolated `CLAUDE_CONFIG_DIR` loses OAuth authentication ("Not logged in"),
so runs keep the real `HOME`. Caveman and Ponytail therefore write their
`.caveman-active`/`.ponytail-active` marker files under `~/.claude`; they are
removed after the campaign and affect only those plugins' own hooks.

## Metrics per run

Functional success by the immutable external grader with fixture hash checks;
input, output, cache read, cache creation, logical input (uncached plus cache
read plus cache creation) and `thinking_tokens` (already inside output, never
added again); `num_turns`, `duration_ms`, `duration_api_ms`; Claude Code's
`total_cost_usd` kept in a separate field labeled as a list-basis estimate, not
an invoice; tool counts, Scopelet invocations and `expand` recoveries, observed
view modes, `rtk recall` calls and RTK audit actions (rewrite/skip counts from
the hook audit log delta), Headroom `headroom_retrieve` calls and `/stats`
deltas, hook lifecycle events per plugin (`--include-hook-events`), tool-result
errors, permission denials, the task4 sequence (a failing checks run before the
fix and a passing one after, from ordered tool returns), bytes of Scopelet cache
paths appearing in tool returns, and the presence of the `.ignore`/`.gitignore`
markers in the final workspace cache.

Observed `default` views in an ultra arm are reported; `expand` returns default
mode by construction, so such a view is not by itself a non-compliance.

## Preflights

One task3 run per integration, in a separate directory, not counted: Fable
observed in `init`; RTK hook audit delta non-empty; Headroom proxy request
counter above zero; Caveman and Ponytail hook responses present in the stream;
an ultra view observed for Scopelet; the Caveman proxy counters probed and
expected at zero. An integration without proof of action is marked unmeasured,
removed from the campaign, and its preflight is kept and exported.

## Improvement rule

Any Scopelet change happens only after the 54-run export is frozen and only on
trace evidence. It is measured in a separate campaign (`fable-followup-<date>`,
new seed, own directory) with contemporaneous `concise` and unchanged
`scopelet-ultra` controls, the same tasks and two repetitions. Nothing is
attributed retroactively to the frozen campaign.

## Amendment during execution (2026-09-09, 11:31)

The first Fable attempt (`fable-comparison-20260909`) hit the account's
five-hour usage window: all 54 sessions returned HTTP 429 after at most one
turn and none is a measurement; the export is kept as
`fable-comparison-2026-09-09-aborted-attempt1.json`. The harness then gained
window-aware retries (aborted attempts are recorded apart from runs) and the
second attempt ran normally. At 40 of 54 completed runs (all graded pass, no
aborted attempt), the operator decided that measured test sessions must use
Haiku 4.5 and that Fable is reserved for orchestration, because the campaign
shares the account window with the operator's own sessions. The Fable attempt
was stopped at that point, not on any result: its 40 runs are exported as a
partial, unplanned dataset (`fable-comparison-2026-09-09-partial.json`) and
are reported separately with their incomplete cells named explicitly.

The declared 54-run matrix (same arms, tasks, seed 20260911, two repetitions,
900 s timeout, same frozen pins and integration modes) is therefore executed
with `--model claude-haiku-4-5-20251001 --effort high` (Haiku accepts the
flag; the probe recorded `claude-haiku-4-5-20251001` in `init`). Everything
else in this protocol is unchanged, including the model verification
(`init.model` must equal the requested model), the metrics, the preflight
rule and the post-freeze improvement rule. The question becomes: on Haiku 4.5
at high effort, with the documented plugin/hook/proxy integrations and the
released 0.1.3, does Scopelet reduce whole-session tokens at equal quality?
The partial Fable data is a secondary observation, not the primary answer.

## Limits known in advance

Claude Code's built-in skills and tools stay visible in every arm; provider
caches are observed, not forced cold; RTK's hook skips piped commands and
commands it does not recognize; the Caveman proxy is outside the headless
scope; two repetitions cannot establish statistical superiority; the operator's
machine and network are shared by all runs.
