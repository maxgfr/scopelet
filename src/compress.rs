//! Recoverable presentation. Selection borrows evidence; storage follows acceptance.
use crate::{clean, compact_table, encoding, model::*, sources::Parsed, store::Store};
use anyhow::{Result, ensure};
use std::{borrow::Cow, collections::BTreeSet, path::PathBuf, sync::LazyLock};

/// Line texts and templates are hashed by the megabyte; the default
/// hasher is built for untrusted keys and costs several times more.
type HashMap<K, V> = foldhash::HashMap<K, V>;
type HashSet<K> = foldhash::HashSet<K>;

pub const DEFAULT_BUDGET: usize = 4096;
pub const SMALL: usize = 2048;
/// Line limit of automatic compression in v1/v2 and of explicit compact
/// queries: selection memory grows with the number of units.
pub const MAX_LINES: usize = 100_000;
/// Compact-v3's lighter units keep the peak resident size of automatic
/// compression under 128 MB up to this many lines (measured with
/// `bench/performance.py --stress`).
pub const V3_MAX_LINES: usize = 250_000;

// Compact-v3 tuning. These are judgement calls, not derived constants: they
// were chosen to behave well on the shared fixtures in `bench/content.py` and
// are pinned by `tests/content_gate.rs`, so changing one has to be re-measured
// there rather than argued from first principles.
//
/// Longest prefix kept of a line cut by the budget. Matches the ultra-mode cut
/// in `render.rs` so both abridgements look the same to a reader.
const TRUNCATED_PREFIX: usize = 1024;
/// A repeated line inside a diagnostic's context stays in source order below
/// this count, so a short stutter next to an error still reads as a sequence.
const FOLD_IN_CONTEXT: usize = 8;
/// Smallest record in which "almost everything repeats" is a meaningful claim.
const FOLD_MAJORITY_MIN_LINES: usize = 20;
/// Share of a record that must be folded before its singletons are promoted,
/// in tenths.
const FOLD_MAJORITY_TENTHS: usize = 9;
/// Share of the budget ordinary lines may take beside diagnostics they cannot
/// all fit into, as a divisor.
const ORDINARY_SHARE: usize = 4;
/// How far a trivial or diff context line glues to the line it belongs with:
/// the three lines of context Git shows around a change.
const GLUE_REACH: usize = 3;
/// Consecutive plain lines needed before a range block pays for its header.
const BLOCK_MIN_LINES: usize = 3;
/// Passes that re-spend the bytes range blocks saved. Each pass can only add
/// units, and rendering is measured after each, so this bounds work rather
/// than correctness.
const REFILL_PASSES: usize = 4;
const PLACEHOLDER: &str =
    "artifact:0000000000000000000000000000000000000000000000000000000000000000";
static SCOPELET_MARKER: LazyLock<memchr::memmem::Finder<'static>> =
    LazyLock::new(|| memchr::memmem::Finder::new("[scopelet "));
static PERSISTED_MARKER: LazyLock<memchr::memmem::Finder<'static>> =
    LazyLock::new(|| memchr::memmem::Finder::new("<persisted-output>"));
static SIGNAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(error|failed|failure|panic|panicked|exception|traceback|warning|assertionerror|caused by|test result|tests? passed|tests? failed)\b|^\s*(FAIL|PASS|E\s+|FATAL|×|✕)").unwrap()
});
/// Compact-v3 vocabulary: diagnostics and summaries that decide an outcome.
/// Anchored markers stay case-sensitive: their convention is upper case, and
/// matching them loosely turns any line starting with `e ` into a diagnostic.
/// So do error and exception type names (`TypeError`, `KeyError`,
/// `NullPointerException`), whose convention is a capitalized identifier.
/// Word boundaries, case folding and indentation are ASCII: the vocabulary is
/// ASCII, Unicode boundaries cost a slower engine on every line, and Unicode
/// classes and folding cost compiling a larger one on every call (every hook
/// is a new process).
static STRONG: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i-u:\b(error|errors|failed|failure|panic|panicked|exception|traceback|fatal|assertionerror|caused by|test result|tests? passed|tests? failed)\b)|(?-u:\b[A-Z][A-Za-z]*(Error|Exception)\b)|npm ERR!|^(?-u:\s*)(FAIL|FAILED|--- FAIL|E {2,}|FATAL|×|✕|✗|✖)").unwrap()
});
/// Advisory lines: shown once per template, never ahead of a diagnostic.
/// A passing test is not an advisory. Listing it here grouped every `PASS`
/// line by exact text, which stopped a suite of passes from folding and let
/// them crowd out the failure's own detail.
static WEAK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i-u:\b(warning|warn|deprecated|deprecation)\b)").unwrap()
});
/// Stack frames: context for a diagnostic, wherever they are. JavaScript and
/// Python frames, Java's `at pkg.Class.method(File.java:N)`, Go's
/// `file.go:N +0x1d` and rustc's `--> file:line:col` locations. Indentation
/// and line numbers are ASCII; a Java identifier keeps Unicode `\w`.
static FRAME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"^(?-u:\s+)at .*\(.*:[0-9]+:[0-9]+\)|^(?-u:\s+)File ".*", line [0-9]+|^(?-u:\s+)at [\w$.<>/-]+\([^()]*\)(?-u:\s*)$|\.go:[0-9]+ \+0x[0-9a-f]+|^(?-u:\s*)--> \S+:[0-9]+:[0-9]+"#).unwrap()
});
/// A file extension at the end of a token: `.rs`, `.py`, `.java`.
static EXTENSION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\.[A-Za-z][A-Za-z0-9]{0,4}$").unwrap());

/// Whether a vocabulary pattern matches the line outside a name. A word
/// inside a path (`src/errors/e1.rs`), a module path (`Error::new`) or a file
/// name (`failure.py`) names something; it does not report an outcome.
fn diagnostic(pattern: &regex::Regex, line: &str) -> bool {
    pattern.find_iter(line).any(|m| {
        let start = m.start() + (m.as_str().len() - m.as_str().trim_start().len());
        let begin = line[..start]
            .rfind(char::is_whitespace)
            .map_or(0, |i| i + line[i..].chars().next().unwrap().len_utf8());
        let end = line[m.end()..]
            .find(char::is_whitespace)
            .map_or(line.len(), |i| m.end() + i);
        let token = &line[begin..end];
        let name = token.trim_end_matches(|c: char| c.is_ascii_digit() || ":,;)]}'\"`".contains(c));
        !(token.contains('/') || token.contains("::") || EXTENSION.is_match(name))
    })
}

/// Versions are ordered: every check of a version's feature is `>=` the
/// version that introduced it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Version {
    #[value(name = "1")]
    V1,
    #[value(name = "2")]
    V2,
    #[value(name = "3")]
    V3,
    #[default]
    #[value(name = "4")]
    V4,
    /// Compact-v4's selection, written in fewer bytes (see `Grammar`).
    #[value(name = "5")]
    V5,
}
impl Version {
    /// The number `--compact-version` and `SCOPELET_COMPACT_VERSION` take.
    pub fn number(self) -> u8 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
            Self::V3 => 3,
            Self::V4 => 4,
            Self::V5 => 5,
        }
    }

    pub fn configured(explicit: Option<Self>) -> Result<Self> {
        if let Some(version) = explicit {
            return Ok(version);
        }
        match std::env::var("SCOPELET_COMPACT_VERSION") {
            Ok(v) if v == "1" => Ok(Self::V1),
            Ok(v) if v == "2" => Ok(Self::V2),
            Ok(v) if v == "3" => Ok(Self::V3),
            Ok(v) if v == "4" => Ok(Self::V4),
            Ok(v) if v == "5" => Ok(Self::V5),
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            _ => anyhow::bail!("SCOPELET_COMPACT_VERSION must be 1, 2, 3, 4 or 5"),
        }
    }
    /// Compact-v3 and later: command profiles, display copies, folding and
    /// tiered selection. V4 only changes the envelope around the same body.
    pub fn profiled(self) -> bool {
        self >= Self::V3
    }
    /// Partial JSON tables with indices and provenance (everything after v1).
    fn tables(self) -> bool {
        self != Self::V1
    }
}

/// How compact-v5 writes what compact-v4 selects. Each part can be turned off
/// alone, so a probe can measure what it saves (`SCOPELET_V5_GRAMMAR`, a
/// comma-separated list of the parts to keep; all of them when unset).
/// Selection never depends on it: a v5 view shows exactly the units the v4
/// view of the same input shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Grammar {
    /// `labels`: a view of one `input` text drops `input:` from its labels.
    labels: bool,
    /// `header`: a 12-character ID and no recovery syntax, which the session
    /// guidance carries.
    header: bool,
    /// `tables`: a table of scalar cells as a header and CSV rows.
    tables: bool,
}

static V5_GRAMMAR: LazyLock<Grammar> = LazyLock::new(|| {
    let parts = std::env::var("SCOPELET_V5_GRAMMAR").ok();
    let has = |part: &str| {
        parts
            .as_deref()
            .is_none_or(|parts| parts.split(',').any(|p| p.trim() == part))
    };
    Grammar {
        labels: has("labels"),
        header: has("header"),
        tables: has("tables"),
    }
});

impl Grammar {
    fn of(version: Version) -> Self {
        if version >= Version::V5 {
            *V5_GRAMMAR
        } else {
            Self::default()
        }
    }
}

/// Conservative detection retained for callers; automatic ingestion reuses parsed values.
pub fn detect(text: &str) -> Format {
    Parsed::detected(text.to_owned()).format()
}

/// Why automatic compression replaced an output or left it unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// At most the exact-size threshold.
    Small,
    NonUtf8,
    /// More lines than automatic selection accepts.
    TooManyLines,
    /// Already a Scopelet view.
    AlreadyCompressed,
    /// The view would not have saved 20% and 512 bytes.
    Savings,
    /// A host's persisted-output preview.
    Persisted,
    Compressed,
    /// Compression failed; the original bytes were kept.
    Error,
    /// Identical to an output already shown in the same context: replaced by
    /// a reference to it.
    Repeat,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::NonUtf8 => "non_utf8",
            Self::TooManyLines => "too_many_lines",
            Self::AlreadyCompressed => "already_compressed",
            Self::Savings => "savings",
            Self::Persisted => "persisted",
            Self::Compressed => "compressed",
            Self::Error => "error",
            Self::Repeat => "repeat",
        }
    }
}

/// The outcome of one automatic compression, for the local event journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub reason: Reason,
    /// The artifact and original stored, when the output was replaced.
    pub artifact: Option<String>,
    pub blob: Option<String>,
    /// Lines were ranked up for containing a relevance anchor.
    pub anchored: bool,
}

impl Decision {
    fn unchanged(reason: Reason) -> Self {
        Self {
            reason,
            artifact: None,
            blob: None,
            anchored: false,
        }
    }
}

/// Share of the budget, as a divisor, that units ranked up by an anchor may
/// take: a term found on every line cannot flood the view.
const ANCHOR_SHARE: usize = 3;

/// Literal terms that mark a line as relevant to the task (compact-v4): the
/// prompt's and the command's. A unit containing one ranks with first
/// diagnostics (tier 3) while anchored units fit `ANCHOR_SHARE` of the
/// budget. The view depends on them; the artifact does not.
struct Anchoring<'a> {
    finders: Vec<memchr::memmem::Finder<'a>>,
    /// Bytes anchored units may still take.
    left: usize,
    /// Units ranked up so far.
    promoted: usize,
}

impl<'a> Anchoring<'a> {
    fn new(terms: &'a [String]) -> Self {
        Self {
            finders: terms
                .iter()
                .filter(|t| !t.is_empty())
                .map(|t| memchr::memmem::Finder::new(t.as_bytes()))
                .collect(),
            left: 0,
            promoted: 0,
        }
    }
    /// Keep only the terms that single lines out of `text`: one found on
    /// more than half of its lines (a search's own pattern in its results)
    /// would spend the anchored share on whatever comes first.
    fn focus(&mut self, text: &str) {
        if self.finders.is_empty() {
            return;
        }
        let lines = clean::line_count(text);
        self.finders.retain(|finder| {
            let found = text
                .split_inclusive('\n')
                .filter(|line| finder.find(line.as_bytes()).is_some())
                .count();
            found * 2 <= lines
        });
    }
    /// A new selection over `available` bytes.
    fn reset(&mut self, available: usize) {
        self.left = available / ANCHOR_SHARE;
        self.promoted = 0;
    }
    fn matches(&self, text: &str) -> bool {
        self.finders
            .iter()
            .any(|f| f.find(text.as_bytes()).is_some())
    }
    /// Rank a unit up when it contains an anchor and still fits the share.
    fn promote(&mut self, unit: &mut Unit<'_>) {
        if self.finders.is_empty() || unit.priority >= 3 || unit.size > self.left {
            return;
        }
        let found = match &unit.line {
            Some(line) => self.matches(line),
            None => unit
                .record
                .value
                .as_ref()
                .is_some_and(|v| self.matches(&v.to_string())),
        };
        if found {
            // Only a unit ranked below a diagnostic's tier was lifted by
            // the anchor alone.
            unit.anchored = unit.priority < 2;
            unit.priority = 3;
            self.left -= unit.size;
            self.promoted += 1;
        }
    }
}

/// The profile that shapes an output: a recognized command's, in compact-v3
/// and later. Earlier versions keep their single default shape.
pub fn effective_profile(
    profile: Option<crate::commands::Profile>,
    version: Version,
) -> Option<crate::commands::Profile> {
    // Install arrived with compact-v4: earlier versions keep the defaults
    // they gave an unrecognized command.
    profile.filter(|p| {
        version.profiled() && (version >= Version::V4 || *p != crate::commands::Profile::Install)
    })
}

/// How one output is compressed: its byte target, the size up to which it
/// stays exact, and whether it is a file being read rather than an outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shape {
    budget: usize,
    small: usize,
    reading: bool,
    /// The effective profile: the recognized command's in compact-v3 and later.
    profile: Option<crate::commands::Profile>,
}

impl Shape {
    fn plain(budget: usize) -> Self {
        Self {
            budget,
            small: SMALL,
            reading: false,
            profile: None,
        }
    }
    /// A recognized command's shape in compact-v3; `budget` overrides its
    /// target.
    fn of(
        profile: Option<crate::commands::Profile>,
        budget: Option<usize>,
        version: Version,
    ) -> Self {
        let profile = effective_profile(profile, version);
        Self {
            budget: budget.unwrap_or(profile.map_or(DEFAULT_BUDGET, |p| p.budget())),
            small: profile.map_or(SMALL, |p| p.small()),
            reading: profile == Some(crate::commands::Profile::FileRead),
            profile,
        }
    }
}

pub fn automatic(bytes: &[u8], store: &Store, budget: usize) -> Result<Vec<u8>> {
    automatic_with(bytes, Shape::plain(budget), Version::V1, &[], || {
        Ok(store.clone())
    })
    .map(|(output, _)| output.into_owned())
}

/// The store factory is not invoked for a rejected or ineligible substitution.
pub fn automatic_lazy<'a>(
    bytes: &'a [u8],
    path: Option<PathBuf>,
    budget: usize,
    version: Version,
) -> Result<Cow<'a, [u8]>> {
    automatic_with(bytes, Shape::plain(budget), version, &[], || {
        Store::open(path)
    })
    .map(|(output, _)| output)
}

/// `automatic_lazy` for the output of a recognized command: its profile's
/// budget unless `budget` is given and, for a file read, no diagnostic
/// ranking (a word like `error` in source code is not an outcome). Profiles
/// only shape compact-v3; earlier versions keep their defaults.
pub fn automatic_profile<'a>(
    bytes: &'a [u8],
    path: Option<PathBuf>,
    profile: Option<crate::commands::Profile>,
    budget: Option<usize>,
    version: Version,
) -> Result<Cow<'a, [u8]>> {
    automatic_decision(bytes, path, profile, budget, version).map(|(output, _)| output)
}

/// `automatic_decision` that journals its decision in the local event
/// journal (`events`). Any failure keeps the original bytes, journaled as
/// `error`, so a hook or wrapper can always return what the command printed.
pub fn automatic_recorded<'a>(
    host: &str,
    bytes: &'a [u8],
    path: Option<PathBuf>,
    profile: Option<crate::commands::Profile>,
    budget: Option<usize>,
    version: Version,
) -> Cow<'a, [u8]> {
    automatic_anchored(host, bytes, path, profile, budget, version, &[])
}

/// `automatic_recorded` that ranks up lines containing one of `anchors`
/// (compact-v4 only; earlier versions ignore them).
pub fn automatic_anchored<'a>(
    host: &str,
    bytes: &'a [u8],
    path: Option<PathBuf>,
    profile: Option<crate::commands::Profile>,
    budget: Option<usize>,
    version: Version,
    anchors: &[String],
) -> Cow<'a, [u8]> {
    automatic_journaled(host, None, bytes, path, profile, budget, version, anchors)
}

/// `automatic_anchored` that journals the program that printed `bytes`.
#[allow(clippy::too_many_arguments)]
pub fn automatic_journaled<'a>(
    host: &str,
    program: Option<&str>,
    bytes: &'a [u8],
    path: Option<PathBuf>,
    profile: Option<crate::commands::Profile>,
    budget: Option<usize>,
    version: Version,
    anchors: &[String],
) -> Cow<'a, [u8]> {
    let shape = Shape::of(profile, budget, version);
    let anchors = if version >= Version::V4 { anchors } else { &[] };
    let (output, decision) =
        automatic_with(bytes, shape, version, anchors, || Store::open(path.clone()))
            .unwrap_or_else(|_| (Cow::Borrowed(bytes), Decision::unchanged(Reason::Error)));
    if !bytes.is_empty() {
        crate::events::record(
            path,
            crate::events::Event {
                host: host.into(),
                profile: effective_profile(profile, version).map(|p| p.name().into()),
                program: program.map(str::to_owned),
                bytes_in: bytes.len(),
                bytes_out: output.len(),
                reason: Some(decision.reason),
                version: Some(version.number()),
                artifact: decision.artifact.as_deref().map(crate::events::prefix),
                blob: decision.blob.as_deref().map(crate::events::prefix),
                anchored: decision.anchored,
                ..Default::default()
            },
        );
    }
    output
}

/// `automatic_profile`, with the decision it made.
pub fn automatic_decision<'a>(
    bytes: &'a [u8],
    path: Option<PathBuf>,
    profile: Option<crate::commands::Profile>,
    budget: Option<usize>,
    version: Version,
) -> Result<(Cow<'a, [u8]>, Decision)> {
    let shape = Shape::of(profile, budget, version);
    automatic_with(bytes, shape, version, &[], || Store::open(path))
}

fn automatic_with<'a>(
    bytes: &'a [u8],
    shape: Shape,
    version: Version,
    anchors: &[String],
    store: impl FnOnce() -> Result<Store>,
) -> Result<(Cow<'a, [u8]>, Decision)> {
    let unchanged = |reason| Ok((Cow::Borrowed(bytes), Decision::unchanged(reason)));
    ensure!(
        (1024..=1024 * 1024).contains(&shape.budget),
        "max_bytes must be 1024..1048576"
    );
    let max_lines = if version.profiled() {
        V3_MAX_LINES
    } else {
        MAX_LINES
    };
    if bytes.len() <= shape.small {
        return unchanged(Reason::Small);
    }
    if memchr::memchr_iter(b'\n', bytes).count() > max_lines {
        return unchanged(Reason::TooManyLines);
    }
    // Existing Scopelet output and host previews are never compressed again.
    // Compact-v3 only recognizes their markers where they are written, at the
    // start of a line: a source file quoting `"[scopelet "` is not a view.
    let marked = |finder: &memchr::memmem::Finder<'_>| {
        if version.profiled() {
            finder
                .find_iter(bytes)
                .any(|i| i == 0 || bytes[i - 1] == b'\n')
        } else {
            finder.find(bytes).is_some()
        }
    };
    if marked(&SCOPELET_MARKER) {
        return unchanged(Reason::AlreadyCompressed);
    }
    if marked(&PERSISTED_MARKER) {
        return unchanged(Reason::Persisted);
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return unchanged(Reason::NonUtf8);
    };
    let parsed = Parsed::detected(text.to_owned());
    let format = parsed.format();
    let mut data = Dataset {
        records: parsed.records("input", String::new()),
        notes: vec![SUPPLIED.into()],
        ..Dataset::default()
    };
    // A compact-v4 test view names its failures, then its totals, right
    // below the header. The lines take at most a quarter of the budget; a
    // longer one is left out rather than starving the view of the output it
    // describes.
    let mut action = String::new();
    if version >= Version::V4 && shape.profile == Some(crate::commands::Profile::Tests) {
        for line in [failure_line(text), totals_line(text)]
            .into_iter()
            .flatten()
        {
            if (action.len() + line.len()) * 4 <= shape.budget {
                action.push_str(&line);
            }
        }
    }
    let failed = (!action.is_empty()).then_some(action);
    let failed_len = failed.as_ref().map_or(0, String::len);
    let mut anchoring = Anchoring::new(anchors);
    anchoring.focus(text);
    let (mut result, shown) = view(
        &data,
        PLACEHOLDER,
        shape.budget - failed_len,
        version,
        Some(text),
        max_lines,
        Kind::of(shape.profile, version),
        &mut anchoring,
    )?;
    if let Some(line) = &failed {
        let end = result.find('\n').unwrap() + 1;
        result.insert_str(end, line);
    }
    if shown == 0 || result.len() + 512 > bytes.len() || result.len() * 5 > bytes.len() * 4 {
        return unchanged(Reason::Savings);
    }
    let store = store()?;
    // Compact-v3 artifacts name the original blob instead of storing every
    // record a second time; earlier versions keep their exact v1 artifacts.
    let artifact = if version.profiled() {
        let (artifact, blob) = store_original(&store, bytes, format)?;
        data.snapshots.push(snapshot(blob, bytes.len()));
        artifact
    } else {
        let blob = store.put("blob", bytes)?;
        for record in &mut data.records {
            record.blob = Some(blob.clone());
        }
        data.snapshots.push(snapshot(blob, bytes.len()));
        store.put_json(&data)?
    };
    store.set_last(&artifact);
    identify(&mut result, &data, &artifact, version);
    ensure!(
        result.len() <= shape.budget,
        "compact metadata exceeds budget"
    );
    let decision = Decision {
        reason: Reason::Compressed,
        blob: Some(data.snapshots[0].blob.clone()),
        artifact: Some(artifact),
        anchored: anchoring.promoted > 0,
    };
    Ok((Cow::Owned(result.into_bytes()), decision))
}

const SUPPLIED: &str = "Bytes supplied to the compressor; completeness before capture is unknown.";

fn snapshot(blob: String, bytes: usize) -> Snapshot {
    Snapshot {
        source: "input".into(),
        blob,
        bytes,
        local_path: None,
    }
}

/// Store supplied bytes and the schema-2 artifact that names them as the
/// records of `format`; returns (artifact, blob). The same bytes always
/// yield the same artifact, whether a view or a repeat reference stored it.
fn store_original(store: &Store, bytes: &[u8], format: Format) -> Result<(String, String)> {
    let blob = store.put("blob", bytes)?;
    let data = Dataset {
        notes: vec![SUPPLIED.into()],
        snapshots: vec![snapshot(blob.clone(), bytes.len())],
        ..Dataset::default()
    };
    let artifact = store.put_json(&DatasetRef::new(
        &data,
        RecordsRef {
            blob: blob.clone(),
            format,
            source: "input".into(),
        },
    ))?;
    Ok((artifact, blob))
}

/// The compact-v4 reference that replaces an output already shown in the
/// same context.
fn repeat_marker(id: &str, lines: usize, version: Version) -> String {
    let recover = if Grammar::of(version).header {
        ""
    } else {
        "; expand ID"
    };
    format!(
        "{}{id} repeat: identical to output already shown in this conversation ({lines} lines){recover}]\n",
        prefix(version)
    )
}

/// Replace output identical to one already shown in this context by a
/// reference to it, journaled as `repeat`. The original is stored as a view's
/// would be, so `expand` recovers it even when the first occurrence passed
/// through unchanged. None when it cannot be stored or is not UTF-8: the
/// caller then treats the output as new.
pub fn repeat_recorded(
    host: &str,
    program: Option<&str>,
    bytes: &[u8],
    path: Option<PathBuf>,
    profile: Option<crate::commands::Profile>,
    version: Version,
) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let format = Parsed::detected(text.to_owned()).format();
    let store = Store::open(path.clone()).ok()?;
    let (artifact, blob) = store_original(&store, bytes, format).ok()?;
    store.set_last(&artifact);
    let marker = repeat_marker(
        short_id(&artifact, version),
        clean::line_count(text),
        version,
    )
    .into_bytes();
    crate::events::record(
        path,
        crate::events::Event {
            host: host.into(),
            profile: effective_profile(profile, version).map(|p| p.name().into()),
            program: program.map(str::to_owned),
            bytes_in: bytes.len(),
            bytes_out: marker.len(),
            reason: Some(Reason::Repeat),
            version: Some(version.number()),
            artifact: Some(crate::events::prefix(&artifact)),
            blob: Some(crate::events::prefix(&blob)),
            ..Default::default()
        },
    );
    Some(marker)
}

pub fn compact(data: &Dataset, store: &Store, budget: usize) -> Result<String> {
    compact_version(data, store, budget, Version::V1)
}
pub fn compact_version(
    data: &Dataset,
    store: &Store,
    budget: usize,
    version: Version,
) -> Result<String> {
    let (mut text, _) = view(
        data,
        PLACEHOLDER,
        budget,
        version,
        None,
        MAX_LINES,
        Kind::default(),
        &mut Anchoring::new(&[]),
    )?;
    let id = store.put_json(data)?;
    identify(&mut text, data, &id, version);
    Ok(text)
}

/// Write the stored artifact's reference into a view's header. Only our
/// header is substituted, never placeholder-looking text in original evidence.
fn identify(text: &mut String, data: &Dataset, artifact: &str, version: Version) {
    if version >= Version::V4 {
        let at = V4_PREFIX.len();
        let id = short_id(artifact, version);
        text.replace_range(at..at + id.len(), id);
    } else {
        let end = text.find('\n').unwrap() + 1;
        text.replace_range(..end, &header(data, artifact, version));
    }
}

fn header(data: &Dataset, artifact: &str, version: Version) -> String {
    match version {
        Version::V1 => format!(
            "[scopelet compact-v1 {artifact} scan_complete={}; original: scopelet expand {artifact} --manifest]\n",
            data.scan_complete
        ),
        Version::V2 => format!(
            "[scopelet compact-v2 {artifact} scan_complete={}; recover: scopelet expand {artifact} --find TEXT (or --manifest)]\n",
            data.scan_complete
        ),
        // ID refers to the artifact reference above: the id is not repeated.
        Version::V3 => format!(
            "[scopelet compact-v3 {artifact} scan_complete={}; recover: scopelet expand ID --find TEXT (or --manifest)]\n",
            data.scan_complete
        ),
        Version::V4 | Version::V5 => header_v4(
            short_id(artifact, version),
            version,
            0,
            0,
            data.scan_complete,
        ),
    }
}

/// Hex characters of the artifact hash a compact-v4 header shows. `expand`
/// resolves it like any unique prefix of 8 or more characters.
const SHORT_ID: usize = 16;
/// The same in a compact-v5 header: still unique in any local cache, and
/// `expand` fails loudly rather than guess when it is not.
const SHORT_ID_V5: usize = 12;
const V4_PREFIX: &str = "[scopelet v4 ";
const PLACEHOLDER_ID: &str = "0000000000000000";

/// The prefix of a view's header: every compact-v4 and v5 header starts
/// with `[scopelet vN ` and its ID.
fn prefix(version: Version) -> &'static str {
    if version >= Version::V5 {
        "[scopelet v5 "
    } else {
        V4_PREFIX
    }
}

fn id_len(version: Version) -> usize {
    if Grammar::of(version).header {
        SHORT_ID_V5
    } else {
        SHORT_ID
    }
}

/// The first hex characters of a reference a header shows, or the
/// placeholder.
fn short_id(reference: &str, version: Version) -> &str {
    let len = id_len(version);
    let hash = reference.split_once(':').map_or(reference, |(_, h)| h);
    hash.get(..len).unwrap_or(&PLACEHOLDER_ID[..len])
}

/// The single line that frames a compact-v4 view: its short artifact ID,
/// whether the display is complete or how many units it omits, and
/// `scan_complete=false` only when the source scan was incomplete. There is
/// no footer.
/// `abridged` counts units shown only in part (a line cut behind
/// `text_truncated`, a record shown without some of its fields): a view with
/// any is not complete either.
/// Compact-v5 writes the same facts with a shorter ID, and without the
/// recovery syntax the session guidance already gives.
fn header_v4(
    id: &str,
    version: Version,
    omitted: usize,
    abridged: usize,
    scan_complete: bool,
) -> String {
    let start = prefix(version);
    let recover = if Grammar::of(version).header {
        ""
    } else {
        "; expand ID --find TEXT"
    };
    let scan = if scan_complete {
        ""
    } else {
        " scan_complete=false"
    };
    let abridged = if abridged > 0 {
        format!(" abridged={abridged}")
    } else {
        String::new()
    };
    if omitted == 0 && abridged.is_empty() {
        format!("{start}{id}{scan} complete]\n")
    } else {
        format!("{start}{id}{scan} omitted={omitted}{abridged}; gaps are not absence{recover}]\n")
    }
}
fn footer(omitted: usize) -> String {
    format!(
        "[scopelet display_complete={} omitted_units={omitted}; gaps are omitted, not evidence of absence]\n",
        omitted == 0
    )
}

struct Unit<'a> {
    record: &'a Record,
    /// Displayed text of a text unit; JSON units serialize their value.
    line: Option<Cow<'a, str>>,
    /// Label and metadata before the text.
    label: Label,
    /// Original length of a v3 line cut behind `text_truncated`; 0 when whole.
    truncated: usize,
    size: usize,
    priority: u8,
    /// Order inside its tier when nonzero: higher first (a compact-v4 diff's
    /// files by change density). Zero keeps head and tail alternation.
    weight: u16,
    /// Ranked up by a relevance anchor: relevant, not a diagnostic.
    anchored: bool,
}

/// The label in front of a unit. Compact-v3 labels are written when the unit
/// is rendered, so building units for a large input allocates no strings.
enum Label {
    /// Earlier versions: the complete prefix, including the separating space.
    Prefix(String),
    /// `source:a`
    Line(usize),
    /// `source:a-b repeat=n`: n contiguous identical lines.
    Run { a: usize, b: usize, n: usize },
    /// `source:a repeat=n last=b`: the same text at n dispersed lines.
    Repeat { a: usize, n: usize, b: usize },
    /// `source:a similar=n last=b`: n lines sharing a template.
    Similar { a: usize, n: usize, b: usize },
}

fn digits(n: usize) -> usize {
    if n == 0 { 1 } else { n.ilog10() as usize + 1 }
}

/// " text_truncated bytes=" before the original line length.
const TRUNCATED_MARK: &str = " text_truncated bytes=";

impl<'a> Unit<'a> {
    fn text(record: &'a Record, prefix: String, line: Cow<'a, str>, priority: u8) -> Self {
        let size = prefix.len() + line.len() + usize::from(!line.ends_with('\n'));
        Self {
            record,
            line: Some(line),
            label: Label::Prefix(prefix),
            truncated: 0,
            size,
            priority,
            weight: 0,
            anchored: false,
        }
    }
    /// Compact-v3 text unit; a line too large for the budget is cut on a
    /// character boundary behind a `text_truncated bytes=N` marker. `raw`
    /// yields the source line and is only called for a line that is cut.
    fn text_v3(
        record: &'a Record,
        label: Label,
        view: Cow<'a, str>,
        raw: impl FnOnce() -> &'a str,
        priority: u8,
        limit: usize,
    ) -> Self {
        let label_len = label.len(&record.source);
        let mut unit = Self {
            record,
            size: label_len + 1 + view.len() + 1,
            line: None,
            label,
            truncated: 0,
            priority,
            weight: 0,
            anchored: false,
        };
        let cap = limit.min(TRUNCATED_PREFIX);
        let bytes = if unit.size > limit {
            clean::body(raw()).len()
        } else {
            0
        };
        let prefix = label_len + TRUNCATED_MARK.len() + digits(bytes) + 1;
        if unit.size <= limit || prefix + 1 >= cap {
            unit.line = Some(view);
            return unit;
        }
        let cut = (0..=cap - prefix - 1)
            .rev()
            .find(|&i| view.is_char_boundary(i))
            .unwrap();
        unit.line = Some(match view {
            Cow::Borrowed(view) => Cow::Borrowed(&view[..cut]),
            Cow::Owned(mut view) => {
                view.truncate(cut);
                Cow::Owned(view)
            }
        });
        unit.truncated = bytes;
        unit.size = prefix + cut + 1;
        unit
    }
    /// Source line of a plain single-line unit that may join a range block.
    fn block(&self) -> Option<usize> {
        match self.label {
            Label::Line(a) if self.truncated == 0 => Some(a),
            _ => None,
        }
    }
    /// Render the unit; `bare` (compact-v5) drops its `source:`. Its size
    /// stays what the labelled unit takes, which selection counts.
    fn append(&self, output: &mut String, bare: bool) {
        if let Some(line) = &self.line {
            let start = output.len();
            self.label.write(&self.record.source, bare, output);
            if self.truncated > 0 {
                output.push_str(TRUNCATED_MARK);
                output.push_str(&self.truncated.to_string());
            }
            if !matches!(self.label, Label::Prefix(_)) {
                output.push(' ');
            }
            output.push_str(line);
            if !line.ends_with('\n') {
                output.push('\n');
            }
            let dropped = if bare {
                self.record.source.len() + 1
            } else {
                0
            };
            debug_assert_eq!(
                output.len() - start + dropped,
                self.size,
                "unit size drifted"
            );
        } else {
            output.push_str(&self.record.source);
            output.push_str(": ");
            output.push_str(&self.record.value.as_ref().unwrap().to_string());
            output.push('\n');
        }
    }
}

impl Label {
    /// Byte length of the label, without the space that follows it.
    fn len(&self, source: &str) -> usize {
        source.len()
            + 1
            + match *self {
                Self::Prefix(ref prefix) => return prefix.len(),
                Self::Line(a) => digits(a),
                Self::Run { a, b, n } => digits(a) + 1 + digits(b) + " repeat=".len() + digits(n),
                Self::Repeat { a, n, b } => {
                    digits(a) + " repeat=".len() + digits(n) + " last=".len() + digits(b)
                }
                Self::Similar { a, n, b } => {
                    digits(a) + " similar=".len() + digits(n) + " last=".len() + digits(b)
                }
            }
    }
    /// `bare` writes the label without its `source:` (a compact-v5 view of
    /// one input); a prefix label then drops the same leading `source:`.
    fn write(&self, source: &str, bare: bool, output: &mut String) {
        use std::fmt::Write;
        if let Self::Prefix(prefix) = self {
            let label = if bare {
                prefix
                    .strip_prefix(source)
                    .and_then(|rest| rest.strip_prefix(':'))
                    .unwrap_or(prefix)
            } else {
                prefix
            };
            output.push_str(label);
            return;
        }
        if !bare {
            output.push_str(source);
            output.push(':');
        }
        // Writing into a String cannot fail.
        let _ = match *self {
            Self::Prefix(_) => unreachable!(),
            Self::Line(a) => write!(output, "{a}"),
            Self::Run { a, b, n } => write!(output, "{a}-{b} repeat={n}"),
            Self::Repeat { a, n, b } => write!(output, "{a} repeat={n} last={b}"),
            Self::Similar { a, n, b } => write!(output, "{a} similar={n} last={b}"),
        };
    }
}

fn signal_value<'v>(
    value: &'v serde_json::Value,
    signal: &dyn Fn(&str) -> bool,
) -> Option<&'v str> {
    match value {
        serde_json::Value::String(s) => signal(s).then_some(s),
        serde_json::Value::Array(a) => a.iter().find_map(|v| signal_value(v, signal)),
        serde_json::Value::Object(o) => o.values().find_map(|v| signal_value(v, signal)),
        _ => None,
    }
}

/// Diagnostic templates already shown, so later occurrences rank lower.
#[derive(Default)]
struct Seen {
    strong: HashSet<String>,
    weak: HashSet<String>,
}

/// Whether this line is the first of its template, recording it if so. The
/// key is copied into the set only when it is new.
fn first_of_template(seen: &mut HashSet<String>, line: &str, key: &mut String) -> bool {
    clean::template_into(line, key);
    if seen.contains(key.as_str()) {
        return false;
    }
    seen.insert(key.clone());
    true
}

fn units<'a>(
    data: &'a Dataset,
    limit: usize,
    version: Version,
    kind: Kind,
    anchoring: &mut Anchoring<'_>,
) -> Vec<Unit<'a>> {
    let mut units = Vec::new();
    let mut distinct = BTreeSet::new();
    let mut seen = Seen::default();
    let v2_signal = |s: &str| SIGNAL.is_match(s);
    let v3_signal = |s: &str| diagnostic(&STRONG, s);
    let signal: &dyn Fn(&str) -> bool = if version.profiled() {
        &v3_signal
    } else {
        &v2_signal
    };
    for (ordinal, record) in data.records.iter().enumerate() {
        if let Some(value) = &record.value {
            let priority = if version == Version::V1 {
                if SIGNAL.is_match(&value.to_string()) {
                    3
                } else {
                    0
                }
            } else if ordinal == 0 || ordinal + 1 == data.records.len() {
                4
            } else if let Some(signal) = signal_value(value, signal) {
                if distinct.insert(signal) { 3 } else { 2 }
            } else {
                0
            };
            let mut unit = Unit {
                record,
                line: None,
                label: Label::Prefix(String::new()),
                truncated: 0,
                size: encoding::size(value, limit).unwrap_or(limit + 1) + record.source.len() + 3,
                priority,
                weight: 0,
                anchored: false,
            };
            anchoring.promote(&mut unit);
            units.push(unit);
            continue;
        }
        if version.profiled() {
            text_units_v3(record, limit, kind, &mut seen, anchoring, &mut units);
            continue;
        }
        let lines: Vec<_> = record.text.split_inclusive('\n').collect();
        let mut nearby = vec![false; lines.len()];
        let signals: Vec<_> = lines.iter().map(|line| SIGNAL.is_match(line)).collect();
        for (i, &signal) in signals.iter().enumerate() {
            if signal {
                nearby[i.saturating_sub(3)..(i + 9).min(lines.len())].fill(true);
            }
        }
        let front = lines.len().min(3);
        nearby[..front].fill(true);
        nearby[lines.len().saturating_sub(5)..].fill(true);
        let base = record.start_line.unwrap_or(1);
        let mut i = 0;
        while i < lines.len() {
            let mut end = i + 1;
            while end < lines.len() && lines[end] == lines[i] {
                end += 1;
            }
            let line = lines[i];
            let priority = if i == 0 || end == lines.len() {
                4
            } else if signals[i] {
                if version == Version::V1 || distinct.insert(line) {
                    3
                } else {
                    2
                }
            } else if nearby[i..end].iter().any(|&v| v) {
                1
            } else {
                0
            };
            let prefix = if end == i + 1 {
                format!("{}:{} ", record.source, base + i)
            } else {
                format!(
                    "{}:{}-{} repeat={} ",
                    record.source,
                    base + i,
                    base + end - 1,
                    end - i
                )
            };
            units.push(Unit::text(record, prefix, Cow::Borrowed(line), priority));
            i = end;
        }
    }
    units
}

/// Per-line flags, one bit per line: a large input keeps a bit per line and
/// flag instead of a byte.
struct Bits(Vec<u64>);
impl Bits {
    fn new(n: usize) -> Self {
        Self(vec![0; n.div_ceil(64)])
    }
    fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    fn clear(&mut self, i: usize) {
        self.0[i / 64] &= !(1 << (i % 64));
    }
    fn fill(&mut self, range: std::ops::Range<usize>) {
        for i in range {
            self.set(i);
        }
    }
    fn any(&self, range: std::ops::RangeInclusive<usize>) -> bool {
        range.into_iter().any(|i| self.get(i))
    }
}

/// Compact-v3 text units work on display copies of the lines (terminal
/// control sequences and carriage-return overwrites removed) while labels stay
/// absolute source lines. Identical lines fold into their first occurrence
/// wherever they are; lines without a diagnostic fold with the lines sharing
/// their template; oversized lines are cut instead of vetoing the view.
///
/// Line indices are `u32` (a capture is at most 32 MiB) and flags are bits,
/// so the working set stays near the units themselves on a large input.
///
/// `kind.file_read`: a file read ranks no line as a diagnostic; anchors still
/// apply. `kind.search`: a file's matches beyond `SEARCH_MATCHES` are never
/// shown (see `search_capped`). `kind.similar_on_overflow`: a text that fits
/// `limit` with its labels folds identical lines only, since the numbers that
/// tell similar lines apart (counts, ids, statuses) are often the evidence.
fn text_units_v3<'a>(
    record: &'a Record,
    limit: usize,
    kind: Kind,
    seen: &mut Seen,
    anchoring: &mut Anchoring<'_>,
    units: &mut Vec<Unit<'a>>,
) {
    let reading = kind.file_read;
    const NONE: u32 = u32::MAX;
    struct Group {
        first: u32,
        last: u32,
        count: u32,
        /// Every member is the same text (otherwise members share a template).
        exact: bool,
        context: bool,
        nearby: bool,
    }
    let mut views: Vec<Cow<'a, str>> = record
        .text
        .split_inclusive('\n')
        .map(clean::line_view)
        .collect();
    let n = views.len();
    // A one-byte indent per line and a few labels: a text this size is shown
    // whole once identical lines fold, so nothing similar needs to.
    let exact_only = kind.similar_on_overflow && record.text.len() + 2 * n + 64 <= limit;
    // A test run's per-binary results are its outcome: they never fold
    // together by their numbers and rank with first diagnostics, so the
    // results a view shows add up to the run's totals.
    let mut results = Bits::new(n);
    if kind.tests {
        for (i, view) in views.iter().enumerate() {
            if CARGO_RESULT.is_match(view) {
                results.set(i);
            }
        }
    }
    // A search shows each file's first matches before any file's others.
    let (later, first_matches) = if kind.search {
        search_ranks(&views, anchoring)
    } else {
        (Bits::new(n), Bits::new(n))
    };
    // Identical text is classified once: `first[i]` is the earliest line with
    // this exact view, and it carries the vocabulary matches and, later, the
    // group for every repetition. A log of the same line five hundred times
    // runs the diagnostic patterns once, not five hundred times.
    let first: Vec<u32> = {
        let mut by_text: HashMap<&str, u32> = HashMap::default();
        views
            .iter()
            .enumerate()
            .map(|(i, view)| *by_text.entry(view.as_ref()).or_insert(i as u32))
            .collect()
    };
    let (mut strong, mut weak) = (Bits::new(n), Bits::new(n));
    // Trivial lines (`}`, a blank line, a `|` gutter) carry no evidence of
    // their own: they stay where they are, between the lines they separate.
    let mut trivial = Bits::new(n);
    for i in 0..n {
        let origin = first[i] as usize;
        if origin == i {
            if !reading && diagnostic(&STRONG, &views[i]) {
                strong.set(i);
            } else if !reading && diagnostic(&WEAK, &views[i]) {
                weak.set(i);
            } else if is_trivial(&views[i]) {
                trivial.set(i);
            }
        } else {
            if strong.get(origin) {
                strong.set(i);
            }
            if weak.get(origin) {
                weak.set(i);
            }
            if trivial.get(origin) {
                trivial.set(i);
            }
        }
    }
    // In a unified diff, code that mentions `error` is a change, not a
    // diagnostic. Hunk headers and context lines are glued like trivial
    // lines: they stay beside their changes and only fold with contiguous
    // copies, so a hunk is shown with the change it locates.
    let roles = diff_roles(&views);
    let role = |i: usize| roles.as_ref().map_or(Diff::None, |r| r[i]);
    if let Some(roles) = &roles {
        for (i, &role) in roles.iter().enumerate() {
            if role != Diff::None {
                strong.clear(i);
                weak.clear(i);
                trivial.clear(i);
                if matches!(role, Diff::Context | Diff::Hunk) {
                    trivial.set(i);
                }
            }
        }
    }
    // Every line of a lockfile or generated file's diff but its header is
    // hidden behind that header's summary; each stays a unit of its own, so
    // the view counts it as omitted.
    let sections = match (&roles, kind.diff_files) {
        (Some(roles), true) => diff_sections(&views, roles),
        _ => Vec::new(),
    };
    let mut collapsed = Bits::new(n);
    // Density orders the files of a plain `git diff` only: in `git log -p`
    // a commit's message must not queue behind its changes.
    let files_only = roles.as_ref().is_some_and(|roles| {
        sections.len() >= 2 && roles.iter().filter(|&&r| r == Diff::File).count() == sections.len()
    });
    let mut weights = vec![0u16; if files_only { n } else { 0 }];
    for section in &sections {
        if section.generated {
            collapsed.fill(section.header + 1..section.end);
        } else if files_only {
            weights[section.header..section.end].fill(section.density());
        }
    }
    for i in (0..n).filter(|&i| collapsed.get(i)) {
        trivial.clear(i);
    }
    // Context is the window around a diagnostic; head and tail lines only
    // count for priority.
    let mut context = Bits::new(n);
    for i in 0..n {
        if strong.get(i) {
            context.fill(i.saturating_sub(3)..(i + 9).min(n));
        }
    }
    let mut nearby = Bits(context.0.clone());
    nearby.fill(0..n.min(3));
    nearby.fill(n.saturating_sub(5)..n);

    // Diagnostics group by their text without timestamps: their variable
    // parts (counters, ids) are evidence and stay visible. Other lines group
    // by template.
    let mut groups: Vec<Group> = Vec::new();
    let mut owner: Vec<u32> = vec![NONE; n];
    // The group of each first occurrence; repetitions look it up by index.
    let mut slot_of_first: Vec<u32> = vec![NONE; n];
    let mut by_template: HashMap<String, u32> = HashMap::default();
    let mut by_diagnostic: HashMap<String, u32> = HashMap::default();
    // One reusable buffer: a template key is only copied when it is new, so a
    // scan over many same-shaped lines allocates once, not once per line.
    let mut key = String::new();
    fn join(
        groups: &mut Vec<Group>,
        first: &[u32],
        slot: u32,
        i: usize,
        context: bool,
        nearby: bool,
    ) {
        if slot as usize == groups.len() {
            groups.push(Group {
                first: i as u32,
                last: i as u32,
                count: 1,
                exact: true,
                context,
                nearby,
            });
        } else {
            let group = &mut groups[slot as usize];
            group.last = i as u32;
            group.count += 1;
            group.exact &= first[group.first as usize] == first[i];
            group.context |= context;
            group.nearby |= nearby;
        }
    }
    // A few repetitions inside a diagnostic's context stay in source order;
    // massive repetition folds wherever it is.
    let folds = |g: &Group| g.count > 1 && (!g.context || g.count as usize >= FOLD_IN_CONTEXT);
    for (i, view) in views.iter().enumerate() {
        if trivial.get(i) {
            continue;
        }
        // Identical text always shares a group, and identical text has an
        // identical template, so a repetition skips building one. On a log of
        // repeated lines that is every line after the first.
        let origin = first[i] as usize;
        let slot = if collapsed.get(i) {
            // Shown nowhere: never the representative of other lines.
            groups.len() as u32
        } else if slot_of_first[origin] != NONE {
            slot_of_first[origin]
        } else if exact_only
            || results.get(i)
            || matches!(role(i), Diff::File | Diff::Change | Diff::Message)
        {
            // Headers, changes and commit messages are evidence: they never
            // fold by template, only with the very same text (a file every
            // commit touches). Nor does any line of a text that fits.
            slot_of_first[i] = groups.len() as u32;
            groups.len() as u32
        } else if strong.get(i) || weak.get(i) {
            // Repetitions of one diagnostic differ by their timestamps only.
            clean::diagnostic_key_into(view, &mut key);
            let slot = match by_diagnostic.get(key.as_str()) {
                Some(&slot) => slot,
                None => {
                    by_diagnostic.insert(key.clone(), groups.len() as u32);
                    groups.len() as u32
                }
            };
            slot_of_first[i] = slot;
            slot
        } else {
            clean::template_into(view, &mut key);
            let slot = match by_template.get(key.as_str()) {
                Some(&slot) => slot,
                None => {
                    by_template.insert(key.clone(), groups.len() as u32);
                    groups.len() as u32
                }
            };
            slot_of_first[i] = slot;
            slot
        };
        join(&mut groups, &first, slot, i, context.get(i), nearby.get(i));
        owner[i] = slot;
    }
    drop(by_template);
    drop(by_diagnostic);
    // A nontrivial line is shown in place unless it folds into an earlier
    // occurrence. A trivial line next to one (ignoring other trivial lines)
    // is glued: it stays where it is. One between lines shown elsewhere has
    // nothing to hold together and folds by exact text, as any other line.
    let in_place = |i: usize| {
        let group = &groups[owner[i] as usize];
        !folds(group) || group.first as usize == i
    };
    // In a diff, a file header ends what can glue and its metadata lines are
    // transparent, so a hunk glues to its own changes only.
    let mut glued = Bits::new(n);
    for order in [true, false] {
        let lines: Box<dyn Iterator<Item = usize>> = if order {
            Box::new(0..n)
        } else {
            Box::new((0..n).rev())
        };
        // Lines since the last nontrivial line shown in place, if any.
        let mut reach: Option<usize> = None;
        for i in lines {
            match role(i) {
                Diff::File => reach = None,
                Diff::Meta => {}
                _ if !trivial.get(i) => reach = in_place(i).then_some(0),
                _ => {
                    reach = reach.map(|r| r + 1);
                    // A hunk header locates the first change below it, past
                    // however much context Git shows before that change.
                    let locates = !order && role(i) == Diff::Hunk;
                    if reach.is_some_and(|r| locates || r <= GLUE_REACH) {
                        glued.set(i);
                    }
                }
            }
        }
    }
    for i in 0..n {
        if !trivial.get(i) {
            continue;
        }
        let origin = first[i] as usize;
        let slot = if glued.get(i) {
            // Only a contiguous run of the same glued text folds.
            if i > 0 && glued.get(i - 1) && first[i - 1] == first[i] {
                owner[i - 1]
            } else {
                groups.len() as u32
            }
        } else if slot_of_first[origin] != NONE {
            slot_of_first[origin]
        } else {
            slot_of_first[origin] = groups.len() as u32;
            groups.len() as u32
        };
        join(&mut groups, &first, slot, i, context.get(i), nearby.get(i));
        owner[i] = slot;
    }
    drop(slot_of_first);
    let folds: Vec<bool> = groups.iter().map(folds).collect();
    let folded: usize = groups
        .iter()
        .zip(&folds)
        .filter(|(_, folds)| **folds)
        .map(|(g, _)| g.count as usize)
        .sum();
    let mostly_folded = n >= FOLD_MAJORITY_MIN_LINES && folded * 10 >= n * FOLD_MAJORITY_TENTHS;
    let base = record.start_line.unwrap_or(1);
    // Source lines are only needed to measure a line that is cut, and units
    // are emitted in increasing line order, so one forward cursor serves all.
    let mut raw_lines = record.text.split_inclusive('\n');
    let mut raw_at = 0;
    // The priority of the unit showing each nontrivial line in place, so a
    // glued unit can take its neighbours' once every unit is known. A line
    // folded into an earlier occurrence is `AWAY`: nothing glues to it.
    const TRIVIAL: u8 = u8::MAX;
    const AWAY: u8 = u8::MAX - 1;
    let mut line_priority: Vec<u8> = (0..n)
        .map(|i| if trivial.get(i) { TRIVIAL } else { AWAY })
        .collect();
    // (unit index, first line, last line) of each glued unit.
    let mut trivial_units: Vec<(usize, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        let slot = owner[i] as usize;
        let group = &groups[slot];
        let (first_line, last, count, exact, near) = if folds[slot] {
            if group.first as usize != i {
                i += 1;
                continue;
            }
            (
                i,
                group.last as usize,
                group.count as usize,
                group.exact,
                group.nearby,
            )
        } else {
            // Unfolded members still merge with contiguous identical lines.
            let mut j = i;
            while j + 1 < n && owner[j + 1] as usize == slot && first[j + 1] == first[i] {
                j += 1;
            }
            (i, j, j + 1 - i, true, nearby.any(i..=j))
        };
        let priority = if first_line == 0 || last + 1 == n {
            4
        } else if results.get(i) {
            3
        } else if role(i) == Diff::File {
            2
        } else if matches!(role(i), Diff::Change | Diff::Message) {
            // A distinct change or commit message is the evidence; one
            // repeated across files or commits is mechanical and ranks below
            // the headers that list them.
            if count == 1 { 2 } else { 1 }
        } else if role(i) == Diff::Meta {
            0
        } else if glued.get(i) {
            trivial_units.push((units.len(), first_line, last));
            TRIVIAL
        } else if strong.get(i) {
            // The first diagnostic of each template, then its variants.
            if first_of_template(&mut seen.strong, &views[i], &mut key) {
                3
            } else {
                2
            }
        } else if weak.get(i) {
            if first_of_template(&mut seen.weak, &views[i], &mut key) {
                2
            } else {
                1
            }
        } else if group.count == 1 && mostly_folded && !trivial.get(i) {
            // The rare line among folded noise is what the reader is after.
            2
        } else if context.get(first_line) || FRAME.is_match(&views[i]) {
            // A failure's own detail outranks the head and tail padding, which
            // would otherwise fill the budget and leave only its first line.
            2
        } else if near {
            1
        } else {
            0
        };
        let (a, b) = (base + first_line, base + last);
        let label = if count == 1 {
            Label::Line(a)
        } else if exact && last + 1 - first_line == count {
            Label::Run { a, b, n: count }
        } else if exact {
            Label::Repeat { a, n: count, b }
        } else {
            Label::Similar { a, n: count, b }
        };
        // Each line's view is displayed at most once: move it into its unit.
        let view = std::mem::take(&mut views[i]);
        let raw = || {
            let line = raw_lines.nth(i - raw_at).unwrap();
            raw_at = i + 1;
            line
        };
        let mut unit = Unit::text_v3(record, label, view, raw, priority, limit);
        if let Some(section) = sections.iter().find(|s| s.generated && s.header == i) {
            unit = Unit::text(
                record,
                format!(
                    "{}:{}-{} file={} changed=+{}/-{} omitted",
                    record.source,
                    base + i,
                    base + section.end - 1,
                    section.path,
                    section.added,
                    section.removed
                ),
                Cow::Borrowed(""),
                priority,
            );
        } else if kind.install && INSTALL_SUMMARY.is_match(unit.line.as_deref().unwrap_or("")) {
            // What was installed is the outcome, whatever its position.
            unit.priority = unit.priority.max(3);
        } else if collapsed.get(i) || (kind.install && unit.priority == 0) {
            unit.priority = CAPPED;
        } else if priority != TRIVIAL {
            if count == 1 && first_matches.get(i) && !later.get(i) {
                unit.priority = unit.priority.max(1);
            }
            anchoring.promote(&mut unit);
        }
        if let Some(&weight) = weights.get(i) {
            unit.weight = weight;
        }
        let priority = unit.priority;
        units.push(unit);
        if collapsed.get(i) {
            // Nothing glues to a hidden line.
        } else if role(i) == Diff::File {
            // A file header ends the previous hunk: nothing glues across it.
        } else if role(i) == Diff::Meta {
            line_priority[i] = TRIVIAL;
        } else if !trivial.get(i) {
            let shown = if folds[slot] { i..=i } else { i..=last };
            line_priority[shown].fill(priority);
        }
        i = if folds[slot] { i + 1 } else { last + 1 };
    }
    // A glued unit ranks with the lower of its neighbours shown in place: it
    // is shown when both lines it separates are, so retained code keeps its
    // braces and blank lines and reads as one block.
    // Each carry is the priority and line of the nearest such neighbour; one
    // further than GLUE_REACH lines away does not count.
    let carried = |carry: &mut Option<(u8, usize)>, priority: u8, line: usize| match priority {
        TRIVIAL => {}
        AWAY => *carry = None,
        priority => *carry = Some((priority, line)),
    };
    let mut before = vec![None; trivial_units.len()];
    let (mut k, mut carry) = (0, None);
    for (line, &priority) in line_priority.iter().enumerate() {
        while k < trivial_units.len() && trivial_units[k].1 == line {
            before[k] = carry
                .filter(|&(_, at)| line - at <= GLUE_REACH)
                .map(|(p, _)| p);
            k += 1;
        }
        carried(&mut carry, priority, line);
    }
    let (mut k, mut carry) = (trivial_units.len(), None);
    for (line, &priority) in line_priority.iter().enumerate().rev() {
        while k > 0 && trivial_units[k - 1].2 == line {
            k -= 1;
            let (unit, first_line, _) = trivial_units[k];
            // A hunk header ranks with the change it locates, at any distance.
            if role(first_line) == Diff::Hunk
                && let Some((priority, _)) = carry
            {
                units[unit].priority = priority;
                continue;
            }
            let after = carry
                .filter(|&(_, at)| at - line <= GLUE_REACH)
                .map(|(p, _)| p);
            units[unit].priority = match (before[k], after) {
                (Some(a), Some(b)) => a.min(b),
                (Some(p), None) | (None, Some(p)) => p,
                (None, None) => 0,
            };
        }
        carried(&mut carry, priority, line);
    }
}

/// How an output is read beyond its version: as a file (no diagnostic
/// ranking) or, in compact-v4, as search results grouped by file.
#[derive(Clone, Copy, Default)]
struct Kind {
    file_read: bool,
    search: bool,
    /// Compact-v4 diffs: lockfiles and generated files collapse to one
    /// counted line and real files rank by change density.
    diff_files: bool,
    /// A package install: only diagnostics, their context and the first and
    /// last lines are shown, never ordinary progress.
    install: bool,
    /// Compact-v4: lines that differ only by their numbers fold together only
    /// when the text does not fit; one that fits folds identical lines only.
    similar_on_overflow: bool,
    /// Compact-v4 test runs: cargo's per-binary `test result:` lines are
    /// shown before ordinary lines and never fold by template.
    tests: bool,
    /// Compact-v5: a view whose every record is the one `input` text writes
    /// its labels without `input:`.
    bare: bool,
}

impl Kind {
    fn of(profile: Option<crate::commands::Profile>, version: Version) -> Self {
        use crate::commands::Profile;
        Self {
            file_read: profile == Some(Profile::FileRead),
            search: version >= Version::V4 && profile == Some(Profile::Search),
            diff_files: version >= Version::V4,
            install: version >= Version::V4 && profile == Some(Profile::Install),
            similar_on_overflow: version >= Version::V4,
            tests: version >= Version::V4 && profile == Some(Profile::Tests),
            bare: Grammar::of(version).labels,
        }
    }
}

/// The closing summary of a package install: what it installed or found
/// up to date (npm, pnpm, yarn, bun, pip, uv, bundler, Homebrew).
static INSTALL_SUMMARY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i-u:^\s*(added|removed|changed|up to date|audited) [0-9]+ packages?|^\s*Successfully installed |^\s*(Installed|Uninstalled|Resolved|Prepared|Audited) [0-9]+ packages? in |^\s*Bundle complete|^\s*Done in [0-9.]+m?s|^\s*Packages: [+-][0-9]+|^\s*Already up[- ]to[- ]date|^==> Summary|^\s*Requirement already satisfied)").unwrap()
});

/// Entries above which a listed directory is folded to a summary.
const LISTING_DIRECTORY: usize = 8;
/// Extensions a folded directory's summary names; the rest are counted.
const LISTING_EXTENSIONS: usize = 4;

/// One line of a path listing: its directory and name, or a line that is
/// not an entry (a `find:` error, `ls -R`'s blank lines, `tree`'s total).
struct ListingLine {
    directory: Option<String>,
    name: String,
}

/// Parse `find`/`git ls-files` (a path per line), `ls -R` (`dir:` headers
/// then names) or `tree` (`├── name` with four columns per level). None when
/// the text is none of them, or a search result (`path:N:`).
fn parse_listing(views: &[Cow<'_, str>]) -> Option<Vec<ListingLine>> {
    let nonblank: Vec<&str> = views
        .iter()
        .map(|v| v.as_ref())
        .filter(|v| !v.trim().is_empty())
        .collect();
    if nonblank.len() < 2 || nonblank.iter().any(|v| search_line(v).is_some()) {
        return None;
    }
    let tree = nonblank.iter().filter(|v| v.contains("── ")).count() * 2 >= nonblank.len();
    let ls =
        nonblank[0].ends_with(':') && nonblank.iter().filter(|v| v.ends_with(':')).count() >= 2;
    let paths = nonblank
        .iter()
        .all(|v| !v.starts_with(char::is_whitespace) && !v.contains('\t'))
        && nonblank.iter().filter(|v| v.contains('/')).count() * 2 >= nonblank.len();
    let other = |name: &str| ListingLine {
        directory: None,
        name: name.to_owned(),
    };
    let entry = |directory: &str, name: &str| ListingLine {
        directory: Some(directory.to_owned()),
        name: name.to_owned(),
    };
    if tree {
        // The directory of each depth, from `tree`'s indentation.
        let mut stack: Vec<String> = vec![String::new()];
        return Some(
            views
                .iter()
                .map(|view| {
                    let Some(at) = view.find("── ") else {
                        return other(view);
                    };
                    let depth = view[..at].chars().count() / 4;
                    let name = &view[at + "── ".len()..];
                    stack.truncate(depth + 1);
                    let directory = stack.last().cloned().unwrap_or_default();
                    stack.push(format!("{directory}{name}/"));
                    entry(&directory, name)
                })
                .collect(),
        );
    }
    if ls {
        let mut directory: Option<String> = None;
        return Some(
            views
                .iter()
                .map(|view| {
                    if let Some(header) = view.strip_suffix(':') {
                        directory = Some(format!("{}/", header.trim_end_matches('/')));
                        return other(view);
                    }
                    match &directory {
                        Some(d) if !view.trim().is_empty() && !view.starts_with("total ") => {
                            entry(d, view)
                        }
                        _ => other(view),
                    }
                })
                .collect(),
        );
    }
    if paths {
        return Some(
            views
                .iter()
                .map(|view| {
                    if view.trim().is_empty() || view.contains(": ") {
                        return other(view);
                    }
                    let path = view.trim_end_matches('/');
                    match path.rsplit_once('/') {
                        Some((directory, name)) => entry(&format!("{directory}/"), name),
                        None => entry("", path),
                    }
                })
                .collect(),
        );
    }
    None
}

/// The extension a listing summary counts a name under.
fn extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty() && !ext.is_empty() && ext.len() <= 8 && !ext.contains('/') =>
        {
            ext
        }
        _ => "none",
    }
}

/// A compact-v4 listing: a directory of more than `LISTING_DIRECTORY`
/// entries is summarized as `DIR/ files=N ext=E:n,...` before its first
/// entry, and only its first and last entries are shown. Lines are labeled
/// as everywhere else (`source:a`, or `source:a-b` range blocks). Returns
/// (body, shown lines, omitted lines), or None when the text is not a
/// listing or no directory folds.
/// `bare` (compact-v5) writes the same lines without `source:`; where the
/// budget cuts them is measured with it.
fn listing_view(record: &Record, available: usize, bare: bool) -> Option<(String, usize, usize)> {
    use std::fmt::Write;
    let views: Vec<Cow<'_, str>> = record
        .text
        .split_inclusive('\n')
        .map(clean::line_view)
        .collect();
    let lines = parse_listing(&views)?;
    let mut entries: HashMap<&str, Vec<usize>> = HashMap::default();
    for (i, line) in lines.iter().enumerate() {
        if let Some(directory) = &line.directory {
            entries.entry(directory.as_str()).or_default().push(i);
        }
    }
    let mut summary: HashMap<usize, String> = HashMap::default();
    let mut hidden = Bits::new(lines.len());
    for (directory, members) in &entries {
        if members.len() <= LISTING_DIRECTORY {
            continue;
        }
        let mut counts: HashMap<&str, usize> = HashMap::default();
        for &i in members {
            *counts.entry(extension(&lines[i].name)).or_default() += 1;
        }
        let mut counts: Vec<(&str, usize)> = counts.into_iter().collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let mut ext: Vec<String> = counts
            .iter()
            .take(LISTING_EXTENSIONS)
            .map(|(e, n)| format!("{e}:{n}"))
            .collect();
        if counts.len() > LISTING_EXTENSIONS {
            ext.push(format!("+{}", counts.len() - LISTING_EXTENSIONS));
        }
        let shown = if directory.is_empty() { "." } else { directory };
        summary.insert(
            members[0],
            format!("{shown} files={} ext={}\n", members.len(), ext.join(",")),
        );
        for &i in &members[1..members.len() - 1] {
            hidden.set(i);
        }
    }
    if summary.is_empty() {
        return None;
    }
    // Shown lines in order, cut where the budget ends: a summary always
    // travels with the entry it precedes.
    let base = record.start_line.unwrap_or(1);
    let mut body = String::new();
    // What the labelled body takes: the budget cuts the bare one alike.
    let mut measured = 0;
    let (mut shown, mut i) = (0, 0);
    let n = lines.len();
    while i < n {
        if hidden.get(i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < n && !hidden.get(j + 1) && !summary.contains_key(&(j + 1)) {
            j += 1;
        }
        let text = |k: usize| views[k].trim_end_matches(['\n', '\r']);
        let render = |end: usize, bare: bool| {
            let mut part = summary.get(&i).cloned().unwrap_or_default();
            let source = if bare {
                String::new()
            } else {
                format!("{}:", record.source)
            };
            if end + 1 - i >= BLOCK_MIN_LINES {
                let _ = writeln!(part, "{source}{}-{}", base + i, base + end);
                for k in i..=end {
                    let _ = writeln!(part, " {}", text(k));
                }
            } else {
                for k in i..=end {
                    let _ = writeln!(part, "{source}{} {}", base + k, text(k));
                }
            }
            part
        };
        let mut part = render(j, false);
        if measured + part.len() > available {
            // Keep the longest beginning of this run that fits, then stop.
            let room = available - measured;
            let mut end = i;
            let mut size = summary.get(&i).map_or(0, String::len)
                + record.source.len()
                + 2 * digits(base + j)
                + 3;
            while end <= j && size + text(end).len() + 2 <= room {
                size += text(end).len() + 2;
                end += 1;
            }
            if end == i {
                break;
            }
            part = render(end - 1, false);
            if measured + part.len() > available {
                break;
            }
            body.push_str(&if bare { render(end - 1, true) } else { part });
            shown += end - i;
            break;
        }
        measured += part.len();
        body.push_str(&if bare { render(j, true) } else { part });
        shown += j + 1 - i;
        i = j + 1;
    }
    (shown > 0).then(|| (body, shown, n - shown))
}

/// Matches of one file a compact-v4 search shows before other files' later
/// ones: its first and last, then those containing an anchor, then the
/// earliest.
const SEARCH_MATCHES: usize = 5;
/// Priority of a unit never selected: a line hidden behind a summary (a
/// lockfile's diff) or an install's ordinary progress.
const CAPPED: u8 = 9;

static SEARCH_MATCH: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^([^\s:][^:]*?):([0-9]+):").unwrap());
static SEARCH_CONTEXT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(\S.*?)-([0-9]+)-").unwrap());

/// The path of a `path:N:text` match or a `path-N-text` context line of
/// `grep -n`/`rg -n` output, and whether it is a match. A context line's path
/// is a guess until a neighbouring match names the same one.
fn search_line(view: &str) -> Option<(&str, bool)> {
    if let Some(c) = SEARCH_MATCH.captures(view) {
        return Some((c.get(1)?.as_str(), true));
    }
    SEARCH_CONTEXT
        .captures(view)
        .map(|c| (c.get(1).unwrap().as_str(), false))
}

/// Match lines per file of every text record.
fn search_totals(data: &Dataset) -> HashMap<String, usize> {
    let mut totals: HashMap<String, usize> = HashMap::default();
    for record in data.records.iter().filter(|r| r.value.is_none()) {
        for line in record.text.split_inclusive('\n') {
            if let Some((path, true)) = search_line(&clean::line_view(line)) {
                *totals.entry(path.to_owned()).or_default() += 1;
            }
        }
    }
    totals
}

/// The lines of a search shown first, and the others: each file's first and
/// last match, its anchored matches and then its earliest, up to
/// `SEARCH_MATCHES`, with the context lines of their runs (contiguous lines
/// of the same file). The other matches, and context whose run holds only
/// such matches, are shown only with budget to spare. Returns (later, first).
fn search_ranks(views: &[Cow<'_, str>], anchoring: &Anchoring<'_>) -> (Bits, Bits) {
    let n = views.len();
    let lines: Vec<Option<(&str, bool)>> = views.iter().map(|v| search_line(v)).collect();
    let mut capped = Bits::new(n);
    let mut by_path: HashMap<&str, Vec<usize>> = HashMap::default();
    for (i, line) in lines.iter().enumerate() {
        if let Some((path, true)) = line {
            by_path.entry(path).or_default().push(i);
        }
    }
    for matches in by_path.values().filter(|m| m.len() > SEARCH_MATCHES) {
        let mut keep = vec![matches[0], matches[matches.len() - 1]];
        let anchored = matches.iter().filter(|&&i| anchoring.matches(&views[i]));
        for &i in anchored.chain(matches.iter()) {
            if keep.len() == SEARCH_MATCHES {
                break;
            }
            if !keep.contains(&i) {
                keep.push(i);
            }
        }
        for &i in matches {
            if !keep.contains(&i) {
                capped.set(i);
            }
        }
    }
    // Runs of contiguous lines naming the same file: context is kept with
    // any shown match of its run.
    let mut first = Bits::new(n);
    let mut start = 0;
    while start < n {
        let Some((path, _)) = lines[start] else {
            start += 1;
            continue;
        };
        let mut end = start + 1;
        while end < n && lines[end].is_some_and(|(p, _)| p == path) {
            end += 1;
        }
        let matched = (start..end).any(|i| lines[i].is_some_and(|(_, m)| m));
        let shown = (start..end).any(|i| lines[i].is_some_and(|(_, m)| m) && !capped.get(i));
        if matched && !shown {
            capped.fill(start..end);
        } else if matched {
            for i in (start..end).filter(|&i| !capped.get(i)) {
                first.set(i);
            }
        }
        start = end;
    }
    (capped, first)
}

/// The role of a line inside a unified diff.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Diff {
    /// Not part of a diff: ranked by the ordinary rules.
    None,
    /// `diff --git`, `---`/`+++` of a diff without one, or `commit <hash>`.
    File,
    /// `index`, mode, rename and the `---`/`+++` a `diff --git` already names.
    Meta,
    /// `@@ -a,b +c,d @@`.
    Hunk,
    /// An added or removed line.
    Change,
    /// Unchanged context inside a hunk.
    Context,
    /// A commit message line of `git log -p`.
    Message,
}

/// Classify every line of a record that contains a unified diff (`diff
/// --git`, or `---` then `+++` then `@@`); None when it contains none.
fn diff_roles(views: &[Cow<'_, str>]) -> Option<Vec<Diff>> {
    let starts = |i: usize, prefix: &str| views.get(i).is_some_and(|v| v.starts_with(prefix));
    let plain = |i: usize| starts(i, "--- ") && starts(i + 1, "+++ ") && starts(i + 2, "@@ ");
    if !(0..views.len()).any(|i| starts(i, "diff --git ") || plain(i)) {
        return None;
    }
    #[derive(PartialEq)]
    enum State {
        Outside,
        /// After `git log`'s `commit <hash>`, before the first diff.
        Commit,
        Header,
        Hunk,
    }
    let commit = |view: &str| {
        view.strip_prefix("commit ").is_some_and(|rest| {
            let hash = rest.split(' ').next().unwrap_or("");
            hash.len() >= 7 && hash.bytes().all(|b| b.is_ascii_hexdigit())
        })
    };
    let mut state = State::Outside;
    let mut roles = vec![Diff::None; views.len()];
    let mut i = 0;
    while i < views.len() {
        let view = views[i].as_ref();
        roles[i] = if view.starts_with("diff --git ") {
            state = State::Header;
            Diff::File
        } else if commit(view) {
            state = State::Commit;
            Diff::File
        } else if state == State::Commit && view.starts_with("    ") {
            Diff::Message
        } else if state == State::Commit
            && (view.is_empty()
                || [
                    "Author:",
                    "AuthorDate:",
                    "Commit:",
                    "CommitDate:",
                    "Date:",
                    "Merge:",
                ]
                .iter()
                .any(|p| view.starts_with(p)))
        {
            Diff::Meta
        } else if state == State::Outside && plain(i) {
            roles[i + 1] = Diff::File;
            state = State::Header;
            i += 1;
            Diff::File
        } else if state != State::Outside && view.starts_with("@@ ") {
            state = State::Hunk;
            Diff::Hunk
        } else if state == State::Header
            && [
                "index ",
                "--- ",
                "+++ ",
                "new file mode",
                "deleted file mode",
                "old mode",
                "new mode",
                "similarity index",
                "dissimilarity index",
                "rename from",
                "rename to",
                "copy from",
                "copy to",
                "Binary files",
            ]
            .iter()
            .any(|p| view.starts_with(p))
        {
            Diff::Meta
        } else if state == State::Hunk && (view.starts_with('+') || view.starts_with('-')) {
            Diff::Change
        } else if state == State::Hunk
            && (view.is_empty() || view.starts_with(' ') || view.starts_with('\\'))
        {
            Diff::Context
        } else {
            state = State::Outside;
            Diff::None
        };
        i += 1;
    }
    Some(roles)
}

/// Lockfiles and generated or minified files: their diff is churn, not
/// evidence about the change.
fn generated_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "Cargo.lock"
            | "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "poetry.lock"
            | "uv.lock"
            | "go.sum"
            | "Gemfile.lock"
            | "composer.lock"
    ) || [".min.js", ".min.css", ".map"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// One file of a `diff --git` listing: its header line, the line after its
/// last, its path and its changed lines.
struct DiffSection {
    header: usize,
    end: usize,
    path: String,
    added: usize,
    removed: usize,
    generated: bool,
}

impl DiffSection {
    /// Changed lines per thousand lines of the section, at least 1.
    fn density(&self) -> u16 {
        let lines = (self.end - self.header).max(1);
        ((self.added + self.removed) * 1000 / lines).clamp(1, 1000) as u16
    }
}

/// The `diff --git` sections of a record classified by `diff_roles`. A
/// section ends at the next file header or commit.
fn diff_sections(views: &[Cow<'_, str>], roles: &[Diff]) -> Vec<DiffSection> {
    let mut sections: Vec<DiffSection> = Vec::new();
    for (i, view) in views.iter().enumerate() {
        if roles[i] == Diff::File {
            if let Some(last) = sections.last_mut()
                && last.end == usize::MAX
            {
                last.end = i;
            }
            if let Some(rest) = view.strip_prefix("diff --git ") {
                let path = rest
                    .rsplit_once(" b/")
                    .map_or(rest, |(_, path)| path)
                    .to_owned();
                sections.push(DiffSection {
                    header: i,
                    end: usize::MAX,
                    generated: generated_file(&path),
                    path,
                    added: 0,
                    removed: 0,
                });
            }
        } else if roles[i] == Diff::Change
            && let Some(last) = sections.last_mut()
            && last.end == usize::MAX
        {
            if view.starts_with('+') {
                last.added += 1;
            } else {
                last.removed += 1;
            }
        }
    }
    if let Some(last) = sections.last_mut()
        && last.end == usize::MAX
    {
        last.end = views.len();
    }
    sections
}

/// At most three visible characters: a closing brace, a blank line, a gutter.
fn is_trivial(line: &str) -> bool {
    line.chars().filter(|c| !c.is_whitespace()).take(4).count() <= 3
}

/// Compact-v3 selection: tiers are filled alternately from the head and the
/// tail so the final summary survives a flood of early diagnostics. When the
/// view carries diagnostics and cannot show every ordinary line anyway,
/// ordinary lines stop at a quarter of `budget`, so the view ends where the
/// evidence does instead of filling up with noise. `available` may exceed
/// `budget` on a second pass that spends range-block savings, which must not
/// raise that ceiling.
/// The fill order of `select_v3`, computed once for every refill pass.
struct Tiers {
    /// Units in fill order: tier by tier, alternating head and tail.
    order: Vec<usize>,
    /// Each tier's (priority, end in `order`, total size).
    tiers: Vec<(u8, usize, usize)>,
    /// Smallest unit size from each position of `order` to its end: once the
    /// remaining budget is below it, no later unit can be selected.
    smallest_after: Vec<usize>,
    diagnostics: bool,
}
impl Tiers {
    fn new(units: &[Unit<'_>]) -> Self {
        let mut order = Vec::with_capacity(units.len());
        let mut tiers = Vec::with_capacity(5);
        for priority in [4, 3, 2, 1, 0] {
            let mut tier: Vec<usize> = (0..units.len())
                .filter(|&i| units[i].priority == priority)
                .collect();
            if tier.iter().any(|&i| units[i].weight > 0) {
                // Weighted units fill in weight order (stable: source order
                // within a weight), unweighted ones after them.
                tier.sort_by_key(|&i| std::cmp::Reverse(units[i].weight));
                order.extend(&tier);
                let total = tier.iter().map(|&i| units[i].size).sum();
                tiers.push((priority, order.len(), total));
                continue;
            }
            let (mut head, mut tail) = (0, tier.len());
            while head < tail {
                order.push(tier[head]);
                head += 1;
                if head < tail {
                    tail -= 1;
                    order.push(tier[tail]);
                }
            }
            let total = tier.iter().map(|&i| units[i].size).sum();
            tiers.push((priority, order.len(), total));
        }
        let mut smallest_after = vec![usize::MAX; order.len() + 1];
        for k in (0..order.len()).rev() {
            smallest_after[k] = smallest_after[k + 1].min(units[order[k]].size);
        }
        Self {
            order,
            tiers,
            smallest_after,
            // A line an anchor ranked up is no outcome: it does not cap the
            // ordinary lines around it, or a source read that names a prompt
            // term would show a quarter of its budget.
            diagnostics: units
                .iter()
                .any(|u| matches!(u.priority, 2 | 3) && !u.anchored),
        }
    }
}

fn select_v3(units: &[Unit<'_>], plan: &Tiers, available: usize, budget: usize) -> Vec<usize> {
    let mut selected = vec![false; units.len()];
    let mut used = 0;
    let mut start = 0;
    'tiers: for &(priority, end, total) in &plan.tiers {
        let cap = if priority == 0 && plan.diagnostics && total > available.saturating_sub(used) {
            budget / ORDINARY_SHARE
        } else {
            available
        };
        let mut tier_used = 0;
        for k in start..end {
            if available.saturating_sub(used) < plan.smallest_after[k] {
                break 'tiers;
            }
            let i = plan.order[k];
            let size = units[i].size;
            if size <= available.saturating_sub(used) && tier_used + size <= cap {
                selected[i] = true;
                used += size;
                tier_used += size;
            }
        }
        start = end;
    }
    selected
        .into_iter()
        .enumerate()
        .filter_map(|(i, yes)| yes.then_some(i))
        .collect()
}

fn consecutive(a: &Unit<'_>, b: &Unit<'_>) -> bool {
    match (a.block(), b.block()) {
        (Some(x), Some(y)) => std::ptr::eq(a.record, b.record) && y == x + 1,
        _ => false,
    }
}

/// Compact-v3 rendering: three or more consecutive plain lines become one
/// `source:a-b` block whose lines are indented by a single space. In a
/// compact-v4 search (`totals`), two or more consecutive plain lines of one
/// file become `source:a-b file=PATH` and each line is shown without its
/// `PATH:` or `PATH-` prefix; the file's first block counts the matches not
/// shown (`more=N`).
/// `bare` (compact-v5) writes every label without its `source:`.
fn render_v3(
    units: &[Unit<'_>],
    selected: &[usize],
    totals: Option<&HashMap<String, usize>>,
    bare: bool,
    output: &mut String,
) {
    use std::fmt::Write;
    fn file_of<'u>(unit: &'u Unit<'_>) -> Option<(&'u str, bool)> {
        unit.block()?;
        search_line(unit.line.as_deref()?)
    }
    let mut shown: HashMap<&str, usize> = HashMap::default();
    if totals.is_some() {
        for &k in selected {
            if let Some((path, true)) = file_of(&units[k]) {
                *shown.entry(path).or_default() += 1;
            }
        }
    }
    let mut counted: HashSet<&str> = HashSet::default();
    let mut i = 0;
    while i < selected.len() {
        if let Some(totals) = totals
            && let Some((path, _)) = file_of(&units[selected[i]])
        {
            let mut j = i;
            while j + 1 < selected.len()
                && consecutive(&units[selected[j]], &units[selected[j + 1]])
                && file_of(&units[selected[j + 1]]).is_some_and(|(p, _)| p == path)
            {
                j += 1;
            }
            if j > i {
                let (first, last) = (&units[selected[i]], &units[selected[j]]);
                if !bare {
                    let _ = write!(output, "{}:", first.record.source);
                }
                let _ = write!(
                    output,
                    "{}-{} file={path}",
                    first.block().unwrap(),
                    last.block().unwrap()
                );
                let more = totals.get(path).map_or(0, |t| {
                    t.saturating_sub(shown.get(path).copied().unwrap_or(0))
                });
                if more > 0 && counted.insert(path) {
                    let _ = write!(output, " more={more}");
                }
                output.push('\n');
                for &k in &selected[i..=j] {
                    output.push(' ');
                    output.push_str(&units[k].line.as_deref().unwrap()[path.len() + 1..]);
                    output.push('\n');
                }
                i = j + 1;
                continue;
            }
        }
        // In a search, a range block never runs from one file into another.
        let same_file = |a: usize, b: usize| {
            totals.is_none()
                || file_of(&units[a]).map(|(p, _)| p) == file_of(&units[b]).map(|(p, _)| p)
        };
        let mut j = i;
        while j + 1 < selected.len()
            && consecutive(&units[selected[j]], &units[selected[j + 1]])
            && same_file(selected[j], selected[j + 1])
        {
            j += 1;
        }
        if j + 1 - i >= BLOCK_MIN_LINES {
            let (first, last) = (&units[selected[i]], &units[selected[j]]);
            if !bare {
                output.push_str(&first.record.source);
                output.push(':');
            }
            let _ = writeln!(
                output,
                "{}-{}",
                first.block().unwrap(),
                last.block().unwrap()
            );
            for &k in &selected[i..=j] {
                output.push(' ');
                output.push_str(units[k].line.as_deref().unwrap());
                output.push('\n');
            }
        } else {
            for &k in &selected[i..=j] {
                units[k].append(output, bare);
            }
        }
        i = j + 1;
    }
}

fn select(units: &[Unit<'_>], available: usize) -> Vec<usize> {
    let mut selected = vec![false; units.len()];
    let mut used = 0;
    for priority in [4, 3, 2, 1, 0] {
        for (i, unit) in units.iter().enumerate() {
            if unit.priority == priority && unit.size <= available.saturating_sub(used) {
                selected[i] = true;
                used += unit.size;
            }
        }
    }
    selected
        .into_iter()
        .enumerate()
        .filter_map(|(i, yes)| yes.then_some(i))
        .collect()
}

/// Compact-v3 selection and rendering, spending what range blocks save.
/// Selection measures labelled lines; `bare` (compact-v5) then writes the
/// same selection without `source:`.
fn fill_v3(
    units: &[Unit<'_>],
    available: usize,
    totals: Option<&HashMap<String, usize>>,
    bare: bool,
) -> (String, Vec<usize>) {
    let plan = Tiers::new(units);
    let mut selected = select_v3(units, &plan, available, available);
    let mut body = String::new();
    render_v3(units, &selected, totals, false, &mut body);
    // Range blocks cost less than the per-line labels selection counted.
    // Spend that slack on more evidence, re-measuring the rendered bytes
    // each time and keeping only a pass that still fits the budget.
    let mut extra = 0;
    for _ in 0..REFILL_PASSES {
        let slack = available.saturating_sub(body.len());
        if slack == 0 {
            break;
        }
        let more = select_v3(units, &plan, available + extra + slack, available);
        let mut extended = String::new();
        render_v3(units, &more, totals, false, &mut extended);
        if more.len() <= selected.len() || extended.len() > available {
            break;
        }
        extra += slack;
        selected = more;
        body = extended;
    }
    if bare {
        body.clear();
        render_v3(units, &selected, totals, true, &mut body);
    }
    (body, selected)
}

/// Failures listed on a test view's action line; the rest are counted.
const FAILURES_LISTED: usize = 5;
/// Longest entry of the action line, so one name cannot take the budget.
const FAILURE_ENTRY: usize = 160;

static CARGO_BLOCK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^---- (.+) stdout ----$").unwrap());
static CARGO_PANIC: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^thread '([^']+)'(?: \(\d+\))? panicked at (\S+?):(\d+):\d+:?$").unwrap()
});
static PYTEST_SUMMARY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(?:FAILED|ERROR) (\S+)(?: - .*)?$").unwrap());
static PYTEST_SECTION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^_{3,} (.+?) _{3,}$").unwrap());
static PYTEST_LOCATION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(\S+\.py):(\d+): ").unwrap());
static JEST_TITLE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*● (.+)$").unwrap());
static VITEST_TITLE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*FAIL\s+(\S+) > (.+)$").unwrap());
static JS_LOCATION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:\(|at |❯ )([^()\s]+?):(\d+):\d+\)?\s*$").unwrap());
static GO_FAIL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*--- FAIL: (\S+) \(").unwrap());
static GO_RUN: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^=== RUN\s+(\S+)$").unwrap());
static GO_LOCATION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s+(\S+\.go):(\d+)(?::| \+0x|\s*$)").unwrap());

/// A recognized failure: its name and, when the output gives one, the
/// `file:line` to open first.
struct Failure {
    name: String,
    location: Option<String>,
}

/// Failures of cargo test (`---- name stdout ----` with its panic location),
/// pytest (`FAILED`/`ERROR` summary lines with the deepest location of their
/// section), jest and vitest (`●` or `FAIL file > name` titles with their
/// first frame outside `node_modules`) and go test (`--- FAIL:`, or a test
/// that panicked, with its log or frame location). Empty when the output
/// matches none of them: nothing is guessed.
fn failures(text: &str) -> Vec<Failure> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut found: Vec<Failure> = Vec::new();
    let mut add = |name: &str, location: Option<String>| {
        if !found.iter().any(|f| f.name == name) {
            found.push(Failure {
                name: name.to_owned(),
                location,
            });
        }
    };
    // cargo test
    for (i, line) in lines.iter().enumerate() {
        if let Some(block) = CARGO_BLOCK.captures(line) {
            let name = &block[1];
            let location = lines[i + 1..]
                .iter()
                .take_while(|l| !CARGO_BLOCK.is_match(l))
                .find_map(|l| CARGO_PANIC.captures(l))
                .filter(|p| &p[1] == name)
                .map(|p| format!("{}:{}", &p[2], &p[3]));
            add(name, location);
        }
    }
    // pytest
    let sections: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            PYTEST_SECTION
                .captures(l)
                .map(|c| (i, c.get(1).unwrap().as_str()))
        })
        .collect();
    for line in &lines {
        let Some(summary) = PYTEST_SUMMARY.captures(line) else {
            continue;
        };
        let id = &summary[1];
        let test = id.rsplit("::").next().unwrap_or(id);
        let location = sections
            .iter()
            .position(|(_, title)| {
                *title == test
                    || title.ends_with(&format!(".{test}"))
                    || title.strip_prefix("ERROR collecting ") == Some(id)
            })
            .and_then(|k| {
                let start = sections[k].0 + 1;
                let end = sections.get(k + 1).map_or(lines.len(), |s| s.0);
                lines[start..end]
                    .iter()
                    .take_while(|l| !l.starts_with("====="))
                    .filter_map(|l| PYTEST_LOCATION.captures(l))
                    .last()
                    .map(|c| format!("{}:{}", &c[1], &c[2]))
            });
        add(id, location);
    }
    // jest and vitest
    let js_location = |from: usize| {
        lines[from..]
            .iter()
            .take_while(|l| !JEST_TITLE.is_match(l) && !VITEST_TITLE.is_match(l))
            .filter(|l| !l.contains("node_modules"))
            .find_map(|l| JS_LOCATION.captures(l))
            .map(|c| format!("{}:{}", &c[1], &c[2]))
    };
    for (i, line) in lines.iter().enumerate() {
        if let Some(title) = JEST_TITLE.captures(line) {
            let name = title[1].trim();
            if name != "Console" {
                add(name, js_location(i + 1));
            }
        } else if let Some(title) = VITEST_TITLE.captures(line) {
            add(
                &format!("{} > {}", &title[1], title[2].trim()),
                js_location(i + 1),
            );
        }
    }
    // go test
    // Where a Go test failed: its log line (`file_test.go:N:`) or, after a
    // panic, the deepest frame outside the Go toolchain (the crash site).
    let toolchain = |l: &&str| l.contains("/src/runtime/") || l.contains("/src/testing/");
    let go_location = |name: &str, at: usize| {
        let run = lines[..at]
            .iter()
            .rposition(|l| GO_RUN.captures(l).is_some_and(|c| &c[1] == name))
            .map_or(at, |r| r + 1);
        let after = lines[at + 1..]
            .iter()
            .take_while(|l| !l.starts_with("=== ") && !l.trim_start().starts_with("--- "));
        lines[run..at]
            .iter()
            .chain(after)
            .filter(|l| !toolchain(l))
            .find_map(|l| GO_LOCATION.captures(l))
            .map(|c| format!("{}:{}", &c[1], &c[2]))
    };
    for (i, line) in lines.iter().enumerate() {
        if let Some(fail) = GO_FAIL.captures(line) {
            add(&fail[1], go_location(&fail[1], i));
        } else if let Some(run) = GO_RUN.captures(line) {
            // A test that panicked may have no `--- FAIL` line.
            let name = &run[1];
            let next = lines[i + 1..].iter().find(|l| {
                l.starts_with("=== ") || l.starts_with("--- ") || l.starts_with("panic: ")
            });
            if next.is_some_and(|l| l.starts_with("panic: ")) {
                add(name, go_location(name, i));
            }
        }
    }
    found
}

/// The action line of a test view: `failed=N: name (file:line), ...`, at most
/// `FAILURES_LISTED` entries then `+M`. Metadata computed from the output,
/// never text claimed to occur in it; None when no failure is recognized.
fn failure_line(text: &str) -> Option<String> {
    let found = failures(text);
    if found.is_empty() {
        return None;
    }
    let entries: Vec<String> = found
        .iter()
        .take(FAILURES_LISTED)
        .map(|f| {
            let entry = match &f.location {
                Some(location) => format!("{} ({location})", f.name),
                None => f.name.clone(),
            };
            match entry.char_indices().nth(FAILURE_ENTRY) {
                Some((cut, _)) => format!("{}…", &entry[..cut]),
                None => entry,
            }
        })
        .collect();
    let more = found.len().saturating_sub(FAILURES_LISTED);
    let more = if more > 0 {
        format!(", +{more}")
    } else {
        String::new()
    };
    Some(format!(
        "failed={}: {}{more}\n",
        found.len(),
        entries.join(", ")
    ))
}

static CARGO_RESULT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;")
        .unwrap()
});

/// Where a totals line's sums come from, on the line itself.
const TOTALS_SOURCE: &str = "(every test result line, omitted ones included)";

/// The totals of a cargo test run over several test binaries:
/// `totals runs=N passed=P failed=F ignored=I (every test result line, omitted
/// ones included)`, since a view rarely has room for all of them and a reader
/// summing the ones shown gets a wrong count; the parenthesis says so, or a
/// reader may trust its own partial sum over the line. None below two such
/// lines: one is its own total.
fn totals_line(text: &str) -> Option<String> {
    let mut sums = [0u64; 3];
    let mut runs = 0usize;
    for line in text.lines() {
        if let Some(result) = CARGO_RESULT.captures(line.trim_end_matches('\r')) {
            runs += 1;
            for (sum, k) in sums.iter_mut().zip(1..) {
                *sum = sum.saturating_add(result[k].parse().unwrap_or(u64::MAX));
            }
        }
    }
    (runs >= 2).then(|| {
        format!(
            "totals runs={runs} passed={} failed={} ignored={} {TOTALS_SOURCE}\n",
            sums[0], sums[1], sums[2]
        )
    })
}

/// The metadata around a view's body. Compact-v1 to v3 open with a header
/// naming the artifact and close with a footer counting omissions; compact-v4
/// writes one header line, once the omitted count is known.
struct Frame {
    version: Version,
    scan_complete: bool,
    /// The opening header of v1 to v3; empty in v4.
    head: String,
}

impl Frame {
    fn new(data: &Dataset, artifact: &str, version: Version) -> Self {
        Self {
            version,
            scan_complete: data.scan_complete,
            head: if version >= Version::V4 {
                String::new()
            } else {
                header(data, artifact, version)
            },
        }
    }
    /// Bytes of metadata a view of `units` units may need. V3 reserves exactly
    /// the widest footer it can emit and v4 its widest header; earlier
    /// versions keep their fixed reserve so their bytes stay identical.
    fn reserve(&self, units: usize) -> usize {
        match self.version {
            // V5 reserves v4's header, so both select the same units.
            Version::V4 | Version::V5 => header_v4(
                PLACEHOLDER_ID,
                Version::V4,
                units,
                units,
                self.scan_complete,
            )
            .len(),
            Version::V3 => self.head.len() + footer(units).len(),
            Version::V1 | Version::V2 => self.head.len() + 160,
        }
    }
    /// Metadata of a complete table.
    fn complete_reserve(&self) -> usize {
        match self.version {
            Version::V4 | Version::V5 => {
                header_v4(PLACEHOLDER_ID, Version::V4, 0, 0, self.scan_complete).len()
            }
            _ => self.head.len() + TABLE_TAIL.len(),
        }
    }
    fn complete(&self, table: &str) -> String {
        match self.version {
            Version::V4 | Version::V5 => self.header(0, 0) + table,
            _ => format!("{}{table}{TABLE_TAIL}", self.head),
        }
    }
    fn finish(&self, body: &str, omitted: usize) -> String {
        self.finish_abridged(body, omitted, 0)
    }
    /// `finish` for a body of which `abridged` units are shown in part;
    /// earlier versions do not count them.
    fn finish_abridged(&self, body: &str, omitted: usize, abridged: usize) -> String {
        match self.version {
            Version::V4 | Version::V5 => self.header(omitted, abridged) + body,
            _ => format!("{}{body}{}", self.head, footer(omitted)),
        }
    }
    /// A compact-v4 or v5 header with the placeholder ID.
    fn header(&self, omitted: usize, abridged: usize) -> String {
        let id = &PLACEHOLDER_ID[..id_len(self.version)];
        header_v4(id, self.version, omitted, abridged, self.scan_complete)
    }
}

/// Selected units shown in part: lines cut behind `text_truncated`, and
/// `similar=N` groups, whose members differ from the line shown (an exact
/// `repeat=N` hides nothing).
fn cut(units: &[Unit<'_>], selected: &[usize]) -> usize {
    selected
        .iter()
        .filter(|&&i| units[i].truncated > 0 || matches!(units[i].label, Label::Similar { .. }))
        .count()
}

/// The footer of a complete table before compact-v4.
const TABLE_TAIL: &str =
    "[scopelet display_complete=true omitted_units=0; rows map positionally to columns]\n";

/// `original` is the text the records were parsed from, when there is one:
/// compact-v3 shows it line by line when no whole JSON record fits.
#[allow(clippy::too_many_arguments)]
fn view(
    data: &Dataset,
    artifact: &str,
    budget: usize,
    version: Version,
    original: Option<&str>,
    max_units: usize,
    kind: Kind,
    anchoring: &mut Anchoring<'_>,
) -> Result<(String, usize)> {
    ensure!(
        (512..=1024 * 1024).contains(&budget),
        "internal compact budget must be 512..1048576"
    );
    let count = data
        .records
        .iter()
        .try_fold(0usize, |n, r| {
            n.checked_add(if r.value.is_some() {
                1
            } else {
                clean::line_count(&r.text)
            })
        })
        .unwrap_or(usize::MAX);
    ensure!(
        count <= max_units,
        "compact input exceeds {max_units} units; use an explicit query"
    );
    let frame = Frame::new(data, artifact, version);
    let available = budget.saturating_sub(frame.reserve(count));
    let uniform = compact_table::uniform(data);
    // Compact-v4 tables name constant columns once; a complete one needs no
    // indices, its rows being every record in order.
    let v4 = version >= Version::V4;
    let csv = Grammar::of(version).tables;
    // Compact-v5 labels a view of the one `input` text by line alone.
    let bare = kind.bare
        && data
            .records
            .iter()
            .all(|r| r.value.is_none() && r.source == "input");
    if uniform {
        let indices: Vec<_> = (0..data.records.len()).collect();
        if let Some(table) = compact_table::encode_with(
            data,
            &indices,
            version.tables() && !v4,
            v4,
            csv,
            budget.saturating_sub(frame.complete_reserve()),
        ) {
            return Ok((frame.complete(&table), indices.len()));
        }
    }
    if v4
        && let [record] = data.records.as_slice()
        && let Some(value) = &record.value
        && let Some(table) =
            compact_table::envelope(value, budget.saturating_sub(frame.complete_reserve()), csv)
    {
        return Ok((frame.complete(&table), 1));
    }
    // Matches per file of a compact-v4 search, for its `more=N` counts.
    let totals = kind.search.then(|| search_totals(data));
    let totals = totals.as_ref();
    // A path listing too long to show whole folds its large directories.
    if kind.search
        && let [record] = data.records.as_slice()
        && record.value.is_none()
        && record.text.len() > available
        && let Some((body, shown, omitted)) = listing_view(record, available, bare)
    {
        let output = frame.finish(&body, omitted);
        ensure!(output.len() <= budget, "compact metadata exceeds budget");
        return Ok((output, shown));
    }
    // Minified JSON too large to show whole (an API response on one line):
    // its objects' short fields as a table, rather than one cut line.
    // Only where whole records cannot be selected: one is a quarter of the
    // budget or more. Rows with a diagnostic or an anchor come first.
    let important = |value: &serde_json::Value| {
        signal_value(value, &|s| diagnostic(&STRONG, s)).is_some()
            || anchoring.matches(&value.to_string())
    };
    if v4
        && let Some(text) = original
        && clean::line_count(text) < data.records.len().max(2)
        && data.records.iter().any(|r| {
            r.value
                .as_ref()
                .is_some_and(|v| encoding::size(v, available / 4).is_none())
        })
        && let Some(projection) = compact_table::projection(data, available, &important, csv)
    {
        let omitted = projection.total - projection.shown;
        let output = frame.finish_abridged(&projection.text, omitted, projection.abridged);
        ensure!(output.len() <= budget, "compact metadata exceeds budget");
        return Ok((output, projection.shown));
    }
    anchoring.reset(available);
    let mut units = units(data, available, version, kind, anchoring);
    if uniform && version.tables() {
        // Exact row costs include positional provenance. Fixed schema/envelope costs
        // are counted by the same serializer as the final table.
        let (columns, _) = compact_table::columns(data, v4);
        if let Some(empty) = compact_table::encode_with(data, &[], true, v4, false, available) {
            let varying_sources = data
                .records
                .iter()
                .any(|r| r.source != data.records[0].source);
            for (i, unit) in units.iter_mut().enumerate() {
                unit.size = compact_table::row_size(unit.record, &columns, available)
                    .unwrap_or(available + 1)
                    + digits(i)
                    + 2
                    + if varying_sources {
                        encoding::size(&unit.record.source, available).unwrap_or(available + 1) + 1
                    } else {
                        0
                    };
            }
            let indices = select(&units, available.saturating_sub(empty.len()));
            if !indices.is_empty()
                && let Some(table) =
                    compact_table::encode_with(data, &indices, true, v4, csv, available)
            {
                let output = frame.finish(&table, units.len() - indices.len());
                return Ok((output, indices.len()));
            }
        }
        // No table row fits: recompute ordinary-record costs before falling back.
        anchoring.reset(available);
        units = self::units(data, available, version, kind, anchoring);
    }
    let (body, selected) = if version.profiled() {
        let (body, selected) = fill_v3(&units, available, totals, bare);
        if selected.is_empty()
            && let Some(text) = original
            && data.records.iter().any(|r| r.value.is_some())
        {
            // No whole JSON record fits: show the document by its source
            // lines, which keep absolute labels into the same original.
            let lines = clean::line_count(text);
            let record = Record {
                source: "input".into(),
                blob: data.records[0].blob.clone(),
                start_line: Some(1),
                end_line: Some(lines),
                text: text.to_owned(),
                value: None,
                omitted_lines: None,
                text_truncated: false,
            };
            let available = budget.saturating_sub(frame.reserve(lines));
            let mut seen = Seen::default();
            let mut units = Vec::new();
            anchoring.reset(available);
            text_units_v3(&record, available, kind, &mut seen, anchoring, &mut units);
            // The document's own lines: the one `input` text.
            let (body, selected) = fill_v3(&units, available, totals, kind.bare);
            let output =
                frame.finish_abridged(&body, units.len() - selected.len(), cut(&units, &selected));
            ensure!(output.len() <= budget, "compact metadata exceeds budget");
            return Ok((output, selected.len()));
        }
        (body, selected)
    } else {
        let selected = select(&units, available);
        let mut body = String::new();
        for &i in &selected {
            units[i].append(&mut body, false);
        }
        (body, selected)
    };
    let output = frame.finish_abridged(&body, units.len() - selected.len(), cut(&units, &selected));
    ensure!(output.len() <= budget, "compact metadata exceeds budget");
    Ok((output, selected.len()))
}
