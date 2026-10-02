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
/// Word boundaries are ASCII: the vocabulary is ASCII, and Unicode boundaries
/// cost a slower engine on every line.
static STRONG: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i:(?-u:\b)(error|errors|failed|failure|panic|panicked|exception|traceback|fatal|assertionerror|caused by|test result|tests? passed|tests? failed)(?-u:\b))|(?-u:\b)[A-Z][A-Za-z]*(Error|Exception)(?-u:\b)|npm ERR!|^\s*(FAIL|FAILED|--- FAIL|E {2,}|FATAL|×|✕|✗|✖)").unwrap()
});
/// Advisory lines: shown once per template, never ahead of a diagnostic.
/// A passing test is not an advisory. Listing it here grouped every `PASS`
/// line by exact text, which stopped a suite of passes from folding and let
/// them crowd out the failure's own detail.
static WEAK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i:(?-u:\b)(warning|warn|deprecated|deprecation)(?-u:\b))").unwrap()
});
/// Stack frames: context for a diagnostic, wherever they are. JavaScript and
/// Python frames, Java's `at pkg.Class.method(File.java:N)`, Go's
/// `file.go:N +0x1d` and rustc's `--> file:line:col` locations.
static FRAME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"^\s+at .*\(.*:\d+:\d+\)|^\s+File ".*", line \d+|^\s+at [\w$.<>/-]+\([^()]*\)\s*$|\.go:\d+ \+0x[0-9a-f]+|^\s*--> \S+:\d+:\d+"#).unwrap()
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Version {
    #[value(name = "1")]
    V1,
    #[value(name = "2")]
    V2,
    #[default]
    #[value(name = "3")]
    V3,
}
impl Version {
    pub fn configured(explicit: Option<Self>) -> Result<Self> {
        if let Some(version) = explicit {
            return Ok(version);
        }
        match std::env::var("SCOPELET_COMPACT_VERSION") {
            Ok(v) if v == "1" => Ok(Self::V1),
            Ok(v) if v == "2" => Ok(Self::V2),
            Ok(v) if v == "3" => Ok(Self::V3),
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            _ => anyhow::bail!("SCOPELET_COMPACT_VERSION must be 1, 2 or 3"),
        }
    }
    /// Partial JSON tables with indices and provenance (everything after v1).
    fn tables(self) -> bool {
        self != Self::V1
    }
}

/// Conservative detection retained for callers; automatic ingestion reuses parsed values.
pub fn detect(text: &str) -> Format {
    match Parsed::detected(text.to_owned()) {
        Parsed::Text(_) => Format::Text,
        Parsed::Json(_) => Format::Json,
        Parsed::Jsonl(_) => Format::Jsonl,
    }
}

pub fn automatic(bytes: &[u8], store: &Store, budget: usize) -> Result<Vec<u8>> {
    automatic_with(bytes, budget, Version::V1, || Ok(store.clone())).map(Cow::into_owned)
}

/// The store factory is not invoked for a rejected or ineligible substitution.
pub fn automatic_lazy<'a>(
    bytes: &'a [u8],
    path: Option<PathBuf>,
    budget: usize,
    version: Version,
) -> Result<Cow<'a, [u8]>> {
    automatic_with(bytes, budget, version, || Store::open(path))
}

fn automatic_with(
    bytes: &[u8],
    budget: usize,
    version: Version,
    store: impl FnOnce() -> Result<Store>,
) -> Result<Cow<'_, [u8]>> {
    ensure!(
        (1024..=1024 * 1024).contains(&budget),
        "max_bytes must be 1024..1048576"
    );
    let max_lines = if version == Version::V3 {
        V3_MAX_LINES
    } else {
        MAX_LINES
    };
    if bytes.len() <= SMALL || memchr::memchr_iter(b'\n', bytes).count() > max_lines {
        return Ok(Cow::Borrowed(bytes));
    }
    // Existing Scopelet output and host previews are never compressed again.
    if SCOPELET_MARKER.find(bytes).is_some() || PERSISTED_MARKER.find(bytes).is_some() {
        return Ok(Cow::Borrowed(bytes));
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(Cow::Borrowed(bytes));
    };
    let parsed = Parsed::detected(text.to_owned());
    let format = parsed.format();
    let mut data = Dataset {
        records: parsed.records("input", String::new()),
        ..Dataset::default()
    };
    data.notes
        .push("Bytes supplied to the compressor; completeness before capture is unknown.".into());
    let (mut result, shown) = view(&data, PLACEHOLDER, budget, version, Some(text), max_lines)?;
    if shown == 0 || result.len() + 512 > bytes.len() || result.len() * 5 > bytes.len() * 4 {
        return Ok(Cow::Borrowed(bytes));
    }
    let store = store()?;
    let blob = store.put("blob", bytes)?;
    for record in &mut data.records {
        record.blob = Some(blob.clone());
    }
    data.snapshots.push(Snapshot {
        source: "input".into(),
        blob,
        bytes: bytes.len(),
        local_path: None,
    });
    // Compact-v3 artifacts name the original blob instead of storing every
    // record a second time; earlier versions keep their exact v1 artifacts.
    let artifact = if version == Version::V3 {
        store.put_json(&DatasetRef::new(
            &data,
            RecordsRef {
                blob: data.snapshots[0].blob.clone(),
                format,
                source: "input".into(),
            },
        ))?
    } else {
        store.put_json(&data)?
    };
    // Only substitute our header, never placeholder-looking text in original evidence.
    let end = result.find('\n').unwrap() + 1;
    result.replace_range(..end, &header(&data, &artifact, version));
    ensure!(result.len() <= budget, "compact metadata exceeds budget");
    Ok(Cow::Owned(result.into_bytes()))
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
    let (mut text, _) = view(data, PLACEHOLDER, budget, version, None, MAX_LINES)?;
    let id = store.put_json(data)?;
    let end = text.find('\n').unwrap() + 1;
    text.replace_range(..end, &header(data, &id, version));
    Ok(text)
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
    fn append(&self, output: &mut String) {
        if let Some(line) = &self.line {
            let start = output.len();
            self.label.write(&self.record.source, output);
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
            debug_assert_eq!(output.len() - start, self.size, "unit size drifted");
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
    fn write(&self, source: &str, output: &mut String) {
        use std::fmt::Write;
        if let Self::Prefix(prefix) = self {
            output.push_str(prefix);
            return;
        }
        output.push_str(source);
        // Writing into a String cannot fail.
        let _ = match *self {
            Self::Prefix(_) => unreachable!(),
            Self::Line(a) => write!(output, ":{a}"),
            Self::Run { a, b, n } => write!(output, ":{a}-{b} repeat={n}"),
            Self::Repeat { a, n, b } => write!(output, ":{a} repeat={n} last={b}"),
            Self::Similar { a, n, b } => write!(output, ":{a} similar={n} last={b}"),
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

fn units(data: &Dataset, limit: usize, version: Version) -> Vec<Unit<'_>> {
    let mut units = Vec::new();
    let mut distinct = BTreeSet::new();
    let mut seen = Seen::default();
    let v2_signal = |s: &str| SIGNAL.is_match(s);
    let v3_signal = |s: &str| diagnostic(&STRONG, s);
    let signal: &dyn Fn(&str) -> bool = if version == Version::V3 {
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
            units.push(Unit {
                record,
                line: None,
                label: Label::Prefix(String::new()),
                truncated: 0,
                size: encoding::size(value, limit).unwrap_or(limit + 1) + record.source.len() + 3,
                priority,
            });
            continue;
        }
        if version == Version::V3 {
            text_units_v3(record, limit, &mut seen, &mut units);
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
fn text_units_v3<'a>(record: &'a Record, limit: usize, seen: &mut Seen, units: &mut Vec<Unit<'a>>) {
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
            if diagnostic(&STRONG, &views[i]) {
                strong.set(i);
            } else if diagnostic(&WEAK, &views[i]) {
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
        let slot = if slot_of_first[origin] != NONE {
            slot_of_first[origin]
        } else if matches!(role(i), Diff::File | Diff::Change) {
            // Headers and changes are evidence: they never fold by template,
            // only with the very same text (a file every commit touches).
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
                    if reach.is_some_and(|r| r <= GLUE_REACH) {
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
        units.push(Unit::text_v3(record, label, view, raw, priority, limit));
        if role(i) == Diff::File {
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
            let (unit, ..) = trivial_units[k];
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
            let tier: Vec<usize> = (0..units.len())
                .filter(|&i| units[i].priority == priority)
                .collect();
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
            diagnostics: units.iter().any(|u| matches!(u.priority, 2 | 3)),
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
/// `source:a-b` block whose lines are indented by a single space.
fn render_v3(units: &[Unit<'_>], selected: &[usize], output: &mut String) {
    let mut i = 0;
    while i < selected.len() {
        let mut j = i;
        while j + 1 < selected.len() && consecutive(&units[selected[j]], &units[selected[j + 1]]) {
            j += 1;
        }
        if j + 1 - i >= BLOCK_MIN_LINES {
            let (first, last) = (&units[selected[i]], &units[selected[j]]);
            output.push_str(&format!(
                "{}:{}-{}\n",
                first.record.source,
                first.block().unwrap(),
                last.block().unwrap()
            ));
            for &k in &selected[i..=j] {
                output.push(' ');
                output.push_str(units[k].line.as_deref().unwrap());
                output.push('\n');
            }
        } else {
            for &k in &selected[i..=j] {
                units[k].append(output);
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
fn fill_v3(units: &[Unit<'_>], available: usize) -> (String, Vec<usize>) {
    let plan = Tiers::new(units);
    let mut selected = select_v3(units, &plan, available, available);
    let mut body = String::new();
    render_v3(units, &selected, &mut body);
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
        render_v3(units, &more, &mut extended);
        if more.len() <= selected.len() || extended.len() > available {
            break;
        }
        extra += slack;
        selected = more;
        body = extended;
    }
    (body, selected)
}

/// `original` is the text the records were parsed from, when there is one:
/// compact-v3 shows it line by line when no whole JSON record fits.
fn view(
    data: &Dataset,
    artifact: &str,
    budget: usize,
    version: Version,
    original: Option<&str>,
    max_units: usize,
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
    let mut output = header(data, artifact, version);
    // V3 reserves exactly the widest footer it can emit; earlier versions keep
    // their fixed reserve so their bytes stay identical.
    let reserve = if version == Version::V3 {
        footer(count).len()
    } else {
        160
    };
    let available = budget.saturating_sub(output.len() + reserve);
    let uniform = compact_table::uniform(data);
    if uniform {
        let indices: Vec<_> = (0..data.records.len()).collect();
        let tail =
            "[scopelet display_complete=true omitted_units=0; rows map positionally to columns]\n";
        if let Some(table) = compact_table::encode(
            data,
            &indices,
            version.tables(),
            budget.saturating_sub(output.len() + tail.len()),
        ) {
            output.push_str(&table);
            output.push_str(tail);
            return Ok((output, indices.len()));
        }
    }
    let mut units = units(data, available, version);
    if uniform && version.tables() {
        // Exact row costs include positional provenance. Fixed schema/envelope costs
        // are counted by the same serializer as the final table.
        if let Some(empty) = compact_table::encode(data, &[], true, available) {
            let varying_sources = data
                .records
                .iter()
                .any(|r| r.source != data.records[0].source);
            for (i, unit) in units.iter_mut().enumerate() {
                unit.size = compact_table::row_size(unit.record, available)
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
                && let Some(table) = compact_table::encode(data, &indices, true, available)
            {
                output.push_str(&table);
                output.push_str(&footer(units.len() - indices.len()));
                return Ok((output, indices.len()));
            }
        }
        // No table row fits: recompute ordinary-record costs before falling back.
        units = self::units(data, available, version);
    }
    let selected = if version == Version::V3 {
        let (body, selected) = fill_v3(&units, available);
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
            let available = budget.saturating_sub(output.len() + footer(lines).len());
            let mut seen = Seen::default();
            let mut units = Vec::new();
            text_units_v3(&record, available, &mut seen, &mut units);
            let (body, selected) = fill_v3(&units, available);
            output.push_str(&body);
            output.push_str(&footer(units.len() - selected.len()));
            ensure!(output.len() <= budget, "compact metadata exceeds budget");
            return Ok((output, selected.len()));
        }
        output.push_str(&body);
        selected
    } else {
        let selected = select(&units, available);
        for &i in &selected {
            units[i].append(&mut output);
        }
        selected
    };
    output.push_str(&footer(units.len() - selected.len()));
    ensure!(output.len() <= budget, "compact metadata exceeds budget");
    Ok((output, selected.len()))
}
