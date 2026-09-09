# Agent comparisons

`run.py` provides synthetic fixtures, immutable external graders, native controls,
provider usage normalization and process-group timeouts. `compare.py` adds full
competitor skills, manual RTK wrapping and an explicit Headroom proxy adapter.
No provider calls occur without `--live`. Output directories must be empty:
completed evidence is never overwritten.

Read the [declared protocol](../docs/direct-comparison-plan.md) and
[completed pilot](../docs/direct-comparison-2026-09-09.md) before interpreting results.

## Dependencies and frozen inputs

Install/authenticate Codex and Claude Code normally. The recorded pilot used
Codex 0.153.4 with GPT-5.6-Luna (low) and Claude Code 2.1.263 with Haiku 4.5.
These models are configured in `run.py`; compare within an agent/model, not across
the two. Host instructions can still affect results. Runs use temporary isolated
Git workspaces, project skills and scoped environment/configuration overrides.
They do not install competitor hooks or change global agent settings.

Clone the competitors outside this repository and check out these revisions:

| Repository | Revision |
|---|---|
| headroomlabs-ai/headroom | e67b3c8a29443a60d6b0018fb22f525c5cd7e709 |
| juliusbrussee/caveman | 15581d14007fd01fb3f132016741962f34936ca2 |
| dietrichgebert/ponytail | 356918eba965ee1eac64bd3a7f0dd02108350de5 |
| rtk-ai/rtk | 8e9aa04cb2afb189747fac4e36bec2254ddd0564 |
| maxgfr/scopelet (original pilot) | d7f114c0e6ce44f34856ee1bd395a59d138f66f7 (v0.1.1) |

Use the full `skills/caveman` and `skills/ponytail` directories. Build RTK with
`cargo build --release --locked` in its checkout. Headroom 0.37.0 was installed
from the pinned checkout in a Python 3.13 virtual environment. Use the published
Scopelet 0.1.1 binary for your platform and its tagged skill for the original
pilot; current main contains a revised skill. Report hardware/build changes.

The harness copies binaries, complete skills and Python prefix files into each
campaign's `frozen/` directory before starting. Headroom's executable/venv stays
external; record its source revision and do not change the installation during a
campaign. SHA-256 hashes and CLI versions are recorded automatically. Supply
`--source-commits-json` with the revisions above as an additional provenance field.

## Run the matrix

Set these shell variables to the corresponding local paths: `COMPARE_SCOPELET`,
`COMPARE_SKILL`, `COMPARE_CAVEMAN`, `COMPARE_PONYTAIL`, `COMPARE_RTK` and
`COMPARE_HEADROOM`. Native executable variables must point to files; skill
variables must point to directories containing `SKILL.md`.

```sh
python3 bench/compare.py --dry-run --tasks task4,task2,task3 --out bench/runs/plan

python3 bench/compare.py --live --agents claude --tasks task4,task2,task3 \
  --scopelet-binary "$COMPARE_SCOPELET" --scopelet-skill "$COMPARE_SKILL" \
  --caveman-skill "$COMPARE_CAVEMAN" --ponytail-skill "$COMPARE_PONYTAIL" \
  --rtk-binary "$COMPARE_RTK" --headroom-binary "$COMPARE_HEADROOM" \
  --headroom-prefix-json '["python3","bench/headroom_proxy.py","--headroom","{headroom}"]' \
  --out bench/runs/comparison-claude

python3 bench/compare.py --live --agents codex --tasks task4,task2,task3 \
  --arms native,concise,scopelet,scopelet-ultra,scopelet-caveman,caveman,ponytail,rtk \
  --scopelet-binary "$COMPARE_SCOPELET" --scopelet-skill "$COMPARE_SKILL" \
  --caveman-skill "$COMPARE_CAVEMAN" --ponytail-skill "$COMPARE_PONYTAIL" \
  --rtk-binary "$COMPARE_RTK" --out bench/runs/comparison-codex
```

Default seed is 20260909 and default repetitions are one. Increase repetitions
and declare the new protocol before measuring. Random order does not make a
single repetition statistically reliable. The revised-skill follow-up uses
`--arms concise,scopelet-ultra --repetitions 2 --seed 20260910` and a separately
frozen 0.1.2 skill/binary.

## Proxy verification

`headroom_proxy.py` is the portable version of the pilot's private adapter, with
an explicit executable path and a new fail-closed routing check. It launches a
local token-mode proxy, configures only the child Claude process and enables
Headroom's recovery MCP. It returns 125 if the agent succeeds without increasing
the proxy request counter. Agent failures preserve their status. All children
remain in the campaign process group for timeout cleanup. Set
`SCOPELET_HEADROOM_MODE=cache` only for a separately labeled cache-mode study.

Codex routing was not verified with this environment's authentication after four
preflight attempts, so the portable adapter rejects Codex rather than producing
false Headroom measurements. This is an environment-specific unmeasured cell,
not an upstream compatibility verdict. The original pilot adapter hashes are
retained in the results; this portable adapter has a different hash.

## Inspect and export

```sh
python3 -m unittest discover -s bench -p 'test_*.py'
python3 scripts/export_comparison.py bench/runs/comparison-claude \
  bench/runs/comparison-codex --out bench/results/my-comparison.json
```

Reports retain failed runs, unknown usage, wall-clock duration, cache fields,
fixture hashes and adoption counters. The external grader checks the final
workspace against pristine checks and rejects modified fixtures. Tool counts do
not prove required command order, skill compliance or semantic fidelity. Review
raw traces when those matter. A returned view's mode can differ from the requested
arm, and an exact expansion can intentionally use default mode.

Raw transcripts, prompts, generated workspaces and proxy logs remain under
ignored `bench/runs/`. Export only inspected summaries. Logical session input
includes cached input; it is not a price calculation. Competitor compression
estimates and byte reductions are separate measurements.
