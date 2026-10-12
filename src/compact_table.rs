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

/// Compact-v4 view of an object whose array of same-keyed objects holds more
/// than 80% of its bytes (`kubectl -o json`'s `items`): the rest of the
/// object on an `envelope` line, then the array as `table=/POINTER` with
/// constant columns factored out. Only when every row fits `limit`: a
/// partial table would hide what folding the source lines keeps visible.
pub(crate) fn envelope(value: &Value, limit: usize) -> Option<String> {
    let object = value.as_object()?;
    let total = encoding::size(value, usize::MAX)?;
    let (key, items) = object.iter().find_map(|(key, v)| {
        let items = v.as_array()?;
        (items.len() >= 2 && encoding::size(v, usize::MAX)? * 5 > total * 4).then_some((key, items))
    })?;
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
