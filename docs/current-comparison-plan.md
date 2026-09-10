# Scopelet 0.3.0 current comparison protocol — 2026-09-10

Declared before new provider calls. Compare automatic default Scopelet, native,
RTK 0.48.0 and Headroom 0.37.0. Freeze release binaries and Python distribution
metadata. Luna (`gpt-5.6-luna`, low) on Codex; Haiku
(`claude-haiku-4-5-20251001`, default effort) on Claude Code. Never pool agents.

Global budget: 60 attempted sessions, including preflights and follow-ups;
timeout 600 seconds, no automatic retries. Four planned preflights (Scopelet
and Headroom per agent), 48 main cells (two repetitions, three tasks, four
arms, two agents), eight follow-up cells. Unsupported integrations are recorded
as unmeasured before spending sessions. The portable Headroom adapter still
rejects Codex; its six main cells and preflight are not replaced by native runs.

Main tasks are the existing immutable `task3` small edit, `task4` noisy diagnostic
and `task2` exact JSONL aggregation. Seed 20260910 shuffles the matrix. Prompts
are identical within each task; default Scopelet hooks add their own guidance.
RTK uses its release's Codex awareness document and Claude PreToolUse hook.
Headroom uses token-mode proxy and recovery MCP, with routing counters required.
Claude skills are disabled; Codex user config and installed skills are disabled.
All settings and data stores are scoped to the fixture, except existing provider
authentication. No global installations are changed. RTK's global audit writer
is disabled; hook events and per-run database counts report adoption.

Report functional acceptance, exit/timeout/model errors, logical input plus
output (cache included), separate cache fields, duration, available provider
cost estimates, tool/hook activation and missing usage. Failures stay in the
sample and cost-per-success denominator. Unknowns never become zeros. Two
repetitions are exploratory; show every value and do not claim significance.

Follow-ups select each agent's task by Scopelet functional failures first, then
greatest mean token ratio versus native (ties: task3, task4, task2). Compare frozen
release and corrected candidate twice. If no runtime fix is justified, candidate
is an identical binary and these are explicitly additional variability samples.
Offline corrections do not justify claiming a new live-tested runtime.

Offline: five warmups, thirty repetitions, application-cache cold/warm (OS cache
not flushed). Cover small output, logs, JSON/JSONL, aggregation, repository query,
recovery and size boundaries. Compare competitors only on shared capabilities;
verify preserved facts and recovery separately from output size. Original bytes,
explicit omissions and exit status are mandatory. Publish inspected summaries;
raw transcripts, provider state and installation paths remain private.
