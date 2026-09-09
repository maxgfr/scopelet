//! Borrowed table encoding; unsuccessful table attempts never allocate all rows.
use crate::{encoding, model::*};
use serde::{Serialize, Serializer, ser::SerializeSeq};

struct Row<'a>(&'a Record);
impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let object = self.0.value.as_ref().unwrap().as_object().unwrap();
        let mut seq = serializer.serialize_seq(Some(object.len()))?;
        for value in object.values() {
            seq.serialize_element(value)?;
        }
        seq.end()
    }
}
struct Rows<'a> {
    data: &'a Dataset,
    indices: &'a [usize],
}
impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.indices.len()))?;
        for &i in self.indices {
            seq.serialize_element(&Row(&self.data.records[i]))?;
        }
        seq.end()
    }
}

#[derive(Serialize)]
struct Table<'a> {
    columns: Vec<&'a str>,
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

pub(crate) fn encode(data: &Dataset, indices: &[usize], v2: bool, limit: usize) -> Option<String> {
    let first = data.records.first()?;
    let object = first.value.as_ref()?.as_object()?;
    let table = Table {
        columns: object.keys().map(String::as_str).collect(),
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
        rows: Rows { data, indices },
        source: &first.source,
        total_records: v2.then_some(data.records.len()),
    };
    encoding::size(&table, limit.saturating_sub(7))?;
    Some(format!("table {}\n", serde_json::to_string(&table).ok()?))
}

pub(crate) fn row_size(record: &Record, limit: usize) -> Option<usize> {
    encoding::size(&Row(record), limit)
}
