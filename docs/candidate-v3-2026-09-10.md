# Compact-v3 candidate results — 2026-09-10

Protocol declared before any provider call in
[candidate-v3-plan.md](candidate-v3-plan.md). Offline measurements come from
the repository's own probes; the live campaign is Claude Code 2.1.267 with
Haiku (`claude-haiku-4-5-20251001`). Byte reductions are not token savings and
not session cost. Two live repetitions per cell are exploratory.

Binaries: released 0.3.2
(`96b3b6daef3743dac86f58fd9015dfa5fd6b1668381890af2949c025e1667e45`, compact-v2
default) and the candidate
(`680c00682f144e5b9ec918dae86805969de81b8dcabe2f10f128085baf16cfa7`, compact-v3
default). Both report version `0.3.2`; the release version is assigned at
publication, so the hashes identify them.

## Shared content probes

`bench/content.py`, eight fixtures whose SHA-256 values match the 2026-09-10
run, five warmups and thirty repetitions, cold application cache. RTK 0.42.4
and Headroom 0.37.0 are shown for context on shared capabilities only.

| fixture | input | v2 output | v3 output | v2 | v3 | RTK | Headroom |
|---|---|---|---|---|---|---|---|
| small | 32 | 32 | 32 | 0.0% | 0.0% | 0.0% | 0.0% |
| diagnostics | 136063 | 3898 | 516 | 97.1% | 99.6% | 0.0% | 0.0% |
| repeated_errors | 28039 | 4031 | 421 | 85.6% | 98.5% | 0.0% | 98.7% |
| json | 171834 | 3983 | 4061 | 97.7% | 97.6% | 0.0% | 30.2% |
| jsonl | 171832 | 3924 | 3984 | 97.7% | 97.7% | 0.0% | 0.0% |
| hidden_receipt | 136193 | 3961 | 634 | 97.1% | 99.5% | 0.0% | 0.0% |
| giant_unicode_line | 12000 | 12000 | 1288 | 0.0% | 89.3% | 0.0% | 0.0% |
| crlf | 137031 | 4026 | 472 | 97.1% | 99.7% | 0.0% | 0.0% |

Every required fact is visible in the v3 view except the giant line, which is
shown cut behind a `text_truncated bytes=12000` marker and recovered exactly
from its blob. `hidden_receipt` changes verdict: under v2 the receipt was
reachable only through `expand`, under v3 it is in the view. Every fixture
round-trips to its original bytes through the stored blob.

The three cases the plan targeted are the three that moved. `repeated_errors`
closes the gap to Headroom (98.5% against 98.7%) by folding dispersed
identical lines. `hidden_receipt` and `diagnostics` gain from template folding
plus the promotion of singleton lines inside a mostly folded record.
`giant_unicode_line` stops passing through because an oversized line is now cut
instead of vetoing the view. JSON and JSONL are unchanged: they take the shared
table path, and the small difference is the v3 envelope spending its saved
bytes on more rows.

Cold latency on these fixtures falls from 26–37 ms to 4.6–7.0 ms and no longer
differs from warm.

## Engine measurements

`bench/performance.py`, five warmups, thirty repetitions, stress case included,
macOS 26.6.2 arm64. `baseline` is the frozen 0.3.2 at compact-v2, `candidate`
is the new binary at compact-v3, `candidate_v2` is the new binary at
compact-v2. `output_mismatches` is empty: compact-v2 output is byte-identical
between the two binaries in every case and repetition, which is the evidence
that v2 is frozen.

Median milliseconds:

| case | 0.3.2 (v2) | candidate (v2) | candidate (v3) |
|---|---|---|---|
| small/cold | 4.1 | 4.1 | 4.0 |
| rejected/cold | 4.4 | 4.3 | 5.7 |
| log/cold | 30.3 | 23.0 | 27.2 |
| log/warm | 32.8 | 33.2 | 37.7 |
| json_table/cold | 37.5 | 29.8 | 29.7 |
| aggregate/cold | 22.3 | 14.3 | 14.5 |
| repo/cold | 1654.4 | 92.4 | 99.0 |
| repo/warm | 30.1 | 30.4 | 30.0 |
| recovery/cold | 15.3 | 11.1 | 11.1 |
| limit_32m/cold | 172.6 | 160.7 | 206.1 |
| limit_32m/warm | 271.9 | 271.9 | 317.1 |

The two columns of the candidate separate the two changes. Comparing 0.3.2
against the candidate at v2 isolates the storage change: the cold repository
query over 400 files drops from 1654 ms to 92 ms, because both store writes
called `sync_all`, which std maps to `F_FULLFSYNC` on Apple platforms and costs
milliseconds per stored item. Comparing the candidate's two columns isolates
the v3 presentation: it costs 13% on a 3 MB log warm and 28% on the 32 MiB
boundary case, the price of computing a template key per line. In absolute
terms the boundary case moves from 161 ms to 206 ms for a 32 MiB input, and
`rejected` moves from 4.3 ms to 5.7 ms while changing from passthrough to a
1288-byte view. Peak resident memory is unchanged.

## Live campaign

Claude Code 2.1.267, Haiku, three arms over three tasks, two repetitions,
seed 20260910, sequential, 600 second timeout. All 18 attempts ran, all 18
passed their independent acceptance grade, none was retried, no usage was
missing and no quota was hit. Reported provider cost was $1.21 in total.
The published summary is
[`bench/results/candidate-v3-2026-09-10.json`](../bench/results/candidate-v3-2026-09-10.json).

Mean logical input plus output tokens (cache included), two runs per cell:

| task | native | released | candidate | candidate vs released |
|---|---|---|---|---|
| task4 (noisy diagnostics) | 349524 | 298559 | 287190 | −3.8% |
| task2 (JSONL aggregation) | 205266 | 229973 | 177937 | −22.6% |
| task3 (small edit) | 131902 | 145824 | 157608 | +8.1% |

The pre-declared rule returns **go**: no candidate attempt failed, and the
candidate's combined `task4` plus `task2` mean is 465128 against 528532 for the
released binary, 12.0% lower.

**The rule is satisfied, and the mechanism it was meant to detect is not
demonstrated.** Two facts prevent reading the table as a compression result.

First, compression only engaged on `task4`. Counting compact views actually
delivered to the model: `task4` saw one under released and one to two under
the candidate, while `task2` and `task3` saw none in either arm, because no
single command in those tasks produced more than 2 KiB on one stream. In those
two tasks the treatment is the hook's session context, not the engine.

Second, the run-to-run spread is larger than every difference in the table:

| cell | run A | run B | ratio |
|---|---|---|---|
| task4/candidate | 207432 | 366949 | 1.77 |
| task2/released | 178673 | 281273 | 1.57 |
| task4/native | 292742 | 406305 | 1.39 |
| task2/native | 177258 | 233275 | 1.32 |

The 22.6% `task2` gap rests on a single released run at 281273 tokens; the
other five `task2` runs across all arms sit between 177113 and 233275. The
3.8% `task4` gap sits inside a candidate arm whose two runs differ by 77%.

What this campaign supports is therefore narrow and worth stating exactly:
compact-v3 introduced no functional regression on Haiku across nine sessions,
its hooks activated in every treated session, and it produced no token
regression that two repetitions could detect. It does not establish a token
saving. The offline probes above are the measured result of this work; the
live campaign is a safety check on top of them.

A campaign able to attribute a token difference to the engine would need
tasks whose commands reliably exceed the 2 KiB threshold several times per
session, and enough repetitions to separate a single-digit effect from a
1.8× spread. That is a larger budget than this plan declared.

## Follow-ups, not addressed here

- Small-edit overhead (`task3`) is dominated by the skill context and mode
  prompt, not by the compression engine; the engine cannot fix it.
- A shared stdout/stderr budget would stop a noisy stderr from crowding out
  stdout evidence. It changes the hook contract and was left out.
- Artifact and blob are stored separately even when one is derived from the
  other; deduplicating them is a schema change.
- No local tokenizer: every figure above is bytes or provider-reported usage,
  never an estimate of tokens.
