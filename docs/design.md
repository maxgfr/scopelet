# Design and contracts

Scopelet computes on evidence before presenting it to an agent. It does not
intercept API traffic, modify model internals, rewrite conversation history or
call another model to compress data.

`Source -> Dataset -> operations -> immutable artifact -> bounded View` is the
single pipeline. The Rust library exposes the same types used by the CLI.
Request version is 1. Saved artifacts are schema 1 (records stored) or, for
compact-v3 automatic compression, schema 2 (records named by their original
blob). Unknown request fields fail.

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
originals. Cleanup also removes per-session hook markers older than the age
limit. Only content-hash-named files are
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

## Automatic integration and compact-v1

Automatic hooks are an optional local installation, independent of skill
activation. They use the same immutable store and do not alter API requests or
conversation history. Default and caveman share the compression engine; caveman
only changes the host's concise response instruction. Off returns native calls.
Routine-edit guidance asks the agent to inspect relevant code and existing
checks, preserve intended behavior and verify the change. Default asks for
1–3 short outcome/validation sentences; caveman targets 30 words with explicit
exceptions for required information and requested detail. These are preferences,
never forced truncation or a reason to omit verification.
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
compact` is a separate textual presentation (compact-v3 by default, v1 and v2
on request), not a JSON schema migration. Every compact view links the full dataset artifact; its
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
before context and ordinary lines. In v1 and v2, automatic compression passes
through when no whole evidence unit fits; compact-v3 instead cuts a single
oversized line on a character boundary and marks it (see below). No version
substitutes a recovery-only envelope: every view carries source bytes.
All originals remain available. Automatic detection of malformed JSONL falls
back to text selection and never asserts an exact aggregate.

Automatic inputs up to 2048 bytes remain unchanged. Larger inputs have a 4096
byte target per stream and need both 512 bytes and 20% savings including all
metadata before replacement. Non-UTF-8 input, existing Scopelet output and host
persisted-output previews remain unchanged. Inputs over 100000 lines (250000
in compact-v3, whose lighter units keep the peak resident size under 128 MB at
that count with a full 32 MiB stream) bypass automatic compression to bound
selection memory; explicit compact queries reject more than 100000 units. Byte thresholds are not tokenizer or session savings.
The original capture limit remains 32 MiB per stream. Capture incompleteness is
reported outside the compressed view; a hook only retains bytes supplied by its
host and cannot recover an earlier truncation.

Automatic runs and Claude hooks avoid opening the store when both streams fit
the small-output threshold. Codex also bypasses the wrapper for a plain `cat`
whose regular-file metadata reports at most 2048 bytes. Relative paths require
a known absolute working directory. Metadata only chooses whether to compress:
the native command still reads the current file and enforces host permissions.
A file growing after the check stays correct, though that read may miss savings.

`run --auto` executes once and returns stdout/stderr independently, preserving
exit status. Failed storage/compression returns captured native bytes; the
command is never rerun. Its stdin remains closed, so only noninteractive commands
belong in this path. The Codex adapter only wraps the shell subset described
below and skips recognized interactive flags. Claude's adapter
replaces only known Bash output shapes after execution and keeps all other
fields. Results carrying a nonempty `persistedOutputPath` or positive
`persistedOutputSize` pass through before Claude renders its own preview, so the
host cannot truncate an already compressed view again. Failure events without
a replaceable result and unknown host/tool shapes pass through unchanged.
OpenCode has no hook protocol; a generated plugin file mutates the `bash`
tool's `output` string in place from `tool.execute.after`, under the same
thresholds, by calling the pinned binary. Any plugin or binary failure leaves
the native output untouched, and the mode text reaches the model through the
system prompt on every step rather than through per-session hook context.

A Codex command is parsed in a small POSIX subset (`src/shell.rs`) that reads
the same in `sh`, `bash` and `zsh`: words may be single quoted, double quoted
without `$`, backticks or backslashes inside, or backslash-escaped; literal
`NAME=value` prefixes, `cd DIR &&`, `&&`, a final `|| true`, pipes, `2>&1`
and output or errors to `/dev/null` are accepted. Expansions (`$`, backticks,
globs, `~`, braces), grouping, `;`, a lone `&`, `|&`, `&>`, `>|`, input and
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

## Execution and compact-v2

The query and saved-dataset schemas remain version 1. The execution planner
recognizes JSONL filter chains ending in count/group and repository queries
starting with search. It streams those operations while retaining every source
snapshot, original scan order, exclusions and limits. JSONL parsing completes
before a result or a deferred grouping error is exposed. Other compositions use
the materialized pipeline. Searches compile once, retain record-wide `all`
semantics and preserve multiline matches. No index substitutes for source-hash
verification or certifies an unvisited source.

Automatic compression prepares a view before opening storage. Rejected views,
small/binary inputs and persisted previews perform no cache writes. Accepted
views save original bytes and serialize the same dataset through a bounded,
buffered hashing writer; artifact identities remain SHA-256 of the exact JSON
serialization. Compact-v1 and v2 store schema-1 artifacts, byte for byte as
before; compact-v3 stores a schema-2 artifact that names the original blob, so
an accepted view caches the original once plus well under a kilobyte. A storage/compression failure returns native captured bytes.
Explicit queries still report storage errors. Compact-v2 became the CLI and hook default after a bounded live rollout
comparison, and compact-v3 replaced it as the default (see below); v2 remains
selectable and byte-identical. Legacy
library helpers `automatic` and `compact` retain v1 behavior.

`--compact-version 2` selects compact-v2; `SCOPELET_COMPACT_VERSION=2` selects it
for automatic hooks too. An explicit CLI version takes precedence. Only `1`, `2`
and `3` are accepted. Version 2 retains complete tables when they fit, otherwise
selects whole rows with `indices` (zero-based dataset positions), `total_records`
and positional `record_sources` when source labels differ. Columns require
identical keys across every row, including missing/null distinctions. Selected
cells and exact decimal representations remain intact; a partial table is not an
aggregate and never establishes absence. Heterogeneous records use ordinary
whole-record selection. The complete-table path is lossless in both versions.

V2 selection prioritizes boundary units, the first occurrence of distinct
signal text, additional signals, context and ordinary units. Diagnostics in
JSON are matched in string values, not field names. In v2, distinctness uses
exact text, without normalizing numbers or paths; v3 folds repetitions and
templates as described in its own section. Selection remains a heuristic:
omissions stay explicit, the output stays within the byte budget, and originals
remain recoverable. The output is restored to source order after selection.

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

The shared command classifier also recognizes simple `rg`/`grep`, non-paginated
Git diff/log/show/status, `go test`, `node --test`, and package-manager
build/lint/typecheck scripts. The existing shell grammar and permission envelope
remain in force. Watch/debug/interactive forms and unrecognized syntax stay
native. Every eligible command executes once; stdout/stderr and process status
remain independent of presentation.

## Compact-v3

Compact-v3 is the CLI and hook default. `--compact-version 3` and
`SCOPELET_COMPACT_VERSION=3` select it explicitly; `1` and `2` remain
selectable and their output stays byte-identical to the releases that
introduced them (`tests/performance_contracts.rs` pins both against 0.3.2).
Only `1`, `2` and `3` are accepted. The query and saved-dataset schemas remain
version 1, and the complete-table and partial-table paths are shared with v2.

The v3 header carries the artifact reference once; `ID` in its recovery hint
refers to that reference. The footer reserve is the exact width of the widest
footer the view can emit instead of a fixed margin, so the budget is spent on
evidence.

V3 text units are built from a display copy of each source line: terminal
control sequences (CSI, OSC and two-byte escapes) are removed and only the last
carriage-return segment of a rewritten line is kept. The copy never changes
the number of lines, so `input:N` labels stay absolute and `expand --find`
still searches the original bytes. A single line larger than the budget is cut
on a character boundary behind `text_truncated bytes=N`, where N is the
original line length without its terminator, so a giant line no longer forces
the whole stream through unchanged. JSON records are still never split; when
no whole record of an automatically compressed JSON document fits (one large
object), v3 selects its source lines instead, with absolute `input:N` labels
into the same original, rather than passing the document through.

Repetitions fold wherever they occur. `input:a-b repeat=N` (N = b−a+1) is a
contiguous run of one text; `input:a repeat=N last=b` (N < b−a+1) is the same
text at N dispersed lines, shown at its first occurrence; `input:a similar=N
last=b` groups N lines that share a template after masking digit runs,
decimal numbers and measurements with a unit (`12.4s`, `42ms`, `1.2KiB`,
`37%`), UUIDs, `0x` addresses, hex runs of seven or more characters that
contain a digit, and whitespace runs. Only lines without a diagnostic fold by
template: diagnostics fold by their text with timestamps masked, so a flood of
one error differing only by its time folds while counters and identifiers in
an error stay visible. When at least 90% of a
record's lines (20 or more) sit in folded groups, its remaining singletons are
promoted ahead of context, because the rare line among noise is usually the
evidence being looked for. All of these markers are metadata, never bytes
claimed to occur in the source.

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

A record containing a unified diff (`diff --git`, or `---` then `+++` then
`@@`) is read by role. Lines inside it are never diagnostics: code that
mentions `error` is a change. File headers (`diff --git`, the `---`/`+++`
pair of a plain diff, and `git log -p`'s `commit <hash>`) never fold by
template, only with identical text, and rank as context does (tier 2).
Changed lines and commit messages fold only with identical text; a distinct
one ranks in tier 2, one repeated across files or commits in tier 1. Hunk
headers and context lines are glued like trivial lines, so a hunk is shown
with the change it locates; a file header ends what can glue and metadata
lines (`index`, modes, renames, `Author:`, `Date:`) are transparent to it.

V3 ranks units in five tiers: the first and last line of a record; the first
occurrence of each strong diagnostic template (`error`, `failed`, `panic`,
`exception`, `traceback`, `fatal`, `npm ERR!`, `FAILED`, Go's `--- FAIL`, `✗`,
`✖`, error and exception type names such as `TypeError` or
`NullPointerException`, test summaries and the like); repeated strong
diagnostics, the first occurrence of each weak template (`warning`, `warn`,
`deprecated`) and singleton lines of a mostly folded record; context (three
lines before and eight after a strong diagnostic, stack frames of JavaScript,
Python, Java and Go, rustc's `-->` locations, the head three and tail five
lines); then ordinary lines. A vocabulary word inside a name does not count:
a match within a whitespace-delimited token that contains `/` or `::`, or
ends with a file extension, is a path or an identifier (`src/error.rs`,
`Error::new`, `failure.py`), not an outcome. Each tier is filled alternately from the head and the tail so the final
summary survives a flood of early diagnostics. When the view carries
diagnostics and cannot show every ordinary line anyway, ordinary lines stop at
a quarter of the available budget, so the view ends where the evidence does;
an ordinary listing without diagnostics still fills the budget. Small
repetitions inside a diagnostic's context stay in source order; massive
repetition (eight lines or more) folds wherever it is. Vocabulary words match
without regard to case on ASCII word boundaries, but anchored markers (`FAIL`,
`FAILED`, `FATAL`, pytest's `E` prefix) and type names must keep their
capitals, so an ordinary line beginning with `e ` is not a diagnostic.
Diagnostics in JSON string values rank by the same vocabulary and veto. The output is restored to source order.

Three or more selected consecutive plain lines are shown as one range block:
`input:a-b` on its own line, then each line prefixed by a single space, so
line a+k is the k-th indented line. The label bytes this saves are spent on
further evidence when it fits. A block header carries no `repeat=`, which
distinguishes it from a contiguous run.
