# Compact-v3 candidate protocol — 2026-09-10

Declared before any provider call. Compare three arms on Claude Code with Haiku
(`claude-haiku-4-5-20251001`, default effort): `native` (no hooks), `released`
(frozen 0.3.2 binary, compact-v2 default) and `candidate` (this branch,
compact-v3 default). Both binaries are copied and hashed before the first
session; the harness files are hashed too.

Budget: 18 attempted sessions at most (three tasks, three arms, two
repetitions), timeout 600 seconds, sequential, no automatic retry. Any quota
rejection or missing usage stops the campaign; the attempt stays in the ledger.
Expected cost is about twelve hook-instrumented Haiku sessions at roughly
$0.07 each plus six native ones, under $1.50 in total.

Tasks are the existing immutable fixtures: `task4` (noisy diagnostic output),
`task2` (exact JSONL aggregation) and `task3` (small edit, known to be dominated
by prompt noise). Seed 20260910 shuffles the matrix. Prompts are identical
across arms; hook arms add only Scopelet's own session context. Hooks are
installed by `install --agent claude` into a fixture-scoped
`CLAUDE_CONFIG_DIR`, passed explicitly with `--settings`; global skills and
slash commands are disabled; every Scopelet store is scoped to the fixture.
Only the existing provider authentication is shared.

Report per session: functional acceptance, exit code, timeout, duration,
logical input plus output tokens (cache included) with the separate usage
fields, reported provider cost, hook lifecycle events and compact outputs seen
by the model. Failures stay in the sample. Unknown usage is unknown, never zero.
Two repetitions are exploratory: every value is shown and no significance is
claimed.

Go/no-go, fixed here: **go** when no candidate attempt fails functionally and
the candidate mean of logical input plus output tokens over `task4` and `task2`
combined is at most the released mean; `task3` is reported without a
criterion. An undecidable campaign (missing cells or usage) is reported as such,
not rounded to a verdict.

Offline evidence accompanies the live result: the content probes
(`bench/content.py`, shared fixtures, Headroom 0.37.0 and RTK 0.42.4 for
context) and the engine measurements (`bench/performance.py`, frozen 0.3.2 as
baseline pinned to compact-v2, candidate measured at compact-v3, `candidate_v2`
proving v2 byte identity). Raw transcripts and installation paths stay
private; `bench/export_candidate.py` publishes the inspected summary.
