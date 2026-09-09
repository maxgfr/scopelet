# Token and session cost optimization research

Date: 2026-09-09

Scope: practical mechanisms relevant to Scopelet's Rust local CLI and Claude
PostToolUse/Codex PreToolUse wrappers. Sources were limited to upstream source,
first-party documentation, and the LLMLingua-2 paper. No live model or API
calls were made.

## Findings

### Deterministic output shaping is the closest fit

Headroom describes a local compression layer for tool outputs, logs, files and
RAG chunks, with content-aware compressors and a reversible path back to the
original. Its current architecture documentation says context management is
“live-zone-only”: content is compressed in place while the message list and
older turns remain intact. It also protects system messages and a configurable
number of recent messages. These are design claims from the project, not an
independent measurement. The project README advertises 15–20% fewer tokens for
coding agents and 60–95% fewer for JSON; those figures are project marketing
and should not be treated as session-cost evidence without an equivalent-task
study. [Headroom README](https://github.com/headroomlabs-ai/headroom),
[Headroom architecture](https://github.com/headroomlabs-ai/headroom/blob/main/docs/content/docs/architecture.mdx),
[Headroom context management](https://docs.headroomlabs.ai/docs/context-management)

RTK's upstream repository documents a simpler hook/proxy model: tool-aware
filters for common commands, repeated-line deduplication, head/tail limits,
and a small-output bypass. Its source-side instructions explicitly say that
the advertised 60–90% is bash-output reduction, not billed-token reduction,
and that its tracking uses a `bytes / 4` estimate rather than a tokenizer.
That distinction supports Scopelet's existing rule that byte reduction is only
a proxy. RTK's documented “same file + same content” cross-turn marker is a
useful possible optimization, but it needs an immutable reference and a clear
retrieval path to avoid making a later tool result unusable. [RTK source
instructions](https://github.com/rtk-ai/rtk/blob/develop/.github/copilot-instructions.md)

The strongest common pattern is therefore deterministic, tool-aware selection
with a cheap bypass for small outputs, and an explicit escape hatch to the
original. It is compatible with Scopelet's immutable snapshots and its policy
of changing only the newest tool output. This is an inference from the source
designs and Scopelet's contracts, not a claim that either project improves
answer quality for every workload.

### Model-assisted token compression is a different product boundary

Microsoft's LLMLingua-2 paper formulates compression as token classification
using a bidirectional encoder trained through data distillation from GPT-4,
rather than relying only on causal-model entropy. On its evaluated datasets,
the paper reports 2x–5x compression ratios, 1.6x–2.9x end-to-end acceleration,
and 3x–6x faster compression than earlier methods. Those are benchmark results
on MeetingBank, LongBench, ZeroScrolls, GSM8K and BBH; they do not establish
session cost or faithfulness for arbitrary shell/tool output. The implementation
also requires a trained model/runtime. [LLMLingua-2 paper](https://arxiv.org/abs/2403.12968),
[Microsoft LLMLingua repository](https://github.com/microsoft/llmlingua)

Because Scopelet cannot call an extra model and does not vendor an external
compressor, LLMLingua-2 should remain background evidence, not a near-term
implementation target. Token deletion does not inherently prevent recovery: an
implementation could retain the original plus a stable mapping. The primary
Scopelet objections are the added model/runtime dependency and unvalidated
fidelity on arbitrary shell/tool output; any future experiment would need to
measure those costs and preserve the mapping.

### Provider prompt caching reduces billed input work only for stable prefixes

The current OpenAI API guide documents GPT-5.6 and later separately from older
models. For GPT-5.6+, a visible prefix must reach 1,024 tokens; implicit or
explicit caching is available, and `prompt_cache_options.ttl` supports a `30m`
example. Cache writes cost 1.25× the uncached input rate and reads 0.1×; usage
reports `cached_tokens` and `cache_write_tokens` in
`input_tokens_details`. The guide says cached-token reporting for these models
uses the exact eligible boundary and excludes hidden tokens; the older-model
128-token rounding rule should not be generalized to GPT-5.6+. Earlier models
have different implicit-breakpoint and retention behavior, including legacy
`in_memory`/`24h` controls. This is API documentation only and says nothing
about Luna subscription billing; Scopelet's wrappers do not see or control API
request prefixes today. [OpenAI Prompt Caching](https://developers.openai.com/api/docs/guides/prompt-caching)

Anthropic documents explicit or automatic `cache_control` breakpoints over the
ordered `tools`, `system`, then `messages` prefix. Cache reads are billed at a
fraction of base input price, while a 5-minute write costs 1.25x and a 1-hour
write 2x; the default TTL is refreshed on use. The docs also state that changing
content before the breakpoint, tool settings, images, or thinking configuration
can invalidate the cache, and that the cache has up to four explicit
breakpoints. Usage exposes cache creation/read counts. These details imply
that compressing or rewriting a stable prefix can reduce future cache hits even
if it shortens one request. [Anthropic Prompt Caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)

## Recommendations (at most three)

1. **Add measured session accounting to the existing experiment path.** Keep
   output bytes as a diagnostic, but compare equivalent tasks with raw and
   Scopelet-wrapped sessions and report input tokens, output tokens, cache
   reads/writes when a provider exposes them, latency, failures, and answer
   success. Preserve failed runs in the denominator. This follows the
   repository contract and avoids repeating RTK's bytes-to-token shortcut.
   A local tokenizer is justified only if it is demonstrably needed for a
   decision on the hot path (for example, a provider-specific budget that bytes
   cannot safely approximate). Otherwise, use byte thresholds for the cheap
   hook and measure real provider usage in offline/session evaluation; bundled
   tokenizer data adds startup, binary and model-tokenizer-version risk without
   proving lower billed cost.

2. **Extend deterministic, lossless structure-aware factoring carefully.**
   Prioritize repeated JSON keys/columns, repeated text lines, and exact
   cross-turn duplicate snapshots only when the view includes an unambiguous
   artifact/blob reference and retrieval remains within the same budget. Keep
   the small-output bypass and whole-record selection; never clip a JSON field
   merely to meet a byte target.

3. **Make cache-awareness an integration note and optional future adapter, not
   a local rewrite.** If a future API adapter is added, place stable tool/system
   material before changing conversation content and expose provider usage
   counters. Do not automatically compress a provider's stable cached prefix:
   cache invalidation and write charges can outweigh one-call byte savings.

## Pitfalls that should affect current code

- Do not call output-byte reduction “token savings” or infer cheaper sessions
  from a shorter final answer. Tokenization, repeated context, output length,
  cache state and task success all matter.
- The current 2 KiB passthrough, 4 KiB target, and replacement gate of at least
  512 bytes and 20% savings are byte-level break-even heuristics. They should
  be retained as conservative overhead guards until session measurements show
  a better threshold; they do not imply a token break-even point because JSON,
  code, Unicode and whitespace tokenize differently.
- Preserve the current newest-tool-only behavior and immutable originals. A
  cross-turn dedup marker must never become the only copy, and a lossy selector
  must continue to state omissions and provide recovery metadata.
- Keep deterministic JSON factoring before lossy selection. LLMLingua-2's
  benchmarked token classifier is not a justification for field-level clipping,
  and malformed JSON must continue to fall back to text selection.
- When diagnostics exceed budget, retain evidence anchors before ordinary
  context: command/error headers, exit status, first diagnostic occurrence,
  and a tail containing the final status. Mark every omitted range/count and
  keep absolute line labels so a reader can request the immutable original.
  Repeated-line collapse must retain the first line's location and repeat
  count; otherwise a smaller view can lose the proof of where a failure came
  from.
- Do not add an implicit model/runtime or API proxy under the name of
  compression. That would change Scopelet's stated architecture, latency and
  trust boundary.
- Thresholds should be validated against serialized metadata and token/session
  measurements, not copied from RTK or Headroom. Their percentages have
  different denominators and workloads.

## Source classification

Headroom and RTK descriptions above are upstream implementation/marketing
claims where explicitly identified. LLMLingua-2 figures are paper benchmark
claims with the listed dataset scope. OpenAI and Anthropic caching behavior is
provider documentation scoped to their APIs, not hosted-agent subscription
billing. The compatibility judgments and three recommendations
are inferences from those sources plus `docs/design.md` and the current
Scopelet contracts.
