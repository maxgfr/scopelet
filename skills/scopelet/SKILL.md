---
name: scopelet
description: Reduce context when exploring repositories, querying large JSON/log files, or reading command output. Use for scopelet, token-saving mode, or bulk evidence selection.
license: MIT
metadata:
  author: maxgfr
  version: "0.1.3"
---

# Scopelet

Compute before reading. If `SCOPELET_BIN` is set, execute `"$SCOPELET_BIN"` directly.
Otherwise use `node <this-skill>/scripts/scopelet.mjs`; an existing compatible
`scopelet` binary on PATH also works. The launcher
downloads its pinned release once. For a small known file, read it directly.

Choose the result needed: matching passages, selected fields, counts or groups.
Compose them in one query. Never read a whole large file to learn its schema: if
fields are unknown, inspect at most 2 KiB first (`head -c 2048 events.jsonl`), then
compute locally. Read [queries.md](references/queries.md) for other operations.

```sh
scopelet query --repo . --find validateToken --context 5
scopelet run -- npm test
```

`--find` is literal; repeat it for alternatives, with one shared `--context`.
Regex needs a spec.

For JSONL counts by a known field, filter then group in one call:

```sh
scopelet query --spec - <<'JSON'
{"version":1,"source":{"type":"file","path":"events.jsonl","format":"jsonl"},"operations":[{"op":"filter","pointer":"/status","equals":"failed"},{"op":"group","pointer":"/suite"}]}
JSON
```

Group rows contain `value.key` and `value.count`; sum counts locally for a total.
`count` replaces records: never put it before `group`. Use native tools for edits
and short tests. If RTK already wraps a command, use it directly; avoid stacking
wrappers. Stop gathering evidence once the question and required checks are met.

Default preserves exact selected passages; `--mode ultra` also abridges large
text units. Ultra is opt-in: pass `--mode ultra` on EVERY `query` and `run` in
that mode, including `--spec` calls. `off` resumes native tools. Both modes can
omit results to fit the budget.

Check `scan_complete`, `display_complete` and `omitted_records` before drawing
conclusions. A partial view cannot establish absence or exhaustiveness. Expand
the artifact to page results, or its blob to read exact lines before editing.
Originals remain local; expansion does not assert the source is still current.

Keep progress brief and avoid repeating tool results in the final answer.
Answer in the user's language, preserving qualifications, exact code,
errors, negations, exceptions, numbers and units. Use familiar words; keep
ordered steps clear. Requested explanations and saved documentation use normal
prose. Before delivery, check these coverage rules
and the task's acceptance tests. Tool byte reductions are not session savings.

See [setup.md](references/setup.md) for installation failures and adapters.
MIT · [maxgfr / support](https://github.com/maxgfr/scopelet/issues).
