# What is reused, and what differs

Inspected on 2026-09-09. These projects evolve; links identify the implementation
surfaces examined, not an exhaustive feature or performance ranking.

| Project | Verified approach | Scopelet decision |
|---|---|---|
| [Caveman](https://github.com/juliusbrussee/caveman/blob/main/skills/caveman/SKILL.md) | Skill steers concise communication; exact code/errors and meaningful qualifiers have explicit protection. Its [retrieval engine](https://github.com/juliusbrussee/caveman/blob/main/engine/retrieve_query.go) already selects complete units and marks omitted gaps. | Keep skill adoption simple. Do not claim handles, focused recovery, or omission markers as new inventions. |
| [Headroom](https://github.com/headroomlabs-ai/headroom/blob/main/crates/headroom-core/src/transforms/smart_crusher/planning.rs) | Content-specific compression combines query anchors, relevance, errors, outliers and budget pruning. | Avoid another general compression engine. Compose deterministic queries before content enters the conversation. |
| [Ponytail](https://github.com/dietrichgebert/ponytail/blob/main/skills/ponytail/SKILL.md) | Steers minimal implementations; [agentic evaluations](https://github.com/dietrichgebert/ponytail/blob/main/benchmarks/agentic/README.md) check resulting code and adversarial cases. | Grade completed tasks with independent checks, including the simple-task case where instructions may cost more than they save. |
| [RTK](https://github.com/rtk-ai/rtk) | Specialized command-output filtering and original-output recovery. [Local tracking](https://github.com/rtk-ai/rtk/blob/develop/src/core/tracking.rs) estimates tokens using byte length divided by four. | Offer generic bounded capture plus query composition. Do not present local byte estimates as model-reported session usage. |
| [codeindex](https://github.com/maxgfr/codeindex) | Symbols, syntactic relationships and repository analytics. | External adapter; do not rebuild its language parsers. |
| [webindex](https://github.com/maxgfr/webindex) | Document extraction, citable text, ranking and retrieval. | External adapter; do not rebuild HTML/PDF/office extraction. |

Scopelet's hypothesis is that an agent can spend fewer tokens by requesting a
count, grouped result or set of exact source spans in one local operation.
Shell pipelines can do many of the same things. The benchmark therefore keeps
native tools available and includes an optimized-shell control rather than
forcing the baseline to dump whole files.

Caveman's engine has a split MIT/BSL license. Scopelet's implementation is new
MIT code; none of that runtime is copied or embedded. Headroom/RTK are useful
comparators, but are not necessary runtime dependencies of this first release.

See [research.md](research.md) for primary papers and their transfer limits.
