//! Recoverable presentation. Selection borrows evidence; storage follows acceptance.
use crate::{clean, compact_table, encoding, model::*, sources::Parsed, store::Store};
use anyhow::{Result, ensure};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::LazyLock,
};

pub const DEFAULT_BUDGET: usize = 4096;
pub const SMALL: usize = 2048;

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
/// Consecutive plain lines needed before a range block pays for its header.
const BLOCK_MIN_LINES: usize = 3;
/// Passes that re-spend the bytes range blocks saved. Each pass can only add
/// units, and rendering is measured after each, so this bounds work rather
/// than correctness.
const REFILL_PASSES: usize = 4;
const PLACEHOLDER: &str =
    "artifact:0000000000000000000000000000000000000000000000000000000000000000";
static SIGNAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(error|failed|failure|panic|panicked|exception|traceback|warning|assertionerror|caused by|test result|tests? passed|tests? failed)\b|^\s*(FAIL|PASS|E\s+|FATAL|×|✕)").unwrap()
});
/// Compact-v3 vocabulary: diagnostics and summaries that decide an outcome.
/// Anchored markers stay case-sensitive: their convention is upper case, and
/// matching them loosely turns any line starting with `e ` into a diagnostic.
static STRONG: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i:\b(error|errors|failed|failure|panic|panicked|exception|traceback|fatal|assertionerror|caused by|test result|tests? passed|tests? failed)\b)|npm ERR!|^\s*(FAIL|FAILED|E {2,}|FATAL|×|✕|✗)").unwrap()
});
/// Advisory lines: shown once per template, never ahead of a diagnostic.
/// A passing test is not an advisory. Listing it here grouped every `PASS`
/// line by exact text, which stopped a suite of passes from folding and let
/// them crowd out the failure's own detail.
static WEAK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i:\b(warning|warn|deprecated|deprecation)\b)").unwrap());
/// Stack frames: context for a diagnostic, wherever they are.
static FRAME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"^\s+at .*\(.*:\d+:\d+\)|^\s+File ".*", line \d+"#).unwrap()
});

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
    if bytes.len() <= SMALL || bytes.iter().filter(|&&b| b == b'\n').count() > 100_000 {
        return Ok(Cow::Borrowed(bytes));
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(Cow::Borrowed(bytes));
    };
    if text.contains("[scopelet ") || text.contains("<persisted-output>") {
        return Ok(Cow::Borrowed(bytes));
    }
    let mut data = Dataset {
        records: Parsed::detected(text.to_owned()).records("input", String::new()),
        ..Dataset::default()
    };
    data.notes
        .push("Bytes supplied to the compressor; completeness before capture is unknown.".into());
    let (mut result, shown) = view(&data, PLACEHOLDER, budget, version)?;
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
    let artifact = store.put_json(&data)?;
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
    let (mut text, _) = view(data, PLACEHOLDER, budget, version)?;
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
    /// Label and metadata before the text, including the separating space.
    prefix: String,
    size: usize,
    priority: u8,
    /// Source line of a plain single-line unit that may join a range block.
    block: Option<usize>,
}
fn digits(n: usize) -> usize {
    if n == 0 { 1 } else { n.ilog10() as usize + 1 }
}
impl<'a> Unit<'a> {
    fn text(record: &'a Record, prefix: String, line: Cow<'a, str>, priority: u8) -> Self {
        let size = prefix.len() + line.len() + usize::from(!line.ends_with('\n'));
        Self {
            record,
            line: Some(line),
            prefix,
            size,
            priority,
            block: None,
        }
    }
    /// Compact-v3 text unit; a line too large for the budget is cut on a
    /// character boundary behind a `text_truncated bytes=N` marker.
    fn text_v3(
        record: &'a Record,
        label: String,
        view: Cow<'a, str>,
        raw: &str,
        priority: u8,
        limit: usize,
        block: Option<usize>,
    ) -> Self {
        let size = label.len() + 1 + view.len() + 1;
        let cap = limit.min(TRUNCATED_PREFIX);
        let prefix = format!("{label} text_truncated bytes={} ", clean::body(raw).len());
        if size <= limit || prefix.len() + 1 >= cap {
            return Self {
                block,
                ..Self::text(record, label + " ", view, priority)
            };
        }
        let cut = (0..=cap - prefix.len() - 1)
            .rev()
            .find(|&i| view.is_char_boundary(i))
            .unwrap();
        Self {
            record,
            line: Some(Cow::Owned(view[..cut].to_owned())),
            size: prefix.len() + cut + 1,
            prefix,
            priority,
            block: None,
        }
    }
    fn append(&self, output: &mut String) {
        if let Some(line) = &self.line {
            output.push_str(&self.prefix);
            output.push_str(line);
            if !line.ends_with('\n') {
                output.push('\n');
            }
        } else {
            output.push_str(&self.record.source);
            output.push_str(": ");
            output.push_str(&self.record.value.as_ref().unwrap().to_string());
            output.push('\n');
        }
    }
}

fn signal_value<'v>(value: &'v serde_json::Value, signal: &regex::Regex) -> Option<&'v str> {
    match value {
        serde_json::Value::String(s) => signal.is_match(s).then_some(s),
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
    let signal = if version == Version::V3 {
        &*STRONG
    } else {
        &*SIGNAL
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
                prefix: String::new(),
                size: encoding::size(value, limit).unwrap_or(limit + 1) + record.source.len() + 3,
                priority,
                block: None,
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

/// Compact-v3 text units work on display copies of the lines (terminal
/// control sequences and carriage-return overwrites removed) while labels stay
/// absolute source lines. Identical lines fold into their first occurrence
/// wherever they are; lines without a diagnostic fold with the lines sharing
/// their template; oversized lines are cut instead of vetoing the view.
fn text_units_v3<'a>(record: &'a Record, limit: usize, seen: &mut Seen, units: &mut Vec<Unit<'a>>) {
    struct Group {
        first: usize,
        last: usize,
        count: usize,
        /// Every member is the same text (otherwise members share a template).
        exact: bool,
        context: bool,
        nearby: bool,
    }
    let lines: Vec<&str> = record.text.split_inclusive('\n').collect();
    let n = lines.len();
    let views: Vec<Cow<'a, str>> = lines.iter().map(|line| clean::line_view(line)).collect();
    let strong: Vec<bool> = views.iter().map(|view| STRONG.is_match(view)).collect();
    let weak: Vec<bool> = views
        .iter()
        .zip(&strong)
        .map(|(view, &strong)| !strong && WEAK.is_match(view))
        .collect();
    // Context is the window around a diagnostic; head and tail lines only
    // count for priority.
    let mut context = vec![false; n];
    for (i, &strong) in strong.iter().enumerate() {
        if strong {
            context[i.saturating_sub(3)..(i + 9).min(n)].fill(true);
        }
    }
    let mut nearby = context.clone();
    nearby[..n.min(3)].fill(true);
    nearby[n.saturating_sub(5)..].fill(true);

    // Diagnostics group by exact text: their variable parts (counters, ids)
    // are evidence and stay visible. Other lines group by template.
    let mut groups: Vec<Group> = Vec::new();
    let mut owner = Vec::with_capacity(n);
    let mut by_text: HashMap<&str, usize> = HashMap::new();
    let mut by_template: HashMap<String, usize> = HashMap::new();
    // One reusable buffer: a template key is only copied when it is new, so a
    // scan over many same-shaped lines allocates once, not once per line.
    let mut key = String::new();
    for (i, view) in views.iter().enumerate() {
        // Identical text always shares a group, and identical text has an
        // identical template, so the exact-text hit skips building one. On a
        // log of repeated lines that is every line after the first.
        let slot = if let Some(&slot) = by_text.get(view.as_ref()) {
            &mut { slot }
        } else if strong[i] || weak[i] {
            by_text.entry(view).or_insert(groups.len())
        } else {
            clean::template_into(view, &mut key);
            let slot = match by_template.get(key.as_str()) {
                Some(&slot) => slot,
                None => {
                    by_template.insert(key.clone(), groups.len());
                    groups.len()
                }
            };
            by_text.insert(view, slot);
            &mut { slot }
        };
        if *slot == groups.len() {
            groups.push(Group {
                first: i,
                last: i,
                count: 1,
                exact: true,
                context: context[i],
                nearby: nearby[i],
            });
        } else {
            let group = &mut groups[*slot];
            group.last = i;
            group.count += 1;
            group.exact &= views[group.first] == *view;
            group.context |= context[i];
            group.nearby |= nearby[i];
        }
        owner.push(*slot);
    }
    // A few repetitions inside a diagnostic's context stay in source order;
    // massive repetition folds wherever it is.
    let folds: Vec<bool> = groups
        .iter()
        .map(|g| g.count > 1 && (!g.context || g.count >= FOLD_IN_CONTEXT))
        .collect();
    let folded: usize = groups
        .iter()
        .zip(&folds)
        .filter(|(_, folds)| **folds)
        .map(|(g, _)| g.count)
        .sum();
    let mostly_folded = n >= FOLD_MAJORITY_MIN_LINES && folded * 10 >= n * FOLD_MAJORITY_TENTHS;
    let base = record.start_line.unwrap_or(1);
    let mut i = 0;
    while i < n {
        let slot = owner[i];
        let group = &groups[slot];
        let (first, last, count, exact, nearby) = if folds[slot] {
            if group.first != i {
                i += 1;
                continue;
            }
            (
                group.first,
                group.last,
                group.count,
                group.exact,
                group.nearby,
            )
        } else {
            // Unfolded members still merge with contiguous identical lines.
            let mut j = i;
            while j + 1 < n && owner[j + 1] == slot && views[j + 1] == views[i] {
                j += 1;
            }
            (i, j, j + 1 - i, true, nearby[i..=j].iter().any(|&v| v))
        };
        let priority = if first == 0 || last + 1 == n {
            4
        } else if strong[i] {
            // The first diagnostic of each template, then its variants.
            if first_of_template(&mut seen.strong, &views[i], &mut key) {
                3
            } else {
                2
            }
        } else if weak[i] {
            if first_of_template(&mut seen.weak, &views[i], &mut key) {
                2
            } else {
                1
            }
        } else if group.count == 1 && mostly_folded {
            // The rare line among folded noise is what the reader is after.
            2
        } else if context[first] || FRAME.is_match(&views[i]) {
            // A failure's own detail outranks the head and tail padding, which
            // would otherwise fill the budget and leave only its first line.
            2
        } else if nearby {
            1
        } else {
            0
        };
        let (a, b) = (base + first, base + last);
        let label = if count == 1 {
            format!("{}:{a}", record.source)
        } else if exact && last + 1 - first == count {
            format!("{}:{a}-{b} repeat={count}", record.source)
        } else if exact {
            format!("{}:{a} repeat={count} last={b}", record.source)
        } else {
            format!("{}:{a} similar={count} last={b}", record.source)
        };
        units.push(Unit::text_v3(
            record,
            label,
            views[i].clone(),
            lines[i],
            priority,
            limit,
            (count == 1).then_some(a),
        ));
        i = if folds[slot] { i + 1 } else { last + 1 };
    }
}

/// Compact-v3 selection: tiers are filled alternately from the head and the
/// tail so the final summary survives a flood of early diagnostics. When the
/// view carries diagnostics and cannot show every ordinary line anyway,
/// ordinary lines stop at a quarter of `budget`, so the view ends where the
/// evidence does instead of filling up with noise. `available` may exceed
/// `budget` on a second pass that spends range-block savings, which must not
/// raise that ceiling.
fn select_v3(units: &[Unit<'_>], available: usize, budget: usize) -> Vec<usize> {
    let mut selected = vec![false; units.len()];
    let mut used = 0;
    let diagnostics = units.iter().any(|u| matches!(u.priority, 2 | 3));
    for priority in [4, 3, 2, 1, 0] {
        let tier: Vec<usize> = (0..units.len())
            .filter(|&i| units[i].priority == priority)
            .collect();
        let mut order = Vec::with_capacity(tier.len());
        let (mut head, mut tail) = (0, tier.len());
        while head < tail {
            order.push(tier[head]);
            head += 1;
            if head < tail {
                tail -= 1;
                order.push(tier[tail]);
            }
        }
        let total: usize = tier.iter().map(|&i| units[i].size).sum();
        let cap = if priority == 0 && diagnostics && total > available.saturating_sub(used) {
            budget / ORDINARY_SHARE
        } else {
            available
        };
        let mut tier_used = 0;
        for i in order {
            let size = units[i].size;
            if size <= available.saturating_sub(used) && tier_used + size <= cap {
                selected[i] = true;
                used += size;
                tier_used += size;
            }
        }
    }
    selected
        .into_iter()
        .enumerate()
        .filter_map(|(i, yes)| yes.then_some(i))
        .collect()
}

fn consecutive(a: &Unit<'_>, b: &Unit<'_>) -> bool {
    match (a.block, b.block) {
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
                first.block.unwrap(),
                last.block.unwrap()
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

fn view(
    data: &Dataset,
    artifact: &str,
    budget: usize,
    version: Version,
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
                r.text.lines().count()
            })
        })
        .unwrap_or(usize::MAX);
    ensure!(
        count <= 100_000,
        "compact input exceeds 100000 units; use an explicit query"
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
        let mut selected = select_v3(&units, available, available);
        let mut body = String::new();
        render_v3(&units, &selected, &mut body);
        // Range blocks cost less than the per-line labels selection counted.
        // Spend that slack on more evidence, re-measuring the rendered bytes
        // each time and keeping only a pass that still fits the budget.
        let mut extra = 0;
        for _ in 0..REFILL_PASSES {
            let slack = available.saturating_sub(body.len());
            if slack == 0 {
                break;
            }
            let more = select_v3(&units, available + extra + slack, available);
            let mut extended = String::new();
            render_v3(&units, &more, &mut extended);
            if more.len() <= selected.len() || extended.len() > available {
                break;
            }
            extra += slack;
            selected = more;
            body = extended;
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
