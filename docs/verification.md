# Verification

Status: pre-release verification is in progress. This page is updated with the
completed agent campaign and CI results before the release is published.

Independent reviews used Claude Fable 5.1 high, Claude Opus 5 high, and separate
Codex agents, including GPT-5.6-Luna. Reproductions led to fixes for transactional
JSONL ingestion, multiline search, process drainage, capture limits, oversized
pagination, output envelope budgets, cache cleanup, adapter spans and document
freshness. Reviews are fallible; the regression tests record the concrete claims.

There are 42 Rust integration tests, 11 offline benchmark-harness tests and 3
Node launcher tests. Actual codeindex and webindex adapter smoke tests are also
provided in `scripts/check_adapters.py`. The standalone skill validator checks
bundle structure, links, version agreement and the included MIT license.

The experiment has three deliberately small tasks: finding and fixing a cache
expiry boundary through its application call path; aggregating 1,000 JSONL
records; and fixing a known small whitespace-normalization function. Each task
has a pristine external grader. The JSONL source must remain unchanged. A visible
shape check does not reveal its expected counts.

Session totals include agent instructions and repeated input across turns. Codex
reported input includes cached input; Claude logical input is the sum of uncached,
cache-read and cache-creation input. Output includes reasoning where supplied;
reasoning is never added again. Missing metrics remain unknown. One run per cell
cannot establish statistical significance or production task generalization.
