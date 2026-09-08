# Setup

Install the skill for both agents:

```sh
npx skills add maxgfr/scopelet -a codex claude-code
```

Invoke `/scopelet <task>` in Claude Code or `$scopelet <task>` in Codex.
Explicit invocation is more reliable than expecting automatic selection.
Run `node <installed-skill>/scripts/scopelet.mjs doctor` to check availability.
The launcher installs Scopelet **0.1.0** into the user's cache, downloading the
matching macOS/Linux release from github.com and verifying its SHA-256. Node 18+
is needed for the launcher. No global agent settings are changed.

An independently installed binary is also supported:

```sh
cargo install --git https://github.com/maxgfr/scopelet --tag v0.1.0 --locked
```

Optional adapters are external dependencies, not bundled copies:

```sh
npm install -g @maxgfr/codeindex@2.30.0
brew install maxgfr/tap/webindex
scopelet doctor
```

Basic repository searches, files, JSON and commands work without them. The
`code`, `url` and `document` sources explain when their adapter is absent. If a
network sandbox blocks setup, allow github.com and release-asset hosts, or
install the release manually. A checksum/version mismatch is a failure; do not
substitute an unverified binary.

Cache: `SCOPELET_CACHE_DIR`, otherwise `$XDG_CACHE_HOME/scopelet`, otherwise
`~/.cache/scopelet`. `scopelet clean --older-days 7` removes old originals and
artifacts; `--older-days 0` removes all. References to removed snapshots expire.
Run cleanup when no Scopelet operation is using those artifacts.

Remove the skill using `npx skills remove scopelet -a codex claude-code`; remove
a Cargo install with `cargo uninstall scopelet`. The launcher cache can be
deleted separately. No telemetry, proxy configuration or agent hooks remain.
