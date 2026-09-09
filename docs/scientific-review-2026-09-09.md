# Scientific review: where Scopelet should save tokens

Reviewed 9 September 2026 against primary papers and Scopelet's current design.
This is an engineering assessment, not a systematic literature review or an
independent replication. Published percentages below belong to the papers'
experiments; none establishes a Scopelet saving.

## What the evidence supports

**Compressing prompts can help, but extraction is still lossy.** LLMLingua-2
trains a bidirectional token classifier on distilled compression examples,
primarily from meeting transcripts. Its LongBench table reports 42.4 versus
44.0 for original prompts while using roughly one third as many tokens.
Performance varies by task: extraction alone does not guarantee preservation of
meaning, even when the retained words are verbatim. The authors acknowledge the
training-domain limitation. For Scopelet, an optional document compressor could
be evaluated separately, but it would introduce model inference and a new
quality boundary. The paper does not justify deleting tokens inside source
code, exact identifiers, structured values, or commands.
[LLMLingua-2, Tables 2–4 and Limitations](https://arxiv.org/html/2403.12968v2)

**Task-aware selection is more relevant than universal compression ratios.**
SWE-Pruner takes an explicit goal hint and uses a 0.6B neural skimmer to retain
relevant code lines. It reports 23–38% token reduction on SWE-Bench and 29–54%
on SWE-QA. However, its own structural evaluation shows Function RAG's AST
correctness falling from 92.3% to 87.3% after adding the pruner. Whole-line
selection is therefore a useful compromise, not proof of intact syntax or
semantics. Its implementation focuses on Python repositories. Our inference:
Scopelet should prefer complete meaningful units and make the recovery path
cheap; deterministic keyword ranking needs adversarial tests for dependencies
that never repeat the query terms.
[SWE-Pruner, §§3–5, Limitations and Appendix H](https://arxiv.org/html/2601.16746v3)

The newer SWE-Pruner Pro reports up to 39% fewer prompt and completion tokens
using a pruning head attached to the coding model's internal representations.
That is a different integration boundary: a portable skill invoking hosted
Codex or Claude cannot implement the described hidden-state extraction by
changing its instructions. It is relevant to a future self-hosted backend,
not a drop-in Rust dependency for the present release.
[SWE-Pruner Pro, §§3–5 and Appendix E](https://arxiv.org/html/2607.18213v1)

**External computation is established prior art and depends on agent behavior.**
Recursive Language Models keep the input in an external environment and let
the model inspect and compute over it, optionally calling submodels. This
supports Scopelet's existing artifact-and-query architecture, without making
that architecture scientifically novel. RLM reports long-tailed costs and
model-dependent scaffolding failures; its negative results include weak coding
models struggling with the environment and reasoning models exhausting output
budgets. Our inference: fewer observed bytes are insufficient when an agent
spends extra turns constructing queries or repeatedly recovering evidence.
Track those recovery trajectories and include competent shell computation as a
control.
[Recursive Language Models, §3 and Appendices B/F](https://arxiv.org/html/2512.24601v2)

**Caveman-style brevity deserves a separate experiment.** Chain of Draft elicits
short intermediate drafts and reports substantial output reductions on
arithmetic, commonsense and symbolic reasoning tasks, including experiments
with Claude 3.5 Sonnet. Those settings do not establish effects on today's
coding agents, hidden reasoning tokens, or lengthy debugging trajectories.
Its GSM8K table also shows a quality tradeoff: Sonnet's accuracy drops from
95.8% with Chain of Thought to 91.4% with Chain of Draft; the zero-shot gap is
larger. Shorter reasoning is therefore not a safe default inferred from this paper.
Our inference: borrow concise communication first—omit repeated narration and
state decisions, evidence and unresolved limits succinctly. Preserve negation,
quantities, units, uncertainty and exact technical text. Do not silently force
shorter model reasoning or apply grammatical deletion to tool evidence. Test
this instruction independently of Scopelet's CLI so any benefit is attributed
to the right mechanism.
[Chain of Draft, §§3–4](https://arxiv.org/html/2502.18600v2)

**Caching changes the cost question.** Prompt Cache reuses attention states for
specified prompt modules to reduce inference latency. This is server-side
attention reuse, distinct from Scopelet's immutable local evidence store.
Consequently, a local cache hit proves neither a provider cache hit nor fewer
billed tokens. Our inference: stable observations may aid reuse, but rewriting
an already cached conversation could have a different cost tradeoff from
avoiding a new large observation. Measure logical input, cached input,
uncached input, output and latency separately; derive currency cost only when
the provider's applicable prices and usage fields are known.
[Prompt Cache, §§2–4](https://arxiv.org/html/2311.04934v2)

## Ranked engineering decisions

1. **First: selective adoption and recovery.** Keep direct tools for small known
   files. Use Scopelet for large evidence or local aggregation. Test bounded
   whole-unit selection with explicit omissions, including recovery after a
   deliberately misleading first query. Optimize full-session cost, including
   instruction loading and retries.
2. **Next: concise-prose ablation inspired by Caveman.** Add a minimal instruction
   variant, preserve technical fidelity, and compare it alone and combined
   with the CLI. Treat any change to reasoning effort as a separate experiment.
3. **Then: better deterministic retrieval.** Compare existing rank/search with
   whole-unit BM25 and symbol-aware neighborhoods under identical budgets.
   Investigate whether their additional local complexity reduces recovery
   calls. Neither retrieval technique is itself a novelty claim.
4. **Later: optional learned selection.** Evaluate a local neural selector only
   after deterministic baselines, counting model startup, memory, latency and
   inference cost. Keep learned pruning opt-in until quality evidence supports
   changing defaults.

## Evaluation required before stronger claims

Freeze binaries, skills, competitor commits, model identifiers, prompts and
fixtures. Compare native tools, an equally concise optimized-shell skill,
concise-prose-only, Scopelet default/ultra, and applicable competitor modes.
Keep integration differences explicit rather than claiming every tool is
interchangeable. Randomize paired run order and separate warm-cache from cold
conditions where the harness can control them.

Use multi-repository bug fixes, noisy command output, exact aggregation, tiny
edits, missing evidence, cross-file negation and semantic questions with weak
keyword overlap. Grade with immutable external tests and gold evidence where
available. Count failures, timeouts, non-adoption, recovery calls and false
completeness claims. Report quality and cost together with uncertainty over
repeated task-level pairs; several runs of four fixtures remain a pilot.
Preregister a quality margin and sufficient task coverage before calling the
default noninferior. Equal-budget baselines matter: more elaborate reasoning
can appear superior simply because it receives more computation.
[Reasoning in Token Economies, EMNLP 2024](https://aclanthology.org/2024.emnlp-main.1112/)
