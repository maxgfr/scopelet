# Scopelet 0.3.0 current comparison — 2026-09-10

Scopelet used **26.9% fewer** logical session tokens than native on the measured Luna task mix and **0.7% more** on Haiku. These are exploratory observations, not a universal savings claim. The Rust runtime is unchanged; confirmed fixes concern benchmark correctness and evidence retention.

**53 attempted sessions:** 3 integration preflights, 42 main comparisons and 8 targeted follow-ups. 52 pass all recorded acceptance checks. One RTK/Codex attempt is a harness failure with an unavailable independent grade; its reported tokens are retained. No attempt was retried. The cap was 60.

## Versions and method

Codex CLI 0.154.0 requested `gpt-5.6-luna`, low effort; Claude Code 2.1.267 used `claude-haiku-4-5-20251001`, default effort. Codex did not report the serving model identity. Results are compared within each host/model. RTK is the checksummed **0.48.0 release**, not the old development build. Headroom is the **0.37.0 PyPI wheel**, with distribution RECORD hash and dependency versions retained; its source commit is unknown. Scopelet is frozen from `3a3a60b` (0.3.0).

The [declared protocol](current-comparison-plan.md) fixes identical ordinary task prompts, immutable synthetic fixtures, external acceptance checks, seed 20260910 and two repetitions per main cell. Scopelet uses automatic default hooks; RTK uses release Codex awareness instructions and its Claude PreToolUse hook; Headroom uses a token-mode proxy plus recovery MCP. Global skills/configuration are excluded and installations are scoped to the temporary workspace.

Headroom received requests in every measured Claude session. Codex/Headroom remains **unmeasured** because the portable adapter rejects its unverified authenticated routing. This is an adapter/environment limitation, not a compatibility verdict about upstream Headroom.

## Whole-session results

Numbers are arithmetic means of logical input plus output tokens. Cached input is included; reasoning is already part of output and is not added again. Percentages compare each task with its own native control.

### Codex / Luna low

| Task | Native | Scopelet | RTK | Headroom |
|---|---:|---:|---:|---:|
| Small edit | 101,980 | 74,216 (-27.2%) | 102,860 (+0.9%) | unmeasured |
| Noisy command | 158,407 | 107,566 (-32.1%) | 175,785 (+11.0%) | unmeasured |
| JSONL aggregation | 90,974 | 74,926 (-17.6%) | 128,287 (+41.0%) | unmeasured |
| Equal task mix | 117,120 | 85,570 (-26.9%) | 135,644 (+15.8%) | unmeasured |

| Task | Native repetitions | Scopelet repetitions |
|---|---|---|
| Small edit | 108,810 / 95,149 | 67,681 / 80,752 |
| Noisy command | 138,737 / 178,077 | 124,707 / 90,424 |
| JSONL aggregation | 96,142 / 85,807 | 81,865 / 67,988 |

### Claude Code / Haiku

| Task | Native | Scopelet | RTK | Headroom |
|---|---:|---:|---:|---:|
| Small edit | 145,864 | 146,146 (+0.2%) | 132,592 (-9.1%) | 98,766 (-32.3%) |
| Noisy command | 257,160 | 261,660 (+1.8%) | 269,412 (+4.8%) | 249,326 (-3.0%) |
| JSONL aggregation | 205,338 | 204,743 (-0.3%) | 203,829 (-0.7%) | 206,502 (+0.6%) |
| Equal task mix | 202,787 | 204,183 (+0.7%) | 201,944 (-0.4%) | 184,864 (-8.8%) |

| Task | Native repetitions | Scopelet repetitions |
|---|---|---|
| Small edit | 132,249 / 159,479 | 133,966 / 158,326 |
| Noisy command | 216,512 / 297,807 | 315,336 / 207,984 |
| JSONL aggregation | 180,693 / 229,983 | 180,851 / 228,635 |

### Duration and available cost estimates

All six main sessions per arm are included. Missing fields stay unknown. Claude’s list-basis cost estimates are not invoices or subscription charges.

| Host | Arm | Mean seconds | Total reported USD |
|---|---|---:|---:|
| codex | native | 46.2 | unknown |
| codex | scopelet | 29.0 | unknown |
| codex | rtk | unknown (one lost duration) | unknown |
| claude | native | 23.8 | 0.4042 |
| claude | scopelet | 25.1 | 0.3920 |
| claude | rtk | 22.0 | 0.3962 |
| claude | headroom | 31.1 | 0.5138 |

On Haiku, Scopelet’s reported cost is 3.0% below native and Headroom’s is 27.1% above native, despite Headroom using 8.8% fewer logical tokens. Cache accounting and provider pricing make token reduction and cost reduction distinct outcomes; these small samples do not establish a stable cost ranking. Codex cost is unavailable.

### Targeted follow-ups

The predeclared rule selected the task with Scopelet failures first, then the largest Scopelet/native token ratio for each host. No runtime defect was reproduced, so the released and candidate executables have **identical SHA-256 hashes**. These eight sessions measure additional variability; their differences are not improvements from a code change. They are separate from the main comparison.

| Host / task | Release repeats | Identical candidate repeats |
|---|---|---|
| codex / JSONL aggregation | 67,617 / 86,430 | 97,083 / 95,910 |
| claude / Noisy command | 312,848 / 248,248 | 281,153 / 409,403 |

## Local compression and recovery

Eight shared content fixtures, five warmups and thirty repetitions in each application-cache state. The table shows byte reduction, **not model tokens**. Headroom uses its public library with ML disabled, recent-message protection zero and analysis protection disabled. RTK `read` uses its documented full-content default.

| Input | Scopelet v2 | Headroom deterministic library | RTK read |
|---|---:|---:|---:|
| small | 0.0% | 0.0% | 0.0% |
| diagnostics | 97.1% | 0.0% | 0.0% |
| repeated_errors | 85.6% | 98.7% | 0.0% |
| json | 97.7% | 30.2% | 0.0% |
| jsonl | 97.7% | 0.0% | 0.0% |
| hidden_receipt | 97.1% | 0.0% | 0.0% |
| giant_unicode_line | 0.0% | 0.0% | 0.0% |
| crlf | 97.1% | 0.0% | 0.0% |

RTK’s specialized `test` wrapper, checked separately, reduces the synthetic failing command by **99.6%**, preserves both diagnostic facts and returns the original exit code 7. The default RTK hook does not rewrite `python3 checks.py`; its dry-run checks do rewrite `git status` and `cat records.jsonl`. Manual `rtk test` and automatic hook coverage are different treatments.

Scopelet original-byte recovery passes for all eight inputs, including CRLF and large integers. The padded receipt is absent from the compact view and is recovered from the immutable source blob. Small input and a giant uncompressible Unicode line pass through unchanged. For other tools, an unverified byte-roundtrip field does not establish semantic loss: a factored representation can preserve values without preserving original formatting.

The [local engine measurements](../bench/results/current-engine-2026-09-10.json) cover 16 workload/cache cells, 30 repetitions and three configurations: the release at v2, the identical binary at v1, and the identical binary at v2. All 1,440 measured process invocations succeed; the 480 matched v2 comparisons have no output differences. This is a release characterization/control, not an optimization benchmark. Cold means empty application cache, not flushed OS cache. Other agent activity ran on the machine; timing and memory are descriptive. The shared-content timer includes CLI startup for Scopelet/RTK but calls an imported Headroom library, so it is not an equivalent engine-speed ranking.

## Confirmed bugs and validation

- Missing Claude cache fields could leave session input unknown while reporting `usage_missing=false`; complete per-model cache accounting was also ignored. The normalizer now uses complete fallback accounting or marks usage unknown.
- The external JSONL grader accepted floating-point counts through Python numeric equality. It now requires integer counts; all retained JSONL workspaces were regraded and still pass.
- Trace analysis missed checks executed inside Scopelet’s automatic shell-wrapped `&&` chains. The corrected parser is replayed against retained transcripts during export.
- The Headroom adapter hardcoded a historical source commit regardless of the supplied installation. Provenance is now explicit or unknown; missing or boolean proxy request counters cannot certify routing. All seven retained Headroom sessions also pass this stricter check.
- Local engine comparisons assumed a v1 baseline even for the released v2 default. `--baseline-version 2` selects matched v2 outputs and mismatches fail the command.
- Optional RTK SQLite telemetry raised on this host’s Python reader and lost one completed grade. Optional counters now return an explicit unknown; completed grades are persisted before archival, and available paid usage is recovered after postprocessing failures. The failed attempt is retained with 174,740 reported tokens and no invented grade or duration. Subsequent RTK runs retain their grades even when database counts are unavailable.

Validation: `cargo fmt --check`, Clippy with warnings denied, 93 Rust integration tests, 93 Python benchmark tests, nine Node tests and skill validation pass. The Rust core and installable skill bundle are unchanged. New live attempts require `--live`, share a locked 60-attempt ledger, and never overwrite or retry an existing attempt. Exports verify transcript hashes and reject private paths.

Two invalid preparatory measurements are [retained separately](../bench/results/current-preparation-2026-09-10.json): an incompatible cached 0.1.0 control, and an initial recovery adapter that compared serialized artifact JSON with source bytes. Both were corrected before the final local reports and are excluded from savings claims.

## Where Scopelet is useful

The useful distinction remains recoverable, bounded command output plus exact local queries. On these Luna tasks Scopelet reduces session tokens, especially for noisy commands. Haiku results are much closer to native and vary by task; Headroom uses fewer logical tokens on the measured small edits, but its total reported Haiku cost is higher than native. Existing native tools remain appropriate for small known inputs, and RTK’s specialized command filters can be very effective when they cover the command.

Caveman and Ponytail are reviewed as instruction-level alternatives, not measured in this paid matrix. Their current upstream heads remain [Caveman `15581d1`](https://github.com/juliusbrussee/caveman/tree/15581d14007fd01fb3f132016741962f34936ca2) and [Ponytail `356918e`](https://github.com/DietrichGebert/ponytail/tree/356918eba965ee1eac64bd3a7f0dd02108350de5), matching the earlier [competitive review](competitive-review-2026-09-09.md). Their prose/task-steering effects cannot be ranked using compression byte counts.

[Per-session evidence](../bench/results/current-comparison-2026-09-10.json) · [Shared-content evidence](../bench/results/current-content-2026-09-10.json) · [Reproduction commands](../bench/README.md#current-automatic-comparison-030-luna--haiku)
