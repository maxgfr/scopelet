---
name: scopelet
description: Reduce context when exploring repositories, querying large JSON/log files, or reading command output. Use for scopelet, token-saving mode, or bulk evidence selection.
license: MIT
metadata:
  author: maxgfr
  version: "0.1.0"
---

# Scopelet

Compute before reading. If `SCOPELET_BIN` is set, execute `"$SCOPELET_BIN"` directly.
Otherwise use `node <this-skill>/scripts/scopelet.mjs`; an existing compatible
`scopelet` binary on PATH also works. The launcher
downloads its pinned release once. For a small known file, read it directly.

Choose the result needed: matching passages, selected fields, counts or groups.
Compose those operations in one query instead of printing bulk data to compute
over it yourself. Start with these recipes; read [queries.md](references/queries.md)
only for other operations or codeindex/webindex.

```sh
scopelet query --repo . --find validateToken --context 5
scopelet run -- npm test
```

For JSONL counts by a known field, filter then group in one call:

```sh
scopelet query --spec - <<'JSON'
{"version":1,"source":{"type":"file","path":"events.jsonl","format":"jsonl"},"operations":[{"op":"filter","pointer":"/status","equals":"failed"},{"op":"group","pointer":"/suite"}]}
JSON
```

Group rows contain `value.key` and `value.count`; sum counts locally for a total.
Operations are sequential: `count` replaces the records, so never put it before
`group`. Use native tools for edits and short test output.

Default preserves exact selected passages; `--mode ultra` also abridges large
text units. Keep the selected mode for this conversation; `off` resumes native
tools. Ultra is opt-in. Both modes can omit results to fit the display budget.

Check `scan_complete`, `display_complete` and `omitted_records` before drawing
conclusions. A partial view cannot establish absence or exhaustiveness. Expand
the artifact to page results, or its blob to read exact lines before editing.
Originals remain local; expansion does not assert the source is still current.

Keep progress brief and avoid repeating tool results in the final answer.
Answer in the user's language, preserving qualifications, exact code,
errors and requested explanations. Before delivery, check these coverage rules
and the task's acceptance tests. Tool byte reductions are not session savings.

Reference files: [queries.md](references/queries.md) covers operations and
recovery; [setup.md](references/setup.md) covers installation failures and adapters.
MIT · [maxgfr / support](https://github.com/maxgfr/scopelet/issues).
