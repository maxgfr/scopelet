# Performance implementation and verification — 2026-09-10

Scopelet now uses less memory on the measured large JSON/log workloads and
recovers saved line ranges faster. Compact-v2 becomes the CLI/hook default:
the complete Luna comparison passes the planned rollout gate. **Token savings
are concentrated in recovery; three other evidence tasks and the small edit
regress against the old Scopelet in this small sample.**

## What changed

- Automatic compression builds an eligible view before touching storage, reuses
  parsed JSON/JSONL, and borrows evidence during selection. Rejected views stay
  byte-exact and create no cache. Accepted artifacts stream through a bounded,
  buffered hashing writer, preserving their original v1 JSON content identities.
- JSONL filter chains ending in count/group accumulate one parsed row at a time.
  Repository leading searches run per file with patterns compiled once. Other
  compositions retain the materialized pipeline. Original snapshots, scan limits,
  ordering, multiline matching and transactional parsing remain intact.
- Compact-v2 factors partial homogeneous JSON tables, retaining every column,
  exact values and original row indices. Distinct diagnostics compete before
  repeated ones; selected units return to source order. Explicit omissions and
  immutable originals remain the recovery contract.
- `expand ID --find TEXT --context N` searches original saved blobs, including
  evidence removed by earlier filtering. Repeated `--find` values are literal
  alternatives; `--source` selects an exact artifact source. Range recovery uses
  disposable, validated line offsets; corrupted or unwritable indexes fall back.
- Conservative automatic profiles now include simple rg/grep, noninteractive Git
  inspection, Go/Node tests and package-manager build/lint/typecheck commands.
  Native failure fallback and process exit/cancellation status remain intact.

The [design contract](design.md) describes boundaries. The
[existing research](optimization-research-2026-09-09.md) informed deterministic
compression and recoverable evidence; the earlier Fable design consultation was
not repeated. All live testing for this implementation requested Luna only.

## Offline release-binary measurements

[Full measurements](../bench/results/performance-engine-2026-09-10.json):
8 fixtures × cold/warm application cache × 3 arms × 30 measurements = 1,440
measurements, plus five warmups per cell. The harness alternates arm order and
uses native processes with `/usr/bin/time`; elapsed time includes process launch.
Cold clears the application cache, **not** the OS page cache. RSS is whole-process
peak, expressed below in decimal MB. These are local macOS measurements.

| Workload / cache | Baseline v1 ms | Candidate v2 ms | Time change | Baseline RSS MB | Candidate RSS MB |
|---|---:|---:|---:|---:|---:|
| Small native output / cold | 4.48 | 3.80 | -15.2% | 7.25 | 7.18 |
| Small native output / warm | 3.78 | 3.78 | +0.0% | 7.13 | 7.18 |
| 3 MB diagnostic log / cold | 31.48 | 29.44 | -6.5% | 24.31 | 17.40 |
| 3 MB diagnostic log / warm | 33.94 | 31.82 | -6.2% | 27.49 | 20.64 |
| 12,000-row JSONL table / cold | 49.04 | 37.32 | -23.9% | 53.47 | 29.39 |
| 12,000-row JSONL table / warm | 50.47 | 39.71 | -21.3% | 53.59 | 34.29 |
| JSONL filter/group / cold | 22.68 | 21.45 | -5.4% | 27.71 | 12.40 |
| JSONL filter/group / warm | 19.02 | 17.73 | -6.8% | 28.08 | 14.78 |
| 400-file repository / cold | 1780.18 | 1785.23 | +0.3% | 10.52 | 9.63 |
| 400-file repository / warm | 28.86 | 29.07 | +0.7% | 10.57 | 9.65 |
| 90,000-line range recovery / cold | 20.10 | 15.19 | -24.5% | 11.45 | 11.19 |
| 90,000-line range recovery / warm | 15.32 | 10.49 | -31.5% | 11.44 | 11.12 |
| 32 MiB compression / cold | 174.12 | 171.08 | -1.7% | 111.33 | 77.91 |
| 32 MiB compression / warm | 271.12 | 272.05 | +0.3% | 178.32 | 111.46 |

There were no process failures and no byte-output mismatches between baseline
v1 and candidate v1 across all 480 paired measurements. Rejected giant-line
compression was also checked: native output is unchanged and no cache opens.
Memory falls roughly 45% for the cold JSON table, 55% for the cold aggregate,
and 30% for the cold 32 MiB fixture. Warm range recovery is about 32% faster.
Repository latency is essentially unchanged; cold storage synchronization still
dominates. Warm 32 MiB throughput is also unchanged. The broad 20% latency / 25%
memory aspirations are therefore met on selected paths, not every workload.

The [initial two-repetition exploration](../bench/results/performance-initial-2026-09-10.json)
is retained, including a roughly 185 ms cold recovery regression caused by
unbuffered index serialization (baseline about 21 ms). Buffered writes and a
bounded index corrected it before the full measurement above. A separate
[20,001-file limit check](../bench/results/performance-limits-2026-09-10.json)
retains exactly 20,000 examined files/snapshots and `scan_complete=false`, with
identical v1 views. Another 120 synthetic differential cases exercised JSON,
JSONL, text, Unicode, CRLF and budgets without v1 differences.

## Luna sessions and rollout

[Inspected machine-readable results](../bench/results/performance-luna-2026-09-10.json)
retain all 30 sessions: five tasks × native/baseline/candidate × two repetitions,
shuffled with seed 20260910. Codex CLI 0.153.4 requested `gpt-5.6-luna` with low
effort. Usage events did not identify the serving model; the report records the
requested model and leaves observed identity unknown. There were no retries,
timeouts, missing usage or quota rejections. No additional live smoke was run.

Each session uses isolated synthetic fixtures and scoped hooks, with an independent
functional grader and immutable evidence hashes. Global skills are disabled.
Both Scopelet arms receive the same brief availability instruction; native does
not. The recovery fixture supplies native text to the control and a compact view
backed by immutable originals to Scopelet. This tests the complete workflow,
not the compactor in isolation. Raw transcripts remain local; exported evidence
is whitelisted and original transcript hashes are verified.

Numbers are whole-session **logical input + reported output tokens**, including
cached input. Reasoning is already included in output and is not added again.
These are neither billable-token estimates nor subscription charges. Each cell
shows both repetitions; changes compare their arithmetic means.

| Task | Native tokens | Baseline v1 tokens | Candidate v2 tokens | Candidate vs baseline |
|---|---:|---:|---:|---:|
| Small edit | 66,952 / 109,576 | 95,979 / 95,661 | 113,571 / 83,810 | +3.0% |
| Diagnostics | 144,971 / 146,188 | 90,820 / 88,026 | 112,952 / 129,940 | +35.8% |
| Aggregation | 82,071 / 67,807 | 165,612 / 228,141 | 154,819 / 329,138 | +22.9% |
| Repository bug | 122,102 / 91,190 | 89,194 / 144,964 | 122,398 / 126,241 | +6.2% |
| Hidden evidence recovery | 283,949 / 91,987 | 270,906 / 785,318 | 57,832 / 57,945 | -89.0% |

Candidate passes 10/10; native and baseline each pass 9/10. Both failures occur
on recovery and remain in totals. Native repetition 1 read incomplete evidence,
then wrote the wrong receipt. Baseline repetition 2 mistyped a blob hash,
received explicit errors, later inspected an oversized expansion and concluded
incorrectly that the receipt was absent; it wrote no answer. Both originals
contained the expected receipt. These are observed agent failures, not missing
fixture data. They make the pooled saving sensitive to individual trajectories.

The predetermined gate required a complete campaign, no candidate functional
failure, and at least 10% fewer total session tokens across the four evidence
tasks than the old Scopelet. Baseline used **1,862,981**, candidate **1,091,265**:
**−41.4%**, so the gate passes. V2 is now the CLI/hook default; use
`--compact-version 1` or `SCOPELET_COMPACT_VERSION=1` to restore v1. Explicit
CLI selection wins. Legacy library helpers `automatic` and `compact` retain v1.
The query and artifact JSON schema versions remain 1.

Recovery drives this decision. Diagnostics, aggregation and repository tokens
increase against baseline, and the small edit increases 3.0%. Native aggregation
is also much cheaper in these trials. Two repetitions per cell cannot establish
statistical confidence, universal savings, or isolate each optimization's effect.
The full sample is reported rather than treating smaller output bytes as proof
of lower session cost.

## Frozen provenance and final checks

Offline engine binaries (SHA-256):

- baseline: `4ffe353b8fc016513375e565956b9fc747c63dc67e3bd4cc4545649191b7f51d`
- candidate: `10ca66b32ecddc5374d5758473fd891ce9212196dc15ce7a5b050a099bef1718`

Luna campaign binaries (SHA-256):

- baseline: `4ffe353b8fc016513375e565956b9fc747c63dc67e3bd4cc4545649191b7f51d`
- candidate: `28b82318f9e94c27eb8d5cdcef8b66abbc5fda689ffdcab9380df2903817f1bc`

Baseline is the original 0.2.3 binary from commit
`37cdafc61498367c1b7e803f5a3ed63b6a811d85`. The upstream manual-invocation fix
`47bf5b8` was preserved during implementation. Binary hashes distinguish the
frozen measured candidates from the final source tree.

The Luna binary was frozen before final line-index provenance validation/API
hardening, invalid-environment fail-open handling, read-only artifact reuse and
interruption-status fixes. The full offline candidate includes the index and
environment fixes, but predates read-only reuse and interruption fixes. The final
default change selects the v2 behavior already measured explicitly. These later
changes are covered offline; they are **not** represented as another live-tested
binary. The offline harness now pins candidate v1 explicitly to remain valid
after the default switch.

Final local checks: formatting; Clippy with warnings denied; 93 Rust integration
tests; 71 Python benchmark-harness tests; six-file skill packing/validation;
nine Node launcher/release tests; Rust 1.88 compatibility; isolated installation,
mode transitions and uninstall for both hosts; real codeindex/webindex adapters.
Recovery checks include forged indexes, original-byte preservation, partial
provenance, malformed final JSONL, cache failures and SIGINT exit 130. Required
checks also run in the existing main release workflow before publication.

Reproduction commands are in [bench/README.md](../bench/README.md). Live calls
always require `--live`; this campaign is complete and is not automatically rerun.
