//! Borrowed table encoding; unsuccessful table attempts never allocate all rows.
use crate::{encoding, model::*};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use serde_json::Value;
use std::collections::BTreeMap;

/// A record's values for the given columns, in column order.
struct Row<'a>(&'a Record, &'a [&'a str]);
impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let object = self.0.value.as_ref().unwrap().as_object().unwrap();
        let mut seq = serializer.serialize_seq(Some(self.1.len()))?;
        for key in self.1 {
            seq.serialize_element(&object[*key])?;
        }
        seq.end()
    }
}
struct Rows<'a> {
    data: &'a Dataset,
    indices: &'a [usize],
    columns: &'a [&'a str],
}
impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.indices.len()))?;
        for &i in self.indices {
            seq.serialize_element(&Row(&self.data.records[i], self.columns))?;
        }
        seq.end()
    }
}

#[derive(Serialize)]
struct Table<'a> {
    columns: &'a [&'a str],
    /// Compact-v4: columns whose value is the same in every record, once.
    #[serde(skip_serializing_if = "Option::is_none")]
    constant: Option<BTreeMap<&'a str, &'a Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    indices: Option<&'a [usize]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record_sources: Option<Vec<&'a str>>,
    rows: Rows<'a>,
    source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_records: Option<usize>,
}

pub(crate) fn uniform(data: &Dataset) -> bool {
    let Some(first) = data.records.first() else {
        return false;
    };
    let Some(object) = first.value.as_ref().and_then(|v| v.as_object()) else {
        return false;
    };
    data.records.len() >= 2
        && data.records.iter().all(|r| {
            r.blob == first.blob
                && (r.blob.is_some() || r.source == first.source)
                && r.value
                    .as_ref()
                    .and_then(|v| v.as_object())
                    .is_some_and(|v| v.keys().eq(object.keys()))
        })
}

/// The columns of a uniform dataset's table and, with `constants`, the
/// keys whose value is identical in every record, which leave the columns.
pub(crate) fn columns(data: &Dataset, constants: bool) -> (Vec<&str>, Vec<&str>) {
    let Some(object) = data
        .records
        .first()
        .and_then(|r| r.value.as_ref()?.as_object())
    else {
        return (Vec::new(), Vec::new());
    };
    fn value<'r>(record: &'r Record, key: &str) -> &'r Value {
        &record.value.as_ref().unwrap()[key]
    }
    object.keys().map(String::as_str).partition(|key| {
        !(constants
            && data.records.len() >= 2
            && data
                .records
                .iter()
                .all(|r| value(r, key) == value(&data.records[0], key)))
    })
}

/// A `table` line of the records at `indices`; `v2` adds their indices and
/// the total, `constants` moves constant columns to `constant`.
pub(crate) fn encode_with(
    data: &Dataset,
    indices: &[usize],
    v2: bool,
    constants: bool,
    limit: usize,
) -> Option<String> {
    let first = data.records.first()?;
    let object = first.value.as_ref()?.as_object()?;
    let (columns, fixed) = columns(data, constants);
    let table = Table {
        columns: &columns,
        constant: (!fixed.is_empty())
            .then(|| fixed.iter().map(|&key| (key, &object[key])).collect()),
        indices: v2.then_some(indices),
        record_sources: data
            .records
            .iter()
            .any(|r| r.source != first.source)
            .then(|| {
                indices
                    .iter()
                    .map(|&i| data.records[i].source.as_str())
                    .collect()
            }),
        rows: Rows {
            data,
            indices,
            columns: &columns,
        },
        source: &first.source,
        total_records: v2.then_some(data.records.len()),
    };
    encoding::size(&table, limit.saturating_sub(7))?;
    Some(format!("table {}\n", serde_json::to_string(&table).ok()?))
}

pub(crate) fn row_size(record: &Record, columns: &[&str], limit: usize) -> Option<usize> {
    encoding::size(&Row(record, columns), limit)
}

/// The field of an object holding an array of two or more objects that
/// takes more than 80% of the object's bytes.
fn dominant(value: &Value) -> Option<(&String, &Vec<Value>)> {
    let object = value.as_object()?;
    let total = encoding::size(value, usize::MAX)?;
    object.iter().find_map(|(key, v)| {
        let items = v.as_array()?;
        (items.len() >= 2
            && items.iter().all(Value::is_object)
            && encoding::size(v, usize::MAX)? * 5 > total * 4)
            .then_some((key, items))
    })
}

/// Longest string a projection shows in a column; longer text is a body,
/// not a field to scan.
const PROJECTED_STRING: usize = 120;

/// A projected view: its text, the rows shown, the rows in all, and
/// whether fields were left out of the rows shown.
pub(crate) struct Projection {
    pub text: String,
    pub shown: usize,
    pub total: usize,
    /// Units shown in part: every row shown when columns were left out,
    /// and the envelope when its nested fields were.
    pub abridged: usize,
}

/// Compact-v4 view of minified JSON too large to show whole: objects (a
/// top-level array, or the dominant array of an object) as a table of the
/// short scalar fields every one of them has, the rest named under
/// `omitted_columns`, as many rows as fit. Values are exact; no field is
/// invented.
/// `important` marks the objects shown first (a diagnostic, an anchor); the
/// others follow in order, and the table keeps source order.
pub(crate) fn projection(
    data: &Dataset,
    limit: usize,
    important: &dyn Fn(&Value) -> bool,
) -> Option<Projection> {
    let (envelope, pointer, items): (
        Option<serde_json::Map<String, Value>>,
        Option<String>,
        Vec<&Value>,
    ) = match data.records.as_slice() {
        [record] => {
            let value = record.value.as_ref()?;
            let (key, items) = dominant(value)?;
            let rest: serde_json::Map<String, Value> = value
                .as_object()?
                .iter()
                .filter(|(k, _)| *k != key)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (
                Some(rest),
                Some(key.replace('~', "~0").replace('/', "~1")),
                items.iter().collect(),
            )
        }
        records if records.len() >= 2 => (
            None,
            None,
            records
                .iter()
                .map(|r| r.value.as_ref().filter(|v| v.is_object()))
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    };
    let objects: Vec<&serde_json::Map<String, Value>> =
        items.iter().map(|v| v.as_object()).collect::<Option<_>>()?;
    // Every field of any object: one that some objects lack is named, never
    // shown as a gap or a null.
    let every: std::collections::BTreeSet<&str> = objects
        .iter()
        .flat_map(|o| o.keys().map(String::as_str))
        .collect();
    let mut keys: Vec<&str> = every.iter().copied().collect();
    keys.retain(|k| objects.iter().all(|o| o.contains_key(*k)));
    let short = |v: &Value| match v {
        Value::String(s) => s.len() <= PROJECTED_STRING,
        Value::Array(_) | Value::Object(_) => false,
        _ => true,
    };
    let (mut kept, mut omitted): (Vec<&str>, Vec<&str>) = keys
        .iter()
        .partition(|k| objects.iter().all(|o| short(&o[**k])));
    // API responses repeat links to every related resource: one link column
    // (`html_url`, else `url`, else the first) is kept, the others named.
    let link = |k: &&str| {
        objects.iter().all(|o| {
            o[*k]
                .as_str()
                .is_some_and(|s| s.starts_with("https://") || s.starts_with("http://"))
        })
    };
    let links: Vec<&str> = kept.iter().copied().filter(link).collect();
    if links.len() > 1 {
        let keep = ["html_url", "url"]
            .into_iter()
            .find(|k| links.contains(k))
            .unwrap_or(links[0]);
        kept.retain(|k| *k == keep || !links.contains(k));
        omitted.extend(links.iter().filter(|k| **k != keep));
        omitted.sort_unstable();
    }
    omitted.extend(every.iter().copied().filter(|k| !keys.contains(k)));
    omitted.sort_unstable();
    let (columns, fixed): (Vec<&str>, Vec<&str>) = kept
        .iter()
        .partition(|k| objects.iter().any(|o| o[**k] != objects[0][**k]));
    if columns.is_empty() {
        return None;
    }
    #[derive(Serialize)]
    struct Projected<'a> {
        columns: &'a [&'a str],
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        constant: BTreeMap<&'a str, &'a Value>,
        #[serde(skip_serializing_if = "<[_]>::is_empty")]
        omitted_columns: &'a [&'a str],
        rows: Vec<Vec<&'a Value>>,
    }
    // The envelope keeps its scalars; its nested fields are named.
    let mut head = String::new();
    let mut envelope_cut = false;
    if let Some(rest) = &envelope {
        let (scalars, nested): (
            serde_json::Map<String, Value>,
            serde_json::Map<String, Value>,
        ) = rest
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .partition(|(_, v)| !v.is_object() && !v.is_array());
        head = format!("envelope {}", serde_json::to_string(&scalars).ok()?);
        if !nested.is_empty() {
            envelope_cut = true;
            let names: Vec<&String> = nested.keys().collect();
            head.push_str(&format!(
                " omitted_fields={}",
                serde_json::to_string(&names).ok()?
            ));
        }
        head.push('\n');
    }
    let label = match &pointer {
        Some(pointer) => format!("table=/{pointer} "),
        None => "table ".into(),
    };
    let mut table = Projected {
        columns: &columns,
        constant: fixed.iter().map(|k| (*k, &objects[0][*k])).collect(),
        omitted_columns: &omitted,
        rows: Vec::new(),
    };
    let fixed_size = head.len() + label.len() + encoding::size(&table, usize::MAX)? + 1;
    let mut size = fixed_size;
    let order = (0..objects.len())
        .filter(|&i| important(items[i]))
        .chain((0..objects.len()).filter(|&i| !important(items[i])));
    let mut chosen = Vec::new();
    for i in order {
        let row: Vec<&Value> = columns.iter().map(|k| &objects[i][*k]).collect();
        let row_size = encoding::size(&row, usize::MAX)? + 1;
        if size + row_size > limit {
            continue;
        }
        size += row_size;
        chosen.push(i);
    }
    if chosen.is_empty() {
        return None;
    }
    chosen.sort_unstable();
    table.rows = chosen
        .iter()
        .map(|&i| columns.iter().map(|k| &objects[i][*k]).collect())
        .collect();
    let text = format!("{head}{label}{}\n", serde_json::to_string(&table).ok()?);
    (text.len() <= limit).then_some(Projection {
        shown: table.rows.len(),
        total: objects.len(),
        abridged: if omitted.is_empty() { 0 } else { chosen.len() } + usize::from(envelope_cut),
        text,
    })
}

/// Compact-v4 view of an object whose array of same-keyed objects holds more
/// than 80% of its bytes (`kubectl -o json`'s `items`): the rest of the
/// object on an `envelope` line, then the array as `table=/POINTER` with
/// constant columns factored out. Only when every row fits `limit`: a
/// partial table would hide what folding the source lines keeps visible.
pub(crate) fn envelope(value: &Value, limit: usize) -> Option<String> {
    let object = value.as_object()?;
    let (key, items) = dominant(value)?;
    let first = items[0].as_object()?;
    if !items
        .iter()
        .all(|item| item.as_object().is_some_and(|o| o.keys().eq(first.keys())))
    {
        return None;
    }
    let rows = Dataset {
        records: items
            .iter()
            .map(|item| Record {
                value: Some(item.clone()),
                ..Record::derived(Value::Null)
            })
            .map(|record| Record {
                source: "input".into(),
                ..record
            })
            .collect(),
        ..Dataset::default()
    };
    let rest: serde_json::Map<String, Value> = object
        .iter()
        .filter(|(k, _)| *k != key)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let envelope = format!("envelope {}\n", serde_json::to_string(&rest).ok()?);
    let indices: Vec<usize> = (0..items.len()).collect();
    let table = encode_with(
        &rows,
        &indices,
        false,
        true,
        limit.checked_sub(envelope.len() + key.len() + 2)?,
    )?;
    let pointer = key.replace('~', "~0").replace('/', "~1");
    let text = format!(
        "{envelope}table=/{pointer} {}",
        table.strip_prefix("table ")?
    );
    (text.len() <= limit).then_some(text)
}
