# Direct comparison pilot protocol

Declared before the direct competitor campaign on 2026-09-09. The question is
whether Scopelet's complete agent workflow uses fewer model-reported session
tokens while completing the same task. This pilot cannot establish superiority
across workloads or a production quality margin.

## Matrix

Use Codex with GPT-5.6-Luna, low reasoning, and Claude Code with Haiku 4.5. Keep
the same model and reasoning configuration across treatments for each agent.
Run a native baseline, a concise-native control, Scopelet default, Scopelet ultra,
Caveman's full skill, Ponytail's skill, RTK and Headroom. Begin with the noisy
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
