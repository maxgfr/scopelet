# Benchmarks

Two offline probes measure the binary that ships. Neither calls a model, needs
an API key, or talks to the network. Both refuse to overwrite an existing output
directory: completed evidence is never replaced in place.

Earlier live-agent campaigns (Codex Luna, Claude Haiku and Fable pilots against
Caveman, Ponytail, RTK and Headroom, on Scopelet 0.1.x–0.3.x with the manual
skill) were removed once the engine, the default presentation and the
installation model had all changed. Their numbers described binaries that no
longer exist. Whole-session token accounting depends on the host, the model and
the task; measure your own setup before quoting a percentage.

## Content: does the view keep the facts?

`content.py` runs `scopelet compress` on eight realistic outputs: a 136 KB
progress log with two diagnostics at the end, a retry loop hiding one fatal
error, the same log with CRLF endings, a receipt buried in the middle, a
1000-row JSON array, a 1000-row JSONL stream, a 12 KB single line and a 32-byte
output. For every fixture it records bytes in and out, cold and warm timings,
whether every fact is visible in the view, and whether the original bytes come
back through the artifact manifest and source blob. It exits non-zero when a
fact or a byte is lost.

```sh
cargo build --release --locked
python3 bench/content.py --scopelet target/release/scopelet --out bench/runs/content
```

`tests/content_gate.rs` mirrors the same fixtures byte for byte and pins them to
the SHA-256 values recorded in `bench/results/content.json`, so a compressor
change that silently drops a diagnostic fails `cargo test`. The README
reduction table is checked against the binary by `scripts/check_readme.py`.

## Performance: how fast, how much memory?

`performance.py` measures one or more binaries on the same synthetic workloads:
a 3 MB log, a 12,000-row JSONL table, an aggregate query over it, a 400-file
repository search, a paged recovery from a stored original and, with
`--stress`, a stream just under the 32 MiB capture limit. Each case runs with a
cold application cache and a warm one, five warmups and thirty measured
repetitions by default. Wall time and peak resident size come from
`/usr/bin/time`. The OS page cache is not flushed.

```sh
python3 bench/performance.py --binary release=target/release/scopelet --stress \
  --out bench/runs/performance
```

To compare a candidate against a released build, pass two arms. They alternate
order every repetition, and every arm must produce identical bytes on every
case: a candidate that changes the output fails the run instead of looking
faster.

```sh
python3 bench/performance.py --binary released=/path/to/scopelet-0.5.0 \
  --binary candidate=target/release/scopelet --stress --out bench/runs/candidate
```

## Publishing a release's figures

`publish.py` runs both probes on one binary, replaces everything under
`bench/results/` with the two fresh reports (`content.json`,
`performance.json`) and rewrites the README speed table between its
`speed-table` markers. Only the shipped version's numbers are kept: an older
version's figures are deleted, not archived.

```sh
cargo build --release --locked
python3 bench/publish.py --scopelet target/release/scopelet
python3 -m unittest discover -s bench -p 'test_*.py'
```

Timings are descriptive: other activity on the machine was not controlled, and
only the medians are quoted. `tests/content_gate.rs` reads
`bench/results/content.json`, so a publish that changes a fixture fails the
build until the Rust mirror is updated too.
