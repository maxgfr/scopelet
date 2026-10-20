# Design and contracts

Scopelet computes on evidence before presenting it to an agent. It does not
intercept API traffic, modify model internals, rewrite conversation history or
call another model to compress data.

`Source -> Dataset -> operations -> immutable artifact -> bounded View` is the
single pipeline. The Rust library exposes the same types used by the CLI.
Request version is 1. Saved artifacts are schema 1 (records stored) or, for
automatic compression and for a recovery whose only record is a
whole stored text original (a blob paged or searched whose view is all of it,
such as a single long line), schema 2 (records named by their original blob).
Unknown request fields fail.

Source loading owns scope and freshness. A dataset carries full records,
snapshots, the number of examined files/streams and observed skip counts.
Filesystem ignores define the search domain; ignored descendants are not
enumerated and their count is not guessed. Read failures and resource caps make
the scan incomplete. A repository is enumerated in path order from metadata,
and the file and byte caps apply in that order; files are then read, checked,
hashed and searched on up to eight worker threads (`SCOPELET_WORKERS` sets the
count) and assembled in path order, so the result and its artifact ID do not
depend on scheduling. Every scanned file gets a snapshot naming its content
hash, but only a file that produced a record has its original stored. External syntactic indexes and document extractors do not
certify completeness, so their datasets explicitly say so.

Operations own transformations. A count is the number of records at that stage,
not necessarily files, regex occurrences or all possible real-world entities.
JSONL ingestion is transactional; malformed input cannot produce a partial
aggregate. JSON numbers retain their source decimal representation. Groups
sort by key, or by decreasing count (ties by key) when the operation asks for
`order: count`, as the `--group` shortcut does. After `project`, records are
keyed by the pointers themselves, so a pointer that resolves nowhere falls back
to that literal key. `--top N` limits a JSON view to its first records; the
artifact keeps them all and the view counts them as omitted. Computed
values retain source provenance through the full artifact's snapshots.

Presentation owns the byte budget. `scan_complete` describes source traversal;
`display_complete` means the complete selected result is in this response with
no abridgements. `omitted_records` counts records absent from this particular
page, including previous pages. `next_offset` advances through the full result. When a single record cannot fit,
`blocked_record` identifies it and `next_offset` is null to prevent a retry loop.
An oversized record is not split in default: expand its blob by lines or raise
the budget. Ultra can abbreviate large text units and records omitted line
counts. A single line longer than its limit is cut on a character boundary and
marked `text_truncated`; a displayed record is never empty. Its information loss
is intentional and recoverable, not assumed safe. `expand --manifest` obeys the
same budget: it lists the first snapshots that fit and reports how many exist.

The artifact ID hashes the complete serialized dataset. Source blob IDs hash
original bytes. A schema-2 artifact carries the dataset's metadata and
snapshots and, instead of `records`, `records_from: {blob, format, source}`;
its ID hashes that exact JSON, which names the blob, so it is as immutable as
a stored copy. Readers verify the artifact, then the blob, and parse the
records again with the recorded format; a missing or damaged blob fails that
read while the metadata stays readable. A dataset read back carries its
records, so saving a query over it stores a self-contained schema-1 artifact.
`expand --manifest` reports `artifact_schema_version` when it is not 1. An
older binary rejects a schema-2 artifact (`missing field records`) rather than
misreading it. `expand` retrieves an immutable snapshot; an artifact used as a
new query source checks recorded local source hashes first. A changed or
deleted local source fails that query, while its old blob remains recoverable.
When every record was read straight from a local file (a repository scan),
only those files must be unchanged; another scanned file that changed is
counted in `skipped.changed_since_scan` with a note. Computed records and
extractions depend on every source, which must all be unchanged. Recovery
search reads an original that was not stored from its local file while that
file still has the recorded hash; otherwise it counts
`skipped.snapshot_not_retained` and marks the search incomplete.
Web extraction snapshots describe extracted text, not original HTML/PDF bytes;
the external engine's envelope is retained separately.

Limits: requests 1 MiB, 32 operations, each file 32 MiB, repository scan 20000
files/128 MiB, stored item 256 MiB, command capture 32 MiB per stream, default
command timeout 120 seconds (maximum 3600). New store directories are private on
Unix; existing directory permissions are preserved; writes are atomic (a synced temporary renamed into place, data flushed to the device without forcing a full disk cache flush) and content hashes are checked on reads, so an interrupted write yields a missing or rejected item, never a wrong one. Publishing bytes that are already stored reuses the existing item by address and length without re-reading it; a stored item of the wrong length is replaced. Storage subdirectories get local `.ignore` and `.gitignore` markers so ordinary searches and Git staging do not re-ingest saved evidence. Existing markers and read-only caches are preserved; marker creation is skipped when permissions forbid it; explicit no-ignore searches can still include the cache. Storage grows
with distinct observations until explicit cleanup; no silent eviction expires
active references. Cleanup retains blobs referenced by surviving artifacts, and
paging an artifact does not store it again. It reports the items and bytes it
removed, the bytes it kept and the corrupt artifacts it found. A surviving
artifact whose bytes no longer match its name is removed as corrupt; one that
is intact but unreadable (a newer schema, a permission) is kept, and no
original is removed in that pass because the originals it names are unknown.
`--max-size` then evicts least recently modified items until the cache fits,
never one modified within the last hour, and removes an original only once no
surviving artifact names it; disposable offset indexes count and evict like
originals. Cleanup also removes per-context hook states (`<config>/sessions/`)
older than the age limit. Only content-hash-named files are
cache items: cleanup leaves anything else in the directory alone, apart from its
aged temporaries in the reserved `.scopelet-write-` namespace (12 alphanumeric
suffix characters). Legacy `.tmp*` files are left alone because their owner
cannot be established from the name. Run cleanup outside active operations.

Commands use argument vectors rather than shell interpolation, inherit the
caller's working directory/environment, and execute once. Stdin is closed.
On Unix, timeout/cancellation terminates the process group. Return codes are
preserved, with 124/130 for timeout/interruption. Capture overflow discards excess
bytes while draining the command; its exit status is preserved and completeness
is false. Parse errors return raw references and the original command exit code. This is not a
sandbox; use the host agent's permission boundaries. Stdout/stderr retain
separate originals; their real-time interleaving is not reconstructed. Text capture
uses adaptive blocks, at most 10,000 per stream, to bound record overhead on
newline-heavy output. Descendants that retain pipes after their parent exits
receive bounded cleanup; escaped process groups may survive, but cannot hold
capture open indefinitely and the result reports incomplete drainage.

The adapters are external executable contracts. Codeindex's symbols schema 5
is checked before use; invalid spans or paths outside the repository fail.
Webindex results require extracted text and retain extraction metadata. The
native core does not require Node or those adapters; the skill launcher and
optional engines do. No external compressor runtime is vendored.

Token savings are a session-level experimental outcome. Byte reductions alone,
identical repeated blocks, or a smaller final answer cannot prove a cheaper
session. Compare equivalent tasks and preserve failed runs in the denominator.

## Automatic integration

Automatic hooks are an optional local installation, independent of skill
activation. They use the same immutable store and do not alter API requests or
conversation history. Default and caveman share the compression engine; caveman
only changes the host's concise response instruction. Off returns native calls.
Routine-edit guidance asks the agent to inspect relevant code and existing
checks, preserve intended behavior and verify the change, and, before writing
code, to reuse existing code, the standard library or installed dependencies
and make the smallest change that passes existing checks. Default asks for
1–3 short outcome/validation sentences; caveman targets 30 words with explicit
exceptions for required information and requested detail. Both end with the
recovery line `Recover: BIN expand ID --find TEXT [--regex] [--ignore-case]
(ID or last)`; with a 45-byte pinned path they fit 380 and 430 bytes. In Claude
Code a `SubagentStart` hook gives each subagent the recovery line and asks its
final report for conclusions with `file:line` evidence, without restating
commands or raw output, since that report becomes its parent's input. These
are preferences, never forced truncation or a reason to omit verification.
Host settings are merged, original bytes backed up, and only exact Scopelet hook
entries are removed. The resolved executable is copied during installation so
hooks never trigger downloads. Host permission/trust mechanisms remain in force;
Codex's input-rewrite protocol requires an explicit hook `allow` decision, but
Scopelet never changes permission rules, mode, sandbox or escalation parameters.
In Codex 0.160 that decision only carries `updatedInput`: the rewritten call
then goes through Codex's normal approval and sandbox policy (only a
`PermissionRequest` hook can approve a call), so wrapping never grants a
command more than it had.

The version-1 JSON request/artifact/view contracts remain unchanged. `--output
compact` is a separate textual presentation (see [Compact format](#compact-format)),
not a JSON schema migration. Every compact view links the full dataset artifact; its
manifest maps source labels to immutable blobs. A unit is a complete JSON record
or consecutive identical text lines represented by an exact line plus a repeat
count. Source line labels remain absolute; ordered gaps and omitted-unit counts
make selection visible. Formatting prefixes and repeat markers are metadata,
never bytes claimed to occur in the source.

Uniform JSON objects first get a complete representation with shared `columns`
and positional `rows`. `record_sources` retains per-row provenance when labels
differ. Every key must exist in every row; missing fields never become null.
Values retain JSON types and exact decimal numbers. If the full representation
cannot fit, the engine selects whole records rather than clipping fields.
Text presentation preserves first/final lines, then selects diagnostic lines
before context and ordinary lines; a single oversized line is cut on a
character boundary and marked (see below). Only one view substitutes a
recovery-only envelope: an output identical to one the same conversation
context was already shown becomes a one-line repeat reference that recovers it
(see Repeat references). Every other view carries source bytes.
All originals remain available. Automatic detection of malformed JSONL falls
back to text selection and never asserts an exact aggregate.

Automatic inputs up to 2048 bytes remain unchanged (16 KiB for a recognized
file read). Larger inputs have a 4096 byte target per stream (the recognized
command's profile target when there is one) and need both 512 bytes and 20%
savings including all metadata before replacement. Non-UTF-8 input, existing
Scopelet output and host persisted-output previews remain unchanged; those
markers are recognized only at the start of a line, where they are written, so
a source file quoting `[scopelet ` is compressed like any other text. Inputs
over 250000 lines (where the light units keep the peak resident size under
128 MB with a full 32 MiB stream) bypass automatic compression to bound
selection memory; explicit compact queries reject more than 100000 units. Byte
thresholds are not tokenizer or session savings.
The original capture limit remains 32 MiB per stream. Capture incompleteness is
reported outside the compressed view; a hook only retains bytes supplied by its
host and cannot recover an earlier truncation.

Automatic runs and Claude hooks avoid opening the store when both streams fit
the small-output threshold. Codex also bypasses the wrapper for a plain `cat` or
`nl` of a regular file whose metadata reports at most the file-read threshold
(16 KiB). Relative paths require
a known absolute working directory. Metadata only chooses whether to compress:
the native command still reads the current file and enforces host permissions.
A file growing after the check stays correct, though that read may miss savings.

`run --auto` executes once and returns stdout/stderr independently, preserving
exit status. Failed storage/compression returns captured native bytes; the
command is never rerun. Its stdin remains closed, so only noninteractive commands
belong in this path. The Codex adapter only wraps the shell subset described
below and skips recognized interactive flags. Claude's adapter
replaces only known Bash output shapes after execution, and the `content` text
of a Grep in `content` mode (compressed as search results; its
`files_with_matches` and `count` modes and unknown shapes pass through), and
keeps all other fields. The Grep shape was captured from Claude Code 2.1.296
(`tests/fixtures/claude_grep_*.json`); a live run confirmed the model receives
the replaced text. Results carrying a nonempty `persistedOutputPath` or positive
`persistedOutputSize` pass through before Claude renders its own preview, so the
host cannot truncate an already compressed view again. Failure events without
a replaceable result and unknown host/tool shapes pass through unchanged.
OpenCode has no hook protocol; a generated OpenCode 2 plugin file mutates the
`shell` tool's text result in place from the tool `execute.after` hook, under the same
thresholds, by calling the pinned binary. When OpenCode has already truncated
a shell result, the plugin compresses the whole output OpenCode saved, read
only as a regular file inside OpenCode's own shell output directory for the
project and within the input limit. Any plugin or binary failure leaves
the native output untouched, and the mode text reaches the model through the
system prompt on every step rather than through per-session hook context.

A Codex command is parsed in a small POSIX subset (`src/shell.rs`) that reads
the same in `sh`, `bash` and `zsh`: words may be single quoted, double quoted
without `$` or backticks inside (a backslash there only before an ordinary
character, where it stays literal: `"\d+"`), or backslash-escaped; a `~`
inside a word (`HEAD~1`) is literal; literal `NAME=value` prefixes,
`cd DIR &&`, `&&`, a final `|| true`, pipes, `2>&1` and output or errors to
`/dev/null` are accepted. Expansions (`$`, backticks, globs, a `~` starting a
word or following any `=` or `:`, braces), grouping, `;`, a lone
`&`, `|&`, `&>`, `>|`, input and
other redirections, heredocs, newlines, and words starting with `=` or `#`
leave the command native. Every command in it must be recognized; after a pipe
only read-only filters are allowed (`grep`, `rg`, `sort` without `-o`, `uniq`,
`cut`, `tr`, `nl`, `cat`, `jq`, `sed -n` with a print-only script, `head`,
`tail` without `-f`, `wc`), and a pipeline that already ends in `head`, `tail`,
`wc` or `grep -c`/`-l`/`-q` is left alone. Prefixes are limited to display,
locale and test variables, never one that loads or runs a program. A single
command is executed from its argv; an AND-list of at most eight plain commands
is rebuilt from quoted argv in `/bin/sh`; anything else runs as the original
string through `/bin/sh -c`, so short-circuiting, pipes and the exit status
are the shell's. Interactive/background tool calls pass through when the host
exposes those flags.

## Execution, journal and recovery

The query and saved-dataset schemas remain version 1. The execution planner
recognizes JSONL filter chains ending in count/group and repository queries
starting with search. It streams those operations while retaining every source
snapshot, original scan order, exclusions and limits. JSONL parsing completes
before a result or a deferred grouping error is exposed. Other compositions use
the materialized pipeline. Searches compile once, retain record-wide `all`
semantics and preserve multiline matches. No index substitutes for source-hash
verification or certifies an unvisited source.

Automatic compression prepares a view before opening storage. Rejected views,
small/binary inputs and persisted previews perform no store writes; each
decision appends one line under 512 bytes to the local event journal,
`<cache>/events/YYYY-MM.jsonl` (time, host, profile, byte counts, a typed reason
`small`, `non_utf8`, `too_many_lines`, `already_compressed`, `savings`,
`persisted`, `compressed`, `repeat` or `error`, 12-character prefixes of the
artifact and original, `anchored` when relevance anchors
ranked lines up, and `program`, the bare name of the program that printed the
output: the first recognized command's, else the first command's, `other` when
it is not a plain name of at most 32 characters; `expand` adds one line with
the reference prefix and how it read the evidence: `find`, `range`, `page`,
`manifest` or `raw`). No content, argument, path or anchor term is recorded,
nothing is transmitted, writing is best effort, and `SCOPELET_EVENTS=0`
disables it. Journals written before 1.0 also carry the compact version of each
decision; they still parse. `stats --days N` summarizes compressions, bytes saved,
pass-through reasons, the share of compressions whose artifact or original was
expanded later, expansions by kind, repeats and the bytes they saved, anchored
views, compressions by program with the ten programs most often compressed
without a profile (`unprofiled_programs`), and, per profile, the same figures with
`budget_probably_tight` when more than 25% of at least four compressions were
expanded. Stats report; nothing is tuned automatically. Accepted
views save original bytes and serialize the same dataset through a bounded,
buffered hashing writer; artifact identities remain SHA-256 of the exact JSON
serialization. Automatic compression stores a schema-2 artifact that names
the original blob, so an accepted view caches the original once plus well
under a kilobyte; an explicit compact query stores its whole result as a
schema-1 artifact. A storage/compression failure returns native captured bytes.
Explicit queries still report storage errors.

`expand ID --find TEXT [--find TEXT] [--regex] [--ignore-case] [--context N]
[--source LABEL]` searches immutable original blobs, including evidence no
longer present in a filtered artifact's records. It uses literal alternatives
(regular expressions with `--regex`; a search spec's additive `ignore_case`
backs `--ignore-case`) and absolute source lines,
returns ordinary paginated JSON views, and labels the data as historical.
`--source` selects an exact artifact snapshot label; unknown labels fail.
Search conflicts with raw/manifest/range expansion. This is text-window
recovery, even for a JSON original; structured computations belong in `query`.
No local freshness check is implied by explicit recovery.

`expand` and artifact query sources resolve what a person types: a full
reference, a bare 64-character hash (an artifact before a blob), a unique
prefix of at least 8 hex characters (an ambiguous one fails with the number of
candidates), or `last`. Each accepted automatic compression atomically
records its artifact in `<cache>/last`; with concurrent hosts it names the
latest write. A blob is paged by lines: when the requested lines do not fit,
the view holds the first lines that do and `next_start`; only a single line
larger than the budget remains a `blocked_record`. `--offset` pages records
and conflicts with a line range instead of being ignored. Marking an item as
used is best effort, so a read-only cache stays readable.

Blob range recovery optionally caches offsets for at most 100000 lines in
`line-index-v1/`. These disposable indexes have local ignore markers and bind
offsets to a blob hash. Recovery verifies original bytes and all index line
boundaries; missing, invalid or unwritable indexes fall back to reconstruction.
Larger sources use bounded sequential range selection. Cleanup removes aged
indexes and indexes whose original blob is gone, preserving foreign files.

The shared command classifier recognizes builds, tests and checks (`cargo test|check|clippy|build|nextest|doc`,
`cargo fmt --check`, `pytest`, `python -m pytest|unittest|mypy`, Python scripts,
`npm`/`pnpm`/`yarn` test/build/lint/typecheck, `bun test`, `make
test|check|build|lint`, `go test|build|vet`, `ruff`, `mypy`, Gradle/Maven/.NET
test or build, `node --test`, and `jest`, `vitest run`, `tsc`, `eslint`,
`prettier --check` or `playwright test` through `npx` (also `-y`), `bunx`,
`yarn`, `pnpm exec` or
`uv run` (after uv's own options, and `python -m pytest|unittest|mypy` too));
package installs (`npm`/`pnpm`/`yarn`/`bun` `install|i|ci|add`, `pip install`,
`python -m pip install`, `uv sync`, `uv pip install`, `brew install|upgrade`,
`bundle install`); searches and listings (`rg`, `grep`, `find` without
`-delete`/`-exec`/`-ok`/`-fprint`, `ls -R`, `tree` without `-o`, `git
ls-files`); Git `diff`, `log` (with
`-p`), `show`, `status`, `blame` and `grep` with no option before the
subcommand other than `--no-pager` or `-C DIR` (`-c` can name a program) and
none that pages, runs a program or writes a file (`--paginate`, `--ext-diff`,
`--textconv`, `--output`, `grep -O`); logs (`docker
logs`/`kubectl logs` without `-f`, `jq` on a file); and file reads (`cat`,
`nl`, `head`/`tail` without `-f`, print-only `sed -n`). Scopelet itself and
`rtk` are recognized by the program they run, not by a substring.
Each recognized command has a profile that sets its byte target: 4 KiB for
tests, 8 KiB for searches, Git and logs, 16 KiB for file reads and 2 KiB for
installs. A file read also stays exact up to 16 KiB, and it is not ranked by diagnostic
words (a string saying `error` in source code is not an outcome). Hooks take
the profile from the parsed command (Codex passes `run --auto --profile`;
Claude Code and OpenCode classify the command after the fact). Classifying
after the fact chooses a profile only, so it also reads lines the execution
grammar rejects: it splits them on `;`, `&&`, `||`, `|` and newlines outside
quotes, drops redirections and leading assignments, and takes the first
recognized command, where `cat` of several files or with `-n`/`-b`/`-s`/`-v`
and `nl` with its numbering options also read files (`sed -n 1,80p a; sed -n
1,80p b`, `cat a b`, `nl -ba f | sed -n 40,90p`). Codex's grammar and what it
wraps are unchanged. The shell grammar and permission envelope remain in
force. Watch/debug/interactive forms and unrecognized syntax stay
native. Every eligible command executes once; stdout/stderr and process status
remain independent of presentation.

## Compact format

Hooks, `run --auto`, `compress` and `--output compact` write one format. The
query and saved-dataset schemas remain version 1.

**Header.** One line and no footer: `[scopelet v5 ID complete]`, or
`[scopelet v5 ID omitted=N abridged=M; gaps are not absence]`, with
` scan_complete=false` after the ID only when the scan was incomplete. `ID` is
the first 12 hex characters of the artifact hash; `expand` resolves it like any
unique prefix and fails, naming the count, when two cached items share it.
`omitted` counts units left out; `abridged` counts units shown in part: a line
cut behind `text_truncated`, a `similar=N` group (its members differ from the
line shown; an exact `repeat=N` hides nothing), or a record shown without some
of its fields. A view is `complete` only when both are zero. The recovery
syntax is not repeated in every view: the session guidance (`SessionStart`,
again after a compaction, `SubagentStart`, OpenCode's system prompt) carries
it. Budgets reserve the widest header the view can emit, at its real size. A
line starting with `[scopelet ` still marks an output as already compressed,
whatever version wrote it.

**Labels.** A view whose every record is the one `input` text labels units by
line alone: `12 text`, `4-6 repeat=3 text`, `4 similar=480 last=492 text`,
`1-3` above a range block's indented lines, `1-9 file=src/a.rs more=2` for a
search block and `9-95 file=Cargo.lock changed=+43/-3 omitted` for a collapsed
diff. A label still starts every line that is not indented, so a line's own
text never reads as a label. Multi-source views (repository queries, `run
--output compact`) keep `source:` labels. Selection measures each unit as it is
written, so the view is rendered once per refill pass.

**Display copies.** Text units are built from a display copy of each source
line: terminal control sequences (CSI, OSC and two-byte escapes) are removed
and only the last carriage-return segment of a rewritten line is kept. The copy
never changes the number of lines, so labels stay absolute and `expand --find`
still searches the original bytes. A single line larger than the budget is cut
on a character boundary behind `text_truncated bytes=N`, where N is the
original line length without its terminator, so a giant line does not force
the whole stream through unchanged. JSON records are never split; when no
whole record of an automatically compressed JSON document fits (one large
object), its source lines are selected instead, with absolute labels into the
same original, rather than passing the document through.

**Folding.** Repetitions fold wherever they occur. `a-b repeat=N` (N = b−a+1)
is a contiguous run of one text; `a repeat=N last=b` (N < b−a+1) is the same
text at N dispersed lines, shown at its first occurrence; `a similar=N last=b`
groups N lines that share a template after masking digit runs, decimal
numbers and measurements with a unit (`12.4s`, `42ms`, `1.2KiB`, `37%`), UUIDs,
`0x` addresses, hex runs of seven or more characters that contain a digit, and
whitespace runs. Only lines without a diagnostic fold by template: diagnostics
fold by their text with timestamps masked, so a flood of one error differing
only by its time folds while counters and identifiers in an error stay
visible. A record whose text fits the budget with one indent byte per line and
a few labels folds identical lines only: the numbers that tell similar lines
apart are often the evidence, and showing them costs no more than the budget
already allows. When at least 90% of a record's lines (20 or more) sit in
folded groups, its remaining singletons are promoted ahead of context, because
the rare line among noise is usually the evidence being looked for. All of
these markers are metadata, never bytes claimed to occur in the source.

A trivial line has at most three visible characters and no diagnostic (`}`,
a blank line, a `|` gutter, an `rg` `--` separator). When the nearest
nontrivial line on either side is shown in place (it is not folded into an
earlier occurrence), the trivial line is glued: it never joins a distant
copy, folds only with contiguous identical neighbours, and ranks with the
lower priority of its in-place neighbours. It is therefore shown when the
lines it separates are, so a read of source code keeps its braces and blank
lines inside range blocks. A trivial line between lines shown elsewhere has
nothing to hold together and folds by exact text like any other line; it is
never promoted as the rare line of a mostly folded record. Glue reaches three
lines, the context Git shows around a change.

**Ranking.** Units rank in five tiers: the first and last line of a record; the
first occurrence of each strong diagnostic template (`error`, `failed`,
`panic`, `exception`, `traceback`, `fatal`, `npm ERR!`, `FAILED`, Go's
`--- FAIL`, `✗`, `✖`, error and exception type names such as `TypeError` or
`NullPointerException`, test summaries and the like); repeated strong
diagnostics, the first occurrence of each weak template (`warning`, `warn`,
`deprecated`) and singleton lines of a mostly folded record; context (three
lines before and eight after a strong diagnostic, stack frames of JavaScript,
Python, Java and Go, rustc's `-->` locations, the head three and tail five
lines); then ordinary lines. A vocabulary word inside a name does not count:
a match within a whitespace-delimited token that contains `/` or `::`, or
ends with a file extension, is a path or an identifier (`src/error.rs`,
`Error::new`, `failure.py`), not an outcome. Each tier is filled alternately
from the head and the tail so the final summary survives a flood of early
diagnostics. When the view carries diagnostics and cannot show every ordinary
line anyway, ordinary lines stop at a quarter of the available budget, so the
view ends where the evidence does; an ordinary listing without diagnostics
still fills the budget. Small repetitions inside a diagnostic's context stay
in source order; massive repetition (eight lines or more) folds wherever it
is. Vocabulary words match without regard to case on ASCII word boundaries,
but anchored markers (`FAIL`, `FAILED`, `FATAL`, pytest's `E` prefix) and type
names must keep their capitals, so an ordinary line beginning with `e ` is not
a diagnostic. Diagnostics in JSON string values rank by the same vocabulary
and veto. The output is restored to source order.

A stack frame or location that is not folded ranks with the diagnostic it
locates, among the first diagnostics, when the nearest nontrivial line above
it, within three lines, is the first diagnostic of its template: rustc's `--> file:line:col` under its `error[...]` line, a trace's
first frame under its exception. Ranked with context, the error's location
was lost among the locations of a flood of warnings, each with its own
`-->`: a build of 2,000 warnings and one error kept the error and dropped
where it was. The rule does not chain, so a Java or Python trace lifts one
frame and the others stay context.

Three or more selected consecutive plain lines are shown as one range block:
`a-b` on its own line, then each line prefixed by a single space, so line a+k
is the k-th indented line. The label bytes this saves are spent on further
evidence when it fits. A block header carries no `repeat=`, which
distinguishes it from a contiguous run.

**Diffs.** A record containing a unified diff (`diff --git`, or `---` then
`+++` then `@@`) is read by role. Lines inside it are never diagnostics: code
that mentions `error` is a change. File headers (`diff --git`, the `---`/`+++`
pair of a plain diff, and `git log -p`'s `commit <hash>`) never fold by
template, only with identical text, and rank as context does. Changed lines
and commit messages fold only with identical text; a distinct one ranks with
context, one repeated across files or commits below it. Context lines are glued
like trivial lines. A hunk header glues to the first change below it however
much context lies between (Git's default three lines put it four lines away)
and ranks with that change, so a hunk is shown with the change it locates; a
file header ends what can glue and metadata lines (`index`, modes, renames,
`Author:`, `Date:`) are transparent to it. A `diff --git` file that is a
lockfile (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`,
`poetry.lock`, `uv.lock`, `go.sum`, `Gemfile.lock`, `composer.lock`) or a
minified or map file (`*.min.js`, `*.min.css`, `*.map`) becomes one line,
`a-b file=PATH changed=+A/-D omitted`; its lines lose change protection and
count as omitted. In a diff of several files with no commit headers, each tier
fills in order of the files' change density (changed lines per line of their
section), densest first; `git log -p` keeps head and tail alternation.

**Repeat references (Claude Code).** Each conversation context (the session,
and each subagent separately, by `agent_id`) keeps a private state under
`<config>/sessions/`: the mode it was told, 16-character hash prefixes of at
most 512 outputs it was shown (oldest forgotten first), and its prompt anchors.
A Bash stream or Grep content over 2048 bytes (even a file read) whose hash is
in that state is replaced by `[scopelet v5 ID repeat: identical to output
already shown in this conversation (N lines)]`; its original and artifact are
stored as a view's would be (the same bytes give the same artifact), so it is
recoverable even when the first occurrence passed through raw. Every other
output is remembered once shown, as a view or raw; a host persisted preview is
neither replaced nor remembered. A `SessionStart` with source `compact` or
`clear` forgets what was shown (a clear also drops the anchors). Only exact
identity counts: two runs that differ in a timing or a thread id are two
outputs. Codex and OpenCode, which signal no compaction, get no repeat
references. A repeat is journaled with the bytes it saved. An earlier
bare-mode marker is read as that mode.

**Relevance anchors.** On `UserPromptSubmit`, the prompt's quoted text, paths
and file names, snake_case and CamelCase identifiers and error codes (3 to 64
bytes, at most 32, newest kept) are stored in the context's private state,
never in the journal. Bare numbers (lines, counts, years) and tool call ids
(`toolu_...`) match lines by accident and are never terms, even ones an earlier
version stored; a prompt the host generates (`<task-notification>`) adds none.
A tool call adds the terms its command names: the literal pattern of `rg`,
`grep` or `git grep` (a regular expression with operators names nothing; `|`
branches split), and the filter of `cargo test NAME`, `pytest -k`, `jest -t`
or `go test -run`; a subagent also gets its session's prompt terms. Codex
passes its context key to the wrapped run (`run --auto --session KEY`);
OpenCode passes its session ID (its plugin sends no prompt event, so only
command terms apply there). A term found on more than half of an output's lines singles
nothing out and is ignored for that output. A unit containing a remaining term
ranks with first diagnostics while anchored units fit a third of the budget. A
unit lifted by an anchor alone is relevant, not a diagnostic: it never caps
ordinary lines at a quarter of the budget, as a real diagnostic does. File
reads take anchors too. The view depends on the anchors; the artifact does not.

**Tests.** A view of the tests profile whose output contains recognized
failures gets a metadata line below the header: `failed=N: name (file:line),
...`, at most five entries then `+M`. Recognized: cargo test (`---- name stdout
----` and its panic location), pytest (`FAILED`/`ERROR` summary lines and the
deepest `file.py:N:` of their section), jest and vitest (`●` or `FAIL file >
name` titles and their first frame outside `node_modules`) and go test (`---
FAIL:`, or a test that panicked, with its log line or the deepest frame
outside the Go toolchain). An unrecognized format gets no line. A cargo run
over two or more test binaries also gets `totals runs=N passed=P failed=F
ignored=I (every test result line, omitted ones included)`, summed from every
`test result:` line, since a view rarely has room for all of them and summing
the ones shown gives a wrong count. The `test result:` lines themselves rank
with first diagnostics and never fold by template, so the results in view add
up to the totals whenever they fit: when they did not, live sessions trusted
their own partial sum over the line, and the 14 tokens of the parenthesis
are worth keeping. Both lines are computed from the output and never claimed
to occur in it; together they take at most a quarter of the budget, and one
that does not fit is left out.

**Search results.** In the search profile, two or more consecutive selected
lines of one file in `path:N:text` / `path-N-text` form (`grep -n`, `rg -n`,
with context) are shown as `a-b file=PATH` then each line without its `PATH:`
or `PATH-` prefix, line a+k being the k-th. The path is metadata, the rest of
each line source bytes. Each file's first and last match, its anchored matches
and then its earliest, up to five, rank before any file's other matches, which
fill only leftover budget; a file's first block counts its matches not shown
as `more=N`. A range block never runs from one file into the next.

**Listings.** A `find`, `git ls-files`, `ls -R` or `tree` listing too long to
show whole summarizes each directory of more than eight entries as `DIR/
files=N ext=E:n,...` (four extensions, then `+K`) before its first entry, and
shows only its first and last entries; smaller directories stay whole. A
listing that fits is not folded, whatever directories it names.

**Installs.** The install profile (2 KiB) shows diagnostics, their context, the
first and last lines and any closing summary (`added N packages`,
`Successfully installed`, `Resolved N packages in`, `Bundle complete`, `Done
in`...), never ordinary progress.

**Tables.** Uniform JSON objects first get a complete table; if it cannot fit,
whole rows are selected by the ranking above. Columns require identical keys
across every row, including missing/null distinctions: no missing field
becomes null and decimals stay exact. Columns identical in every record are
named once under `constant`. A table whose cells and constants are all
scalars, and whose records' sources are `input` or follow from their index
(`input#record=I`), is written `table COLUMNS rows=N` (complete) or `table
COLUMNS rows=N/TOTAL indices=RANGES` (partial; zero-based positions, `a-b` for
three or more consecutive indices), then `constant KEY=VALUE,...` and one CSV
row per record. Numbers, booleans and null are written as JSON writes them, so
digits are exact; a string is bare unless it is empty, edged by whitespace,
holds `,`, `"`, `=`, `\` or a control character, starts with `[` or `{`, or
spells a number, a boolean or null, in which case it is a JSON string. A table
with a nested cell, or with other record sources, is a JSON line, `table
{"columns":[...],"constant":{...},"indices":[...],"record_sources":[...],
"rows":[...],"total_records":N}`, the indices and total only when partial and
the sources only when labels differ. Which rows a table shows is decided on
its JSON form, which the CSV form never exceeds. A partial table is not an
aggregate and never establishes absence.

An object whose array of same-keyed objects holds more than 80% of its bytes
(`kubectl -o json`'s `items`) is shown, when every row fits, as `envelope
{...}` (its other fields) and `table=/POINTER COLUMNS rows=N` with its rows.
Minified JSON whose records are too large to select whole (a quarter of the
budget or more, such as `gh api` responses) is shown as a table of the short
scalar fields every object has, one link column kept (`html_url`, else `url`),
every other field of any object named under `omitted_columns` (an envelope
keeps its scalars and names its nested fields as `omitted_fields`), and as
many rows as fit, rows carrying a diagnostic or an anchor first; the rows shown
count as abridged and the others as omitted. Pretty-printed JSON keeps the line
view, whose folding keeps rare values visible.

### History

Before 1.0, `--compact-version` and `SCOPELET_COMPACT_VERSION` selected among
five formats. Compact-v1 framed a view with an artifact header and a footer;
v2 added partial JSON tables and distinct-diagnostic selection, and became the
default after a bounded live rollout; v3 introduced display copies, folding,
tiers, range blocks, command profiles and schema-2 artifacts; v4 the one-line
header with a 16-character ID, repeat references, relevance anchors, test
action lines and the search, listing, diff, install and JSON shapes above; v5
the 12-character header without recovery syntax, bare labels and CSV tables,
6.1% fewer view tokens than v4 at the same selection, plus two opt-in rules
(`artifacts`, folding build and dependency directories in every listing, and
`outline`, ranking a large file's definitions first) that a paired live
campaign found either costing calls or bringing nothing. 1.0 keeps v4's
selection written with v5's grammar, and the `-->` rule above. Views written
by any earlier version stay recognized as already compressed, and their
artifacts, schema 1 or 2, with 16- or 12-character IDs, stay readable by
`expand` and as query sources.
