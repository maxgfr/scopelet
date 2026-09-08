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
counts. Its information loss is intentional and recoverable, not assumed safe.

The artifact ID hashes the complete serialized dataset. Source blob IDs hash
original bytes. `expand` retrieves an immutable snapshot; an artifact used as a
new query source checks all recorded local source hashes first. A changed or
deleted local source fails that query, while its old blob remains recoverable.
Web extraction snapshots describe extracted text, not original HTML/PDF bytes;
the external engine's envelope is retained separately.

Limits: requests 1 MiB, 32 operations, each file 32 MiB, repository scan 20000
files/128 MiB, stored item 256 MiB, command capture 32 MiB per stream, default
command timeout 120 seconds (maximum 3600). New store directories are private on
Unix; existing directory permissions are preserved; writes are atomic and content hashes are checked on reads. Storage grows
with distinct observations until explicit cleanup; no silent eviction expires
active references. Cleanup retains blobs referenced by surviving artifacts. Run cleanup outside active operations.

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
