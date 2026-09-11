# Verification

What is checked on every change, what is measured on the shipped binary, and
what is deliberately not claimed.

## On every change

[CI](https://github.com/maxgfr/scopelet/actions/workflows/ci.yml) runs on
Ubuntu 24.04 and macOS 14 for every pull request and pre-release push:

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and the
  Rust integration tests: queries, exact byte recovery, freshness, malformed
  input, path and span validation, budgets, cache cleanup, capture throughput,
  process behaviour, the compact-v3 presentation and the content gate.
- `scopelet bench`, the deterministic offline correctness and byte-size check
  built into the binary.
- `scripts/check_skill.py --pack`: the skill bundle is standalone, its version
  matches Cargo, every host may invoke it, and it packs cleanly.
- The offline benchmark unit tests, the launcher tests and the release
  automation tests under Node 24.
- Real codeindex and webindex adapter checks against pinned published versions.
- A release build, then `scripts/check_readme.py`, which rebuilds every fixture
  in the README reduction table, runs the binary and fails if any figure
  drifts by a byte.
- Rust 1.88 minimum, checked with `cargo check` on a pinned toolchain.

## On the shipped binary

Two offline probes described in [bench/README.md](../bench/README.md) produce
the figures quoted in the README. `bench/publish.py` runs them on the release
binary and replaces `bench/results/content.json` and
`bench/results/performance.json`; each records the binary's version and
SHA-256, the platform and every sample. Only the shipped version's figures are
kept.

- `content.py`: eight realistic outputs; bytes in and out, cold and warm
  timings, every fact still visible or recoverable, original bytes round-trip
  through the artifact manifest and source blob.
- `performance.py`: wall time and peak resident size on a 3 MB log, a
  12,000-row table, an aggregate query, a 400-file repository search, a paged
  recovery and a stream just under the 32 MiB capture limit.

`tests/content_gate.rs` pins the content fixtures by SHA-256 to the published
report, so the fixtures cannot drift from the numbers without failing the build.

## What the host sees

The Claude Code adapter skips outputs the host has already persisted to disk:
Claude supplies `persistedOutputPath` and `persistedOutputSize` before rendering
its own preview, and recompressing that stdout made the host truncate the
compact view and hide its omission footer. Failure events keep native output.
The replacement shape follows the
[Claude Code hook contract](https://code.claude.com/docs/en/hooks#posttooluse-decision-control).
Codex and OpenCode adapters have equivalent contract tests under
`tests/automatic.rs`.

The installer is exercised in temporary configurations for every host: hook
installation, idempotent reinstall, mode changes, preservation of unrelated
hooks, and uninstall. `scripts/check_install.py` covers the published launcher.

## Not claimed

Bytes are not tokens, and no figure here is a bill. Earlier live-agent
campaigns on Scopelet 0.1.x–0.3.x found no functional regression, but their
run-to-run spread was wider than every token difference they measured, and they
ran against binaries and a skill design that no longer ship. They were removed
rather than kept as stale evidence. Whole-session cost depends on the host, the
model and the task. Measure your own setup before quoting a percentage.
