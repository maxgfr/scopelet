# Scopelet: research basis and evaluation limits

Scopelet is a proposed local Rust CLI plus an installable agent skill. It composes deterministic operations—counting, filtering, grouping, and selecting exact source excerpts—before evidence enters an agent's context. Version 1 uses no auxiliary model. The papers below motivate parts of this design; they do not prove Scopelet saves tokens or preserves task quality. This project makes no claim of scientific novelty.

## Recursive Language Models

RLM stores a prompt and intermediate results in an external execution environment. The root model operates through symbolic handles, bounded observations, and optional recursive model calls. Its no-subcall ablation is particularly relevant: on CodeQA, GPT-5 scores 58 versus 62 with recursion, while Qwen3-Coder scores 66 versus 56. Recursive calls help more on dense semantic aggregation. These are results from the paper's scaffold and benchmarks, not evidence about Scopelet. [Primary paper, January 2026 revision, §§2–4](https://arxiv.org/html/2512.24601v2)

Transferable mechanisms are keeping bulk evidence outside context, computing over it locally, and selectively exposing exact evidence. A skill cannot retroactively remove a user prompt already ingested by its host. Deterministic selection cannot substitute for semantic reading when the answer depends on most of the input. The authors also report worse performance on small inputs and substantial cost variance from long trajectories. Scopelet borrows external evidence handling; it does not implement recursive model inference. [Primary paper, §§4–6](https://arxiv.org/html/2512.24601v2)

## ReWOO

ReWOO separates planning, tool execution, and answer synthesis. A planner emits operations referencing evidence variables; a worker resolves those dependencies; a solver receives the assembled evidence. This reduces repeated prompting with intermediate observations. The paper reports fivefold token efficiency and a four-percentage-point accuracy improvement on HotpotQA. [Primary paper, §§2–3](https://arxiv.org/pdf/2305.18323)

The transferable mechanism is local execution of foreseeable dependent operations without returning every intermediate result to the agent. The full method still includes planner and solver model invocations, and some tools use models. Its authors identify poorly known environments as a limitation: planning everything upfront can require enumerating possible outcomes. Scopelet should compose deterministic operations while retaining adaptive follow-up when observations change the investigation. [Primary paper, §§2, 4](https://arxiv.org/pdf/2305.18323)

## CodeAct

CodeAct represents actions as executable Python, supporting control flow, tool composition, intermediate variables, and revision after execution feedback. Its evaluation across 17 models reports up to 20 percentage points higher success and 30% fewer actions on its complex benchmark. [Primary paper, §§1–2](https://arxiv.org/html/2402.01030v4)

The transferable mechanism is computing before printing: compose filters, counts, and grouping locally, then return useful evidence. Its comparison concerns text and JSON action formats, not competent shell pipelines. Fewer actions do not establish lower total token use. CodeAct therefore supports local composition as a design choice, but cannot establish an advantage over `rg`, `jq`, `awk`, or Python. Executable composition is established prior art. [Primary paper, §§1–2](https://arxiv.org/html/2402.01030v4)

## Product implications

The proposed differentiation is a consistent evidence interface with explicit coverage accounting. Results should identify searched scope, selected evidence, exclusions, and omitted material, and provide a route to exact source excerpts. Counts and grouping should operate on the full selected input before any display budget is applied.

- **Default:** favor complete requested evidence and reversible local transformations. Make limitations visible; do not promise quality preservation before evaluation.
- **Ultra:** select context aggressively within an explicit budget. Report omissions and partial coverage so a short answer cannot silently imply an exhaustive search.
- **Optional external indexes:** `codeindex` or `webindex` may supply inputs when available. Their retrieval quality, freshness, and costs remain separate dependencies; version 1 should function without them or additional model calls.

Handles, filtering, batching, excerpt extraction, caching, and token budgets are individually established techniques. Any useful advantage must come from their measured implementation and agent usability.

## Falsifiable comparisons

Compare native shell use, an equally concise skill teaching optimized shell pipelines, Scopelet default, and Scopelet ultra. Use the same inexpensive model and task budgets, with repeated paired runs. Include overlapping searches, counts across large inputs, grouping, cross-file conditions, zero matches, excluded paths, stale evidence, and misleading keywords.

Measure task correctness, evidence recall, false completeness claims, total input/output tokens including skill instructions and recovery, tool calls, and latency. Report cached-token accounting separately when available. Preregister a quality noninferiority threshold for default and report ultra's measured quality/cost tradeoff. If gains disappear against the optimized-shell skill, do not claim that the CLI improves efficiency. No savings figure is established by this research note.
