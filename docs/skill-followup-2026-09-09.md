# Skill follow-up and cache correction — 2026-09-09

The revised skill passed all 24 external functional checks. The four Scopelet
JSONL runs used a 2,048-byte schema preview and returned an ultra-mode aggregate.
That observed instruction failure did not recur on this fixture. Session usage
remains workload-dependent: concise native controls were cheaper for the tiny
edit on both agents and for JSONL on Codex.

This is the [separately declared follow-up](direct-comparison-plan.md), not a
replacement for the [51-run competitor pilot](direct-comparison-2026-09-09.md).
Two repetitions per cell, seed 20260910, same model per agent, frozen candidate
skill and binary. [Full accounting](../bench/results/skill-v012-followup-2026-09-09.json)
includes cache, timings and hashes. Values are logical input plus output tokens;
they are not prices. The two samples do not establish statistical superiority.

## Both repetitions, without selection

| Agent | Task | Concise native (reps 1 / 2) | Scopelet ultra (reps 1 / 2) | Change in mean |
|---|---|---:|---:|---:|
| codex | Noisy command | 137,329 / 143,912 | 108,465 / 170,514 | -0.8% |
| codex | JSONL counts | 65,231 / 65,508 | 119,473 / 90,071 | +60.3% |
| codex | Tiny edit | 78,305 / 104,949 | 118,503 / 124,901 | +32.8% |
| claude | Noisy command | 406,030 / 337,619 | 128,493 / 298,809 | -42.5% |
| claude | JSONL counts | 165,594 / 312,737 | 124,126 / 124,171 | -48.1% |
| claude | Tiny edit | 144,055 / 119,113 | 147,671 / 175,566 | +22.8% |

The spread matters. Claude concise JSONL varied from 165,594 to 312,737 tokens;
Claude ultra noisy-command varied from 128,493 to 298,809. Neither an arbitrary
noise threshold nor a global winner can be inferred from these observations.
Comparisons against the original pilot are not contemporaneous controls for
the instruction change.

## Independent trace audit

A separate agent checked all 25 noisy-command traces across both campaigns:
each executes checks.py before the successful fix and afterward. Some native
pipelines hide the child exit status through tail, but their actual success
markers confirm completion. All four candidate Scopelet JSONL traces use the
bounded preview and an actual ultra result, with no whole-file emission.

Two retained failures of efficiency/compliance:

- Claude noisy ultra repeat 2 used three repository queries in **default** mode,
  despite the improved instruction; its two command wrappers did use ultra.
  Explicit flags in a skill improve guidance, not enforce agent behavior.
- Codex noisy ultra repeat 2 searched with `rg --hidden` and re-ingested its
  workspace-local Scopelet cache: 139,464 characters from three cache-hit lines.
  That adds avoidable context. The recorded result remains in the table.

## Final release cache fix (after these measurements)

The instruction campaign used the old runtime with a version-only 0.1.2 bump.
The final 0.1.2 binary additionally creates atomic `.ignore` and `.gitignore`
markers inside blobs/artifacts. They keep cached evidence out of ordinary
ignore-respecting searches and Git staging, without changing repository-root
rules. Existing marker files/symlinks remain untouched. Old read-only caches
remain readable; unwritable directories may lack markers. Explicit no-ignore
searches and direct file reads can still include the cache.

A [deterministic replay](../bench/results/cache-replay-2026-09-09.json) of the
offending search on the saved final workspace fell from **142,385 to 2,918 bytes**.
Non-cache search lines were identical, source blobs recovered byte-for-byte,
and Git did not list cache entries for staging. This is a byte-level regression
check, not a new session-token result. The final workspace differs slightly
from the original intermediate trace. Four new Rust regressions cover search
exclusion, existing rules, symlinks and read-only migration; an independent
32-process probe found no clobber or temporary-file leftovers.

## What ships and why

Keep the bounded-read and explicit-mode instructions as reliability corrections.
Keep Caveman-inspired concise prose and fidelity rules; the full Caveman stack
showed no consistent advantage in the pilot. Keep existing RTK wrappers where
appropriate. Avoid activating Scopelet for tiny known-file edits merely to save
tokens: these measurements favor the native concise control.

Fable reviewed the comparative evidence twice in plan mode. Its useful critique
was to retain the adherence fix, expose run-to-run spread and avoid stacking
dependencies without evidence. We did not adopt its unsupported numerical noise
thresholds or an arbitrary three-command routing rule. Passing finite graders
does not establish broad semantic fidelity or a production quality guarantee.
