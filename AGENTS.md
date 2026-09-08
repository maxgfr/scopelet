# Scopelet

Rust core, installable agent skill, optional codeindex/webindex CLI adapters.
Read `docs/design.md` before changing the query or provenance contract.

Verify changes with `cargo fmt --check`, `cargo clippy --all-targets --locked --
-D warnings`, `cargo test --locked` and `python3 scripts/check_skill.py`.
For benchmark changes run `python3 -m unittest discover -s bench -p 'test_*.py'`.
Real model calls require an explicit `--live`; use isolated synthetic fixtures.

Preserve original bytes, record explicit omissions and propagate process exit
status. Missing usage is unknown, never zero. Benchmark failures stay in reports.
Read the complete skill bundle when changing its behavior; keep the default
instructions short and route detailed syntax to references.
