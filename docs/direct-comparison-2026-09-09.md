# Direct competitor pilot — 2026-09-09

Scopelet is not an overall winner in this pilot. All 51 completed runs passed
the immutable external functional grader. RTK had the lowest observed session
usage on the noisy-command task with Codex; Headroom had the lowest observed
usage in all three Claude tasks. One run per cell cannot establish a reliable
ranking or a production quality guarantee.

The [protocol](direct-comparison-plan.md) was committed before measurement.
The [sanitized measurements](../bench/results/direct-comparison-2026-09-09.json)
include provider usage, cache accounting, timings, fixture/skill/binary/trace
hashes and adoption diagnostics. Models: GPT-5.6-Luna (low) in Codex 0.153.4;
Haiku 4.5 in Claude Code 2.1.263. These are whole-session logical input plus
reported output tokens, including cached input, **not cost in currency**.

## Completed matrix

Three tasks: a noisy failing Python check and source fix (task4), exact JSONL
aggregation (task2), and a tiny known-file edit (task3). Values below are tokens;
lower is better only when quality and faithful treatment adoption are retained.

### Codex

| Workflow | Noisy command | JSONL counts | Tiny edit |
|---|---:|---:|---:|
| native | 161,394 | 78,351 | 104,174 |
| concise | 159,685 | 52,306 | 91,707 |
| scopelet | 144,177 | 71,651 | 104,104 |
| scopelet-ultra | 131,587 | 57,422 | 104,268 |
| scopelet-caveman | 215,088 | 86,761 | 79,649 |
| caveman | 153,556 | 74,691 | 104,121 |
| ponytail | 128,186 | 90,917 | 88,773 |
| rtk | 79,153 | 79,489 | 105,716 |
| headroom | unmeasured | unmeasured | unmeasured |

### Claude

| Workflow | Noisy command | JSONL counts | Tiny edit |
|---|---:|---:|---:|
| native | 288,348 | 189,104 | 164,864 |
| concise | 166,561 | 167,573 | 118,248 |
| scopelet | 142,552 | 180,248 | 153,276 |
| scopelet-ultra | 127,807 | 316,294 | 146,505 |
| scopelet-caveman | 195,690 | 189,374 | 192,086 |
| caveman | 277,539 | 173,170 | 154,172 |
| ponytail | 230,016 | 378,484 | 177,245 |
| rtk | 303,063 | 262,490 | 169,838 |
| headroom | 124,305 | 139,857 | 101,444 |

## What was actually exercised

- Scopelet 0.1.1 used the published binary and frozen project skill. Ultra is an
  instruction-level treatment: Claude used default mode for its JSONL ultra
  query and read the entire JSONL before computing. Its 316,294-token result is
  retained. Ultra does not abridge JSON values, so attributing this result to
  JSON compression damage would be incorrect.
- Scopelet plus Caveman means **ultra plus the full upstream Caveman skill**.
  It did not consistently beat ultra alone. Claude also issued a default-mode
  query in the tiny-edit combination; Codex retried an invalid repeated context
  flag in the noisy-command combination. These are workflow costs, not discarded
  outliers. The Claude tiny-edit ultra-only run used no Scopelet command
  (native bypass); its score is not an engine-compression result.
- RTK used its documented manual test wrapper on the noisy task. The other
  tasks allowed native fallback. This is not a benchmark of all RTK command
  filters or its automatic rewriting hooks.
- Caveman and Ponytail used full pinned project skills and explicit native
  activation. A missing `Skill` tool event is not proof that native slash/$
  activation failed, nor is shorter prose proof of instruction compliance.
- Headroom used token mode, a real local proxy and its recovery MCP in Claude.
  Proxy request counters rose from zero in each included run. Its own estimates
  are separate diagnostics, not substitutes for provider-reported usage. Its
  default cache mode was not measured.
- Four Codex preflight attempts completed their task but registered zero proxy
  requests in this environment. The three planned Codex/Headroom cells were
  therefore left **unmeasured**. This does not establish a general Headroom
  incompatibility or make Scopelet the winner by default. The [six integration
  validation runs](../bench/results/headroom-integration-2026-09-09.json) retain
  these preflights plus the successful portable-adapter smoke.

## Product decisions

Scopelet is useful when exact local filtering/grouping and recoverable evidence
are needed without a proxy, hooks or an extra model. This pilot does not establish
that those features are unique, or that they always reduce session tokens.
Prefer direct reads for tiny known files. If RTK already handles a command well,
keep it instead of adding a second wrapper. Headroom deserves consideration for
proxy-based workflows; Caveman offers a smaller communication intervention.
Do not install every layer merely because each advertises token savings.

The next skill revision addresses observed mistakes: bounded schema previews,
explicit ultra flags on every query/run, one shared search-context flag, and
stopping unnecessary evidence collection. Its separate repeated evaluation is
published in the [24-run follow-up](skill-followup-2026-09-09.md); it is not
silently substituted into this table.

## Limits and reproduction

This is a randomized, single-repetition synthetic pilot, not a multi-repository
quality noninferiority study. Required-check counts are recorded; automatic
sequence verification is unknown. Acceptance checks validate the resulting
behavior and reject modified original test fixtures. Provider caches were
observed, not forced cold. Codex discoverable global skills were disabled;
remaining host instructions and agent scaffolding can still affect totals.
Earlier campaigns used different surrounding context and are not interchangeable
with these cells. Raw transcripts remain local; published hashes support local
audit but do not allow independent inspection of private transcripts.

See [reproduction instructions](../bench/README.md) and the
[scientific review](scientific-review-2026-09-09.md) for the rationale and stronger
evaluation designs. No universal savings percentage is established.
