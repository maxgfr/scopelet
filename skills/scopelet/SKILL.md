---
name: scopelet
description: Query large files or repositories, recover compressed evidence, or configure Scopelet automatic compression and response modes.
license: MIT
metadata:
  author: maxgfr
  version: "0.2.0"
---

# Scopelet

Use `"$SCOPELET_BIN"` when set; otherwise `scopelet` or
`node <this-skill>/scripts/scopelet.mjs`.
Small known files and edits use native tools. Installed hooks work without this
skill; avoid stacking Scopelet with another output wrapper.

Compute the result needed before reading bulk data:

```sh
scopelet query --repo . --find validateToken --context 5
scopelet query --file events.jsonl --format jsonl --filter /status --equals '"failed"' --group /suite --output compact
scopelet run --auto -- npm test
```

Unknown JSON fields: inspect at most 2 KiB first. `--find` is literal.
Read [queries.md](references/queries.md) for regex, composed operations,
structured data or recovery. Partial results cannot establish absence or an
exhaustive count. Recover exact source bytes before editing abridged evidence.

`scopelet mode default|caveman|off` controls installed hooks and response style.
Caveman uses minimal telegraphic replies while preserving necessary information;
saved documents use normal prose. `--mode ultra` separately abridges CLI views.
Read [setup.md](references/setup.md) for automatic installation, host coverage,
mode details, failures or optional adapters. Byte savings are not session savings.

MIT · [maxgfr / support](https://github.com/maxgfr/scopelet/issues).
