# Proportional small-task evaluation

Date: 2026-09-09. Three exploratory task types, not a general savings guarantee.

## Implementation

Small automatic stdout/stderr no longer open the artifact store. A plain Codex
`cat` of a known regular file up to 2 KiB stays native; its current contents and
host permissions still govern execution. Larger reads retain compression. This
removes unnecessary filesystem work and wrapper arguments; it does not prove a
particular token or latency reduction.

Default instructions ask for relevant code and existing checks, preservation of
intended behavior, verification, and a short outcome/validation response. Caveman
has a 30-word routine target with exceptions for required information and user
requests. Neither truncates a model response or overrides user requirements.
The entrypoint skill remains 1,523 bytes and is not invoked during these trials.

## Method

Luna was explicitly requested as `gpt-5.6-luna`, low effort. Its stream does not
independently identify the serving model. Native, released 0.2.2 and candidate
arms receive identical task prompts in fresh synthetic workspaces with isolated
Codex settings and the same experimental hook-trust bypass. All arms disable
global skills. Executables are frozen and hashed before invocation. This does
not change trust or configuration in the user's installation.

Python tests whitespace normalization against an existing README and acceptance
script. JavaScript tests nullish defaults, including false, zero and empty
strings. JSON changes only a top-level retry count, preserving nested fields and
JSON types. Independent graders evaluate saved files; the JS grader includes
cases beyond the visible check. Every attempt and failure is retained.

## Initial candidate: rejected

The [initial campaign](../bench/results/proportional-initial-2026-09-09.json)
contains 18 sessions, two per task/arm. Native and released each pass 6/6;
the candidate passes 5/6. In the failing Python session it searched for tests,
missed `acceptance.py` and the README, changed the intended underscore separator
to spaces, and wrote its own passing assertions. Lower token usage did not make
that answer acceptable. The initial prompt urged targeted reads and the smallest
sufficient verification; it was replaced with an explicit requirement to inspect
existing checks and preserve intended behavior. The frozen failed candidate
remains in its original report.

## Refined candidate: nine separate sessions

All [9/9 refined sessions](../bench/results/proportional-refined-2026-09-09.json)
pass. This is one sample per cell, separate from the initial campaign. Values
are whole-session logical input plus output tokens, including cached input.
They are not billed cost. Full usage and trace summaries remain in the JSON.

| Small task | Native | Released 0.2.2 | Refined | Refined vs native | Refined vs released |
| --- | ---: | ---: | ---: | ---: | ---: |
| Python | 80,668 | 80,122 | 66,808 | -17.2% | -16.6% |
| JavaScript | 79,562 | 65,986 | 66,035 | -17.0% | +0.1% |
| JSON | 65,864 | 52,110 | 65,605 | -0.4% | +25.9% |
| Combined | 226,094 | 198,218 | 198,448 | -12.2% | +0.1% |

No compact output was observed on these small tasks. Differences reflect the
instructions and model variation, not bulk-output compression. The refined
candidate is approximately equal to the release on this mix; it does not
establish an improvement over 0.2.2. Its three final replies are 19–24 words.
One sample cannot establish robustness or statistical significance. The shipped
changes make the small-output path cheaper mechanically and clarify the mode;
the report intentionally makes no new universal performance claim.

## Fable and competitor checks

[Three Fable calls](../bench/results/proportional-fable-2026-09-09.json) cover a
bounded design consultation and two functional smokes with a private relocated
copy of the user's real hooks. The initial default candidate passes JavaScript;
the refined caveman candidate passes the noisy worker repair and preserves the
zero retry count and prohibition in the warning. The default response is 80
words and caveman 127: the 30-word target is not reliably followed with existing
user requirements. No extra calls were spent tuning a claim that the data does
not support. Total provider-reported list cost is about $0.77, not necessarily
the subscription charge.

Fable suggested repeat-read suppression, but a previous-content marker can lose
meaning after host compaction. Scopelet keeps those bytes until a host-aware
implementation can prove recoverability and worthwhile session savings. Its
claim that JSONL gains came from response instructions alone was also too strong:
the earlier campaign could not separate those instructions from model variation.

The [competitor source review](small-task-competitors-2026-09-09.md) checks pinned
Headroom, RTK and Caveman implementations. Relevant practices include size gates,
stable steering placement, preservation of meaning and independent accounting.
Scopelet already has JSON factoring, repetition counts and immutable recovery.
It does not copy a proxy's system-prefix editing into local hooks or interpret
competitors' byte estimates as session savings. No competitor was rerun in this
small-task campaign.

## Reproduction

Use `bench/proportional.py` with `--live`, explicit released/candidate binaries,
and a fresh output directory; see the benchmark README. Export inspected results
with `python3 bench/export_proportional.py INPUT OUTPUT`. The initial campaign
uses two repetitions; the refined campaign uses `--repetitions 1`. Source,
fixture, binary and transcript hashes identify the separate treatments. Raw
transcripts, private hook settings and the Fable driver remain under ignored
`bench/runs/proportional-20260909/`.
