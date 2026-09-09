# Competitive review, 2026-09-09

Scopelet has a useful, narrower position: request a computed result or selected
evidence through one typed local pipeline. The four comparators optimize
different parts of an agent session. Their advertised percentages do not
establish which tool saves more on the same task. No five-way live benchmark was
run for this review.

Fresh shallow clones pinned the following upstream revisions. Scopelet was read
at `bd87f443a461462fff850a21491122c575db8533`, including its follow-up changes.

| Project | Inspected commit | Main intervention |
|---|---|---|
| [Caveman](https://github.com/JuliusBrussee/caveman/tree/15581d14007fd01fb3f132016741962f34936ca2) | `15581d1` | Concise-output skill; separate compression/retrieval engine and proxy |
| [Headroom](https://github.com/headroomlabs-ai/headroom/tree/e67b3c8a29443a60d6b0018fb22f525c5cd7e709) | `e67b3c8` | Content-specific compression, recovery, API integration and cache-aware processing |
| [Ponytail](https://github.com/DietrichGebert/ponytail/tree/356918eba965ee1eac64bd3a7f0dd02108350de5) | `356918e` | Reduce unnecessary implementation and explanation through instructions |
| [RTK](https://github.com/rtk-ai/rtk/tree/8e9aa04cb2afb189747fac4e36bec2254ddd0564) | `8e9aa04` | Specialized command-output filters and agent integration |

## Features and evidence

| Tool | Verified strength | Evaluation inspected | Scopelet implication |
|---|---|---|---|
| Caveman | Explicit prose-fidelity rules; whole-record recovery, BM25 ranking and gap markers | Skill output-token experiments with a short-instruction control; retrieval regression tests | Adopt clarity safeguards and the control design. Recoverability is established prior art. |
| Headroom | JSON selection combines anchors, outliers, query matches and relevance; recoverable compression | Rust CCR round-trip tests; cache/prefix and agent-session benchmark programs | Stronger automatic content routing. Scopelet depends on the agent choosing the useful computation. |
| Ponytail | Prefer existing code, standard libraries and root-cause fixes; preserve requested functionality | Real Claude sessions, deterministic adversarial graders, completeness and over-engineering judges | A smaller answer or diff is insufficient. Keep independent functional grading and negative results. |
| RTK | Many command-specific parsers; failure/truncation recovery; integration for both agents | Byte-accuracy and faithful-search integration tests; local token-estimation implementation | Generic capture does not provide RTK's command-specific understanding or integration breadth. |
| Scopelet | Composable filter/project/group/count/search; immutable snapshots; explicit scan/display completeness | [Correctness and live-session report](verification.md), including unfavorable outcomes | Useful for bulk aggregation and some noisy commands; small tasks can cost more. |

Headroom's [selection implementation](https://github.com/headroomlabs-ai/headroom/blob/e67b3c8a29443a60d6b0018fb22f525c5cd7e709/crates/headroom-core/src/transforms/smart_crusher/planning.rs)
preserves signals before pruning to budget. Its [CCR tests](https://github.com/headroomlabs-ai/headroom/blob/e67b3c8a29443a60d6b0018fb22f525c5cd7e709/crates/headroom-core/tests/ccr_roundtrip.rs)
check that dropped rows remain recoverable. These are substantial overlapping
capabilities, not merely README claims. Scopelet's distinction is explicit
relational computation and source-freshness contracts, rather than recovery alone.

RTK's [tracking function](https://github.com/rtk-ai/rtk/blob/8e9aa04cb2afb189747fac4e36bec2254ddd0564/src/core/tracking.rs#L1657)
uses `ceil(UTF-8 bytes / 4)`. That is a local estimate, unlike Scopelet's reported
whole-session model usage. This observation concerns that tracker, not every RTK
benchmark. Its [diff tests](https://github.com/rtk-ai/rtk/blob/8e9aa04cb2afb189747fac4e36bec2254ddd0564/tests/diff_byte_accuracy_test.rs)
protect CRLF/LF distinctions even when explaining them takes more space. Its
[Codex integration](https://github.com/rtk-ai/rtk/blob/8e9aa04cb2afb189747fac4e36bec2254ddd0564/hooks/codex/README.md)
uses prompt guidance rather than a programmatic rewriting hook; installation
alone therefore does not prove command adoption.

## What Caveman gets right

The current [skill](https://github.com/JuliusBrussee/caveman/blob/15581d14007fd01fb3f132016741962f34936ca2/skills/caveman/SKILL.md)
protects negations, exceptions, numbers, units, technical strings and the user's
language. It rejects invented abbreviations and restores clarity when compressed
wording makes ordered steps ambiguous. It also keeps persisted documentation,
comments and issue text in ordinary prose. Scopelet already protects exact code,
errors and qualifications; making the remaining safeguards explicit is a small,
useful improvement. Broken grammar and suppressed progress updates are not
necessary to obtain these benefits.

Caveman's [retrieval implementation](https://github.com/JuliusBrussee/caveman/blob/15581d14007fd01fb3f132016741962f34936ca2/engine/retrieve_query.go)
returns whole JSON records, keeps CSV headers and multiline rows together, marks
non-adjacent selections, and falls back to the original when ranking cannot find
a useful decomposition. Its [regressions](https://github.com/JuliusBrussee/caveman/blob/15581d14007fd01fb3f132016741962f34936ca2/engine/retrieve_query_test.go)
specifically prevent attributing a field to the wrong record. Scopelet's JSON
records and provenance address part of the same problem. Native CSV/TSV querying
is a concrete capability gap, but adding it deserves full quoted-newline and
header-association tests rather than treating tables as ordinary text lines.

The [license map](https://github.com/JuliusBrussee/caveman/blob/15581d14007fd01fb3f132016741962f34936ca2/LICENSING.md)
separates the MIT skill from the BSL engine. Scopelet should continue implementing
its own MIT runtime; linking or copying the engine is not equivalent to reusing
an MIT skill. This is a dependency decision, not a runtime-quality comparison.

## Verification gaps and next experiments

Caveman's [evaluation](https://github.com/JuliusBrussee/caveman/blob/15581d14007fd01fb3f132016741962f34936ca2/evals/README.md)
compares against a plain concision instruction and explicitly excludes fidelity
and full-session economics from its conclusions. Add that control to Scopelet:
native tools, concise native tools, Caveman skill, Scopelet, and their combination.
This separates the effect of shorter prose from local evidence computation.

Ponytail's [agentic harness](https://github.com/DietrichGebert/ponytail/blob/356918eba965ee1eac64bd3a7f0dd02108350de5/benchmarks/agentic/README.md)
includes short YAGNI controls and adversarial checks. During this review,
`python3 benchmarks/agentic/run.py --selftest` exited successfully: known-good
references passed, known-bad references were rejected, and timeout handling was
checked. That verifies its instruments locally; it does not independently
reproduce its published model results. Other upstream suites were inspected,
not executed wholesale.

Before ranking tools, repeat equivalent tasks in randomized arm order with pinned
versions, native skill activation, equal tool permissions and external graders.
Report input/output/cache usage, adoption, recovery calls, failures, latency and
spread. Include tiny tasks, sparse relevant evidence, exhaustive aggregates,
misleading near-matches and ambiguous error attribution. An automatic selection
rule should then be justified by those results: expected saved reading must
outweigh skill, command and recovery overhead. Current evidence supports selective
use and tested recovery contracts; it does not establish universal savings or
production readiness for every workload.
