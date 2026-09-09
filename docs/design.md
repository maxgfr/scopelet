# Design and contracts

Scopelet computes on evidence before presenting it to an agent. It does not
intercept API traffic, modify model internals, rewrite conversation history or
call another model to compress data.

`Source -> Dataset -> operations -> immutable artifact -> bounded View` is the
single pipeline. The Rust library exposes the same types used by the CLI.
Request version and saved artifact schema are both 1. Unknown fields fail.

Source loading owns scope and freshness. A dataset carries full records,
snapshots, the number of examined files/streams and observed skip counts.
Filesystem ignores define the search domain; ignored descendants are not
enumerated and their count is not guessed. Read failures and resource caps make
the scan incomplete. External syntactic indexes and document extractors do not
certify completeness, so their datasets explicitly say so.

Operations own transformations. A count is the number of records at that stage,
not necessarily files, regex occurrences or all possible real-world entities.
JSONL ingestion is transactional; malformed input cannot produce a partial
aggregate. JSON numbers retain their source decimal representation. Computed
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
original bytes. `expand` retrieves an immutable snapshot; an artifact used as a
new query source checks all recorded local source hashes first. A changed or
deleted local source fails that query, while its old blob remains recoverable.
Web extraction snapshots describe extracted text, not original HTML/PDF bytes;
the external engine's envelope is retained separately.

Limits: requests 1 MiB, 32 operations, each file 32 MiB, repository scan 20000
files/128 MiB, stored item 256 MiB, command capture 32 MiB per stream, default
command timeout 120 seconds (maximum 3600). New store directories are private on
Unix; existing directory permissions are preserved; writes are atomic and content hashes are checked on reads. Storage subdirectories get local `.ignore` and `.gitignore` markers so ordinary searches and Git staging do not re-ingest saved evidence. Existing markers and read-only caches are preserved; marker creation is skipped when permissions forbid it; explicit no-ignore searches can still include the cache. Storage grows
with distinct observations until explicit cleanup; no silent eviction expires
active references. Cleanup retains blobs referenced by surviving artifacts, and
paging an artifact does not store it again. Only content-hash-named files are
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

The version-1 JSON request/artifact/view contracts remain unchanged. The new
`--output compact` is a separate textual compact-v1 presentation, not a JSON
schema migration. Every compact view links the full dataset artifact; its
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
before context and ordinary lines. Automatic compression passes through when
no whole evidence unit fits; it never substitutes a recovery-only envelope.
All originals remain available. Automatic detection of malformed JSONL falls
back to text selection and never asserts an exact aggregate.

Automatic inputs up to 2048 bytes remain unchanged. Larger inputs have a 4096
byte target per stream and need both 512 bytes and 20% savings including all
metadata before replacement. Non-UTF-8 input, existing Scopelet output and host
persisted-output previews remain unchanged. Inputs over 100000 lines bypass
automatic compression to bound selection memory; explicit compact queries reject
more than 100000 units. Byte thresholds are not tokenizer or session savings.
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
belong in this path. The Codex adapter intentionally skips shell expansions,
control operators, pipelines and recognized interactive flags. Claude's adapter
replaces only known Bash output shapes after execution and keeps all other
fields. Results carrying a nonempty `persistedOutputPath` or positive
`persistedOutputSize` pass through before Claude renders its own preview, so the
host cannot truncate an already compressed view again. Failure events without
a replaceable result and unknown host/tool shapes pass through unchanged.

A Codex AND-list (`cmd && cmd`) of at most eight individually recognized simple
commands is supported. It is reconstructed from quoted argv in `/bin/sh`;
short-circuiting and the final process status are preserved. Other shell control
operators and pipelines still pass through. Interactive/background tool calls
also pass through when the host exposes those flags.
