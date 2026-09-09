# Independent comparison on Haiku 4.5, with a partial Fable 5.1 dataset — 2026-09-09

Scopelet's advantage is task-specific, not general. On Haiku 4.5 at high effort
it cut the noisy-command session by 51% (default) and 56% (ultra) against the
native workflow, with both repetitions within 1% of each other, and cut the
JSONL aggregation by 19% in default mode. It cost 35% more on the tiny edit,
where the agent mostly did not use it at all. Headroom was the strongest
general competitor: 44% below native on the noisy command on Haiku, and the
cheapest arm in all three Fable tasks (two of them from one repetition). On Fable 5.1 the
noisy-command advantage disappears because Claude Code 2.1.266 now persists
large tool outputs to disk and shows a 2 KB preview, which removes most of the
cost Scopelet was designed to avoid. All 94 measured sessions passed the
immutable external grader.

Decision: **specialize**. Keep Scopelet for noisy commands whose output the
host does not truncate and for exact aggregation of large structured files;
do not activate it for small known-file edits; do not present it as a general
token saver. Details, evidence and limits follow.

The [protocol](fable-comparison-plan.md) was committed before measurement and
amended once, during execution, when the operator moved measured sessions to
Haiku. The primary dataset is the 54-run Haiku campaign
([`haiku-comparison-2026-09-09.json`](../bench/results/haiku-comparison-2026-09-09.json)).
The 40 completed Fable runs
([`fable-comparison-2026-09-09-partial.json`](../bench/results/fable-comparison-2026-09-09-partial.json))
are a secondary, incomplete dataset. Values are whole-session logical input
(uncached plus cache read plus cache creation) plus reported output tokens,
including Claude Code's small auxiliary Haiku calls; they are not prices.
Claude Code's own list-basis cost estimate is exported separately.

## What was run

Claude Code 2.1.266, `/opt/homebrew/bin/claude`, `--effort high`,
`--setting-sources project`, no MCP servers, hook lifecycle events recorded.
Nine arms, three tasks, two repetitions, seed 20260911, sequential, 900 s
timeout. Frozen inputs: Scopelet 0.1.3 release binary (SHA-256
`0d2e57b5…d5a432`, verified against the release manifest) with the v0.1.3
skill; Caveman `15581d1` and Ponytail `356918e` as `--plugin-dir` plugins
(SessionStart and UserPromptSubmit hooks proven in every run); RTK `8e9aa04`
rebuilt (`rtk 0.42.4`) as the documented PreToolUse hook through `--settings`;
Headroom 0.37.0 as a fresh token-mode proxy per run with the recovery MCP.
`CLAUDE_CODE_EFFORT_LEVEL` and `ANTHROPIC_*` were removed from every child
environment. `init.model` equalled the requested model in all 94 runs.

Six task3 preflights on Fable
([`fable-preflight-2026-09-09.json`](../bench/results/fable-preflight-2026-09-09.json))
proved each integration acted: plugin hook responses in the stream, an RTK
audit entry, Headroom proxy requests, an observed ultra view. The Caveman
standalone proxy was probed separately
([`fable-preflight-caveman-proxy-2026-09-09.json`](../bench/results/fable-preflight-caveman-proxy-2026-09-09.json)):
six requests, all classified as subscription auth, none eligible for
compression. It is unmeasured, as expected from upstream issue #1020.

The first Fable attempt hit the account's five-hour window: 54 sessions, all
HTTP 429, none measured
([`fable-comparison-2026-09-09-aborted-attempt1.json`](../bench/results/fable-comparison-2026-09-09-aborted-attempt1.json)).
The harness then gained window-aware retries; neither later campaign needed one.

## Haiku 4.5, 54 runs, both repetitions

Change columns compare cell means; both repetitions are shown because the
spread matters more than the mean at this sample size.

### Noisy command (task4)

| Arm | Rep 1 / Rep 2 | Mean | vs native | vs concise |
|---|---:|---:|---:|---:|
| native | 243,273 / 336,117 | 289,695 | — | +24.4% |
| concise | 218,289 / 247,319 | 232,804 | -19.6% | — |
| scopelet | 142,961 / 142,534 | 142,748 | -50.7% | -38.7% |
| scopelet-ultra | 128,364 / 127,558 | 127,961 | -55.8% | -45.0% |
| scopelet-caveman | 137,308 / 136,949 | 137,128 | -52.7% | -41.1% |
| caveman | 386,645 / 394,724 | 390,684 | +34.9% | +67.8% |
| ponytail | 231,465 / 409,454 | 320,460 | +10.6% | +37.7% |
| rtk | 338,607 / 166,219 | 252,413 | -12.9% | +8.4% |
| headroom | 138,936 / 184,912 | 161,924 | -44.1% | -30.4% |

### JSONL counts (task2)

| Arm | Rep 1 / Rep 2 | Mean | vs native | vs concise |
|---|---:|---:|---:|---:|
| native | 168,233 / 166,373 | 167,303 | — | -22.0% |
| concise | 214,032 / 215,078 | 214,555 | +28.2% | — |
| scopelet | 149,657 / 121,918 | 135,788 | -18.8% | -36.7% |
| scopelet-ultra | 236,602 / 124,227 | 180,414 | +7.8% | -15.9% |
| scopelet-caveman | 133,699 / 161,405 | 147,552 | -11.8% | -31.2% |
| caveman | 235,031 / 235,163 | 235,097 | +40.5% | +9.6% |
| ponytail | 110,418 / 169,102 | 139,760 | -16.5% | -34.9% |
| rtk | 165,247 / 214,987 | 190,117 | +13.6% | -11.4% |
| headroom | 156,658 / 184,993 | 170,826 | +2.1% | -20.4% |

### Tiny edit (task3)

| Arm | Rep 1 / Rep 2 | Mean | vs native | vs concise |
|---|---:|---:|---:|---:|
| native | 118,860 / 117,750 | 118,305 | — | -18.3% |
| concise | 167,412 / 122,324 | 144,868 | +22.5% | — |
| scopelet | 147,942 / 172,692 | 160,317 | +35.5% | +10.7% |
| scopelet-ultra | 146,357 / 174,852 | 160,604 | +35.8% | +10.9% |
| scopelet-caveman | 212,253 / 185,617 | 198,935 | +68.2% | +37.3% |
| caveman | 165,570 / 192,949 | 179,260 | +51.5% | +23.7% |
| ponytail | 163,456 / 162,831 | 163,144 | +37.9% | +12.6% |
| rtk | 143,121 / 141,968 | 142,544 | +20.5% | -1.6% |
| headroom | 200,166 / 87,154 | 143,660 | +21.4% | -0.8% |

### Breakdown

Uncached input is negligible everywhere (33 to 106 tokens); sessions are
dominated by cache reads, so the totals track turns times context size. The
Scopelet arms finished the noisy command in 5 turns with 4 tool calls in every
one of their six runs; native needed 9 to 11 turns and 8 to 10 calls. Thinking
tokens (already inside output) ranged from 475 to 1,925 per session and never
dominated. Full per-run breakdowns, including API time and tool errors, are
in the exported file; the "tool error" in each Scopelet noisy-command run is
the `scopelet run` wrapper propagating the expected failing exit status of
`checks.py`, and the one error in most tiny-edit runs across all arms is the
agent calling `python`, which this host does not provide, before `python3`
(19 of 54 sessions, spread over every arm).

## Fable 5.1, 40 of 54 runs (stopped by the operator, not on results)

Missing cells: caveman on the noisy command (no run); one repetition only for
native, concise, scopelet-caveman and headroom on the noisy command, scopelet,
ponytail and rtk on JSONL, and scopelet, caveman, ponytail and headroom on the
tiny edit. Single-repetition cells are marked.

| Task | native | concise | scopelet | scopelet-ultra | scopelet-caveman | caveman | ponytail | rtk | headroom |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Noisy command | 128,557 (1) | 178,510 (1) | 137,359 / 132,309 | 141,697 / 141,111 | 238,150 (1) | not run | 242,660 / 159,107 | 128,214 / 150,755 | 92,063 (1) |
| JSONL counts | 61,949 / 105,281 | 61,525 / 61,826 | not run | 111,528 / 112,488 | 166,969 / 168,116 | 81,079 / 81,207 | 75,993 (1) | 62,053 (1) | 41,817 / 42,130 |
| Tiny edit | 80,351 / 80,705 | 80,681 / 80,691 | 109,929 (1) | 110,166 / 109,914 | 165,540 / 139,521 | 132,362 (1) | 101,222 (1) | 80,709 / 79,397 | 55,732 (1) |

On Fable, Scopelet is never below native: +5% to +10% on the noisy command,
+34% (ultra) on JSONL, +37% on the tiny edit. Headroom is the cheapest arm in
every Fable task where it ran. Two host facts explain most of the noisy-command
reversal. First, Claude Code 2.1.266 persisted the 1,203-line check output in
7 of 12 Fable noisy-command runs and returned a 2 KB preview; the agent then
read the failure line from the persisted file with one `grep`. The Haiku
pilot's Claude Code 2.1.263 traces contain no such preview, and only 1 of 18
Haiku runs in this campaign triggered it, because Haiku usually piped the
command through `tail` or `grep -v` itself. Second, Fable's sessions were
short and stable (5 to 13 turns; native tiny edit 80,351 / 80,705), so the
context of an activated skill is a larger share of the total: on the tiny
edit the Scopelet ultra arm read 97,733 cached tokens over five turns where
native read 70,770 over six.

Fable's sessions also bill a small auxiliary Haiku usage (15,310 input tokens
across the 40 runs, included in the totals above).

## Adoption and trace audit

Every run's evidence is in the exported `tools` field; the summary below is
from the 54 Haiku runs unless stated.

- **Scopelet was used in 15 of 18 Scopelet-arm runs.** The three exceptions
  are tiny-edit runs (scopelet, scopelet-ultra and scopelet-caveman, repetition
  1) that read and edited the file natively, as the skill permits for a small
  known file; their cost is the activated skill's context, not the engine.
  Every ultra-arm run that used Scopelet returned an ultra view; no default
  view appeared in an ultra arm. No `expand` was needed in either campaign.
- **Noisy command.** All six Haiku Scopelet runs wrapped both `checks.py` runs
  with `scopelet run`, and the fail-then-pass sequence was verified in all 18
  Haiku and 12 Fable noisy-command runs (7 of them through the persisted
  output file).
- **JSONL.** Eight of the twelve non-Scopelet Haiku runs read the whole
  86,788-byte `records.jsonl` into context; the six Scopelet runs used the
  2,048-byte preview and one aggregate query instead. The expensive
  scopelet-ultra repetition 1 (236,602) spent four extra turns on a malformed
  `query --repo` call, a `--help` call and a read of the skill's query
  reference before the correct `--spec` query; repetition 2 (124,227) went
  straight to it. That friction is retained in the mean.
- **Cache correction.** The `.ignore` and `.gitignore` markers were present in
  the final workspace cache of every Scopelet run, and no tool return in
  either campaign contained a `scopelet-cache/blobs` or `scopelet-cache/artifacts`
  path (0 re-ingested bytes over 94 runs). The 0.1.3 fix held under live
  sessions on both models.
- **RTK hook.** The hook fired on every Bash call (PreToolUse events present)
  but rewrote only one of fifteen audited commands in the six Haiku runs
  (`ls -la src/`) and eight of eleven in the five Fable runs; every
  `python3 checks.py` form, including piped
  ones, was logged as `skip:defer`. No `rtk recall` was issued. RTK's
  automatic mode therefore measured close to native here; the pilot's manual
  `rtk test` wrapper is a different integration and is not repeated.
- **Headroom.** The proxy served 6 to 12 requests per run and reported 14% to
  26% input savings by its own accounting; no `headroom_retrieve` call was
  made, so nothing compressed was needed back. Its default cache mode was not
  measured.
- **Caveman and Ponytail.** Both plugins injected their rulesets at
  SessionStart and reinforced them at UserPromptSubmit in every run. Caveman
  was the most expensive arm on the noisy command and JSONL on Haiku (13 and
  12 turns on the noisy command); Ponytail varied by 77% between its two
  noisy-command repetitions.
- **Model and effort.** No model mismatch in 94 runs; `fast_mode_state` was
  `off`; no permission denial. Effort is recorded as requested; the provider
  does not expose it.

## Answers to the five questions

1. **Fewer tokens than native at equal quality?** Yes on the noisy command
   and JSONL aggregation on Haiku (-51%/-56% and -19% for default mode, all
   graded pass), no on the tiny edit (+35%), and no on Fable, where the host's
   output persistence and the short sessions leave nothing for Scopelet to
   save (+5% to +37%).
2. **Better than a one-sentence concision instruction?** The instruction alone
   was inconsistent: -20% on the noisy command but +28% on JSONL and +22% on
   the tiny edit on Haiku. Scopelet beat it on the two evidence-heavy tasks
   and lost on the tiny edit.
3. **Against the four competitors?** Headroom is the only competitor that
   reduced the noisy command on both models (-44% on Haiku, -28% on Fable
   against a single native run) and was the cheapest arm in all three Fable tasks.
   Scopelet ultra beat Headroom on the Haiku noisy command (127,961 against
   161,924) and Scopelet default beat it on Haiku JSONL (135,788 against
   170,826). RTK's hook mode, Caveman and Ponytail did not reduce session
   tokens on these tasks; Caveman and Ponytail increased them.
4. **Does the 0.1.3 cache correction hold?** Yes: markers present and zero
   re-ingested bytes in every run, on both models.
5. **What do the traces say about Scopelet's own behavior?** Adoption was
   correct where it mattered (wrappers on both check runs, ultra flags on
   every ultra call, bounded preview before aggregation). The costs are the
   activated skill's context on tasks that do not need it, and one Haiku run
   of instruction friction. Neither is an engine defect.

## Recommendation

Specialize. The evidence supports Scopelet as a targeted tool for two
situations: a noisy command whose output the host would otherwise place in
context, and exact filtering or grouping over a large structured file. It
does not support activating Scopelet by default, nor combining it with
Caveman (the combination was never cheaper than ultra alone). For an operator
who accepts a proxy, Headroom is the stronger general-purpose choice on these
tasks and is not replaced by Scopelet. On Claude Code 2.1.266 with Fable, the
noisy-command case is largely handled by the host itself; the remaining
Scopelet value there is exact aggregation with recoverable evidence, which no
run in this campaign made cheaper than native.

No follow-up campaign was run. The improvement rule required a trace-proven,
correctable Scopelet defect; the observed costs come from the arm design
(explicit activation on a task that needs no evidence tool), from the host
(output persistence, missing `python`, a `find` alias) and from one
instruction-following slip in a single run. Changing the skill to say "do not
activate for small edits" would change the arm, not the tool, and is a
documentation decision recorded here rather than a measured change.

## Limits

Two repetitions per cell are a pilot; several Haiku cells differ by more
than 40% between repetitions (native noisy command, ponytail noisy command,
headroom tiny edit), and no significance is claimed. The Fable dataset is
incomplete and its single-repetition cells are indicative only. Claude Code's
built-in skills and tools remained visible in every arm. The host had no
`python` executable and a `find` alias that failed in at least one Fable
native run; these affected arms equally but inflated turn counts. Provider
caches were observed, not forced cold; all runs shared one machine, one
network and one account window that the operator's own session also used.
RTK was measured in its documented automatic mode, which does not cover
`python3` invocations. Headroom ran in token mode with its default coding
profile. The Caveman proxy is outside the headless OAuth scope. Raw traces
stay local under `bench/runs/`; the exported files carry SHA-256 hashes of
every trace, prompt, binary and skill.

Reproduction commands and versions are in the [bench README](../bench/README.md);
`scripts/summarize_traces.py` prints the per-run audit used above.
