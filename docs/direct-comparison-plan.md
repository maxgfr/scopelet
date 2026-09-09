# Direct comparison pilot protocol

Declared before the direct competitor campaign on 2026-09-09. The question is
whether Scopelet's complete agent workflow uses fewer model-reported session
tokens while completing the same task. This pilot cannot establish superiority
across workloads or a production quality margin.

## Matrix

Use Codex with GPT-5.6-Luna, low reasoning, and Claude Code with Haiku 4.5. Keep
the same model and reasoning configuration across treatments for each agent.
Run a native baseline, a concise-native control, Scopelet default, Scopelet ultra,
Caveman's full skill, Ponytail's skill, RTK and Headroom. Following the user's
request, add Scopelet ultra combined with Caveman's full communication skill
before starting measurements: nine arms, three tasks and two agents = 54 runs.
Begin with the noisy
command task, then include exact JSONL aggregation and a tiny known-file edit.
Use one repetition per cell for this pilot, seeded shuffled order, and retain
failures. Additional repetitions must be labeled separately.

Pin upstream revisions from the [competitive review](competitive-review-2026-09-09.md),
freeze native binaries and skill bundles, and record hashes. Use the published
Scopelet 0.1.1 binary. Disable discoverable host skills in Codex where supported;
Claude uses project settings. Host instructions may still affect behavior.

## Integration boundaries

Caveman and Ponytail use their full project-local skills and explicit native
activation. RTK uses documented manual command prefixes and recovery, without
installing global rewriting hooks. Scopelet similarly receives explicit command
guidance. Headroom uses a genuine local proxy in token mode, with per-process
agent configuration and retrieval support; its default cache mode optimizes a
different tradeoff. If an integration cannot run faithfully with current account
authentication, report it as unmeasured rather than assigning zero savings or
calling another tool the winner.

## Evidence and interpretation

Reuse immutable external task graders and original-fixture hashes. Check actual
tool/skill/proxy adoption separately from task success. Publish logical input
plus reported output, cache fields, failures and latency per agent and task.
Do not turn byte estimates into tokens or tokens into currency without verified
pricing. Inspect raw command traces for unexpected retries, missed activation
and omitted required checks. Keep private transcripts local and publish sanitized
accounting with hashes.

Compare like cells; do not combine the two agents into a global score. A lower
number on one synthetic task is an observed pilot result, not a reliable ranking.
The [scientific review](scientific-review-2026-09-09.md) describes the repeated,
multi-repository evaluation needed for stronger claims.

## Declared follow-up: bounded reads and explicit modes

After inspecting the completed 51-run pilot, test a revised Scopelet skill against
contemporaneous concise-native controls: the same three tasks and two agents,
two repetitions per cell, 24 runs, shuffled seed 20260910. This is a separate,
post-pilot campaign. Freeze its binaries and skills independently. The changes
require a bounded schema preview before aggregation, explicit `--mode ultra` on
query/spec/run calls, a shared context flag for alternative search patterns and
native handling of short tests. Retain the existing evidence/recovery safeguards.
The runtime query engine is unchanged. Check actual modes, premature bulk reads,
required checks and correctness alongside whole-session token usage. Compare
candidate and control within this campaign; comparisons to the original one-run
pilot are exploratory and do not isolate the instruction change causally.

## Post-follow-up cache correction

The 24-run instruction follow-up completed before the final 0.1.2 runtime change.
Independent trace review found that a Codex broad `rg --hidden` re-ingested
139,464 characters from workspace-local cache entries. Add local ignore markers
inside storage subdirectories, preserving existing rules. Validate with a
deterministic replay of the offending search, original-byte recovery, regression
tests and final installed-agent smokes. Keep the original follow-up measurements
and binary hashes: they do not measure this later cache correction.
