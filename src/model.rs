use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, clap::ValueEnum, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Default,
    Ultra,
}

impl Mode {
    pub fn budget(self) -> usize {
        match self {
            Self::Default => 16 * 1024,
            Self::Ultra => 4 * 1024,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub source: Source,
    #[serde(default)]
    pub operations: Vec<Operation>,
    #[serde(default)]
    pub mode: Mode,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Repo {
        path: String,
        #[serde(default)]
        include: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
    },
    File {
        path: String,
        #[serde(default)]
        format: Format,
    },
    Code {
        path: String,
        #[serde(default)]
        symbol: Option<String>,
        #[serde(default)]
        relation: CodeRelation,
    },
    Url {
        url: String,
    },
    Document {
        path: String,
    },
    Artifact {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Text,
    Json,
    Jsonl,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeRelation {
    #[default]
    Definitions,
    Callers,
    Impact,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Search {
        patterns: Vec<String>,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        regex: bool,
        #[serde(default = "default_context")]
        context: usize,
    },
    Filter {
        pointer: String,
        equals: Value,
    },
    Project {
        pointers: Vec<String>,
    },
    Count,
    Group {
        pointer: String,
    },
    Unique,
    Read {
        start: usize,
        end: usize,
    },
    Rank {
        query: String,
    },
}

fn default_context() -> usize {
    3
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Record {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted_lines: Option<usize>,
}

impl Record {
    pub fn derived(value: Value) -> Self {
        Self {
            source: "computed".into(),
            blob: None,
            start_line: None,
            end_line: None,
            text: String::new(),
            value: Some(value),
            omitted_lines: None,
        }
    }

    pub fn searchable(&self) -> String {
        match &self.value {
            Some(value) => value.to_string(),
            None => self.text.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub source: String,
    pub blob: String,
    pub bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Dataset {
    pub schema_version: u32,
    pub scan_complete: bool,
    pub examined: usize,
    pub skipped: BTreeMap<String, usize>,
    pub notes: Vec<String>,
    pub snapshots: Vec<Snapshot>,
    pub records: Vec<Record>,
}

impl Default for Dataset {
    fn default() -> Self {
        Self {
            schema_version: 1,
            scan_complete: true,
            examined: 0,
            skipped: BTreeMap::new(),
            notes: Vec::new(),
            snapshots: Vec::new(),
            records: Vec::new(),
        }
    }
}

impl Dataset {
    pub fn skip(&mut self, reason: &str) {
        *self.skipped.entry(reason.into()).or_default() += 1;
    }

    pub fn incomplete(&mut self, note: String) {
        self.scan_complete = false;
        // The full record of omissions lives in snapshots and skipped counts.
        if self.notes.len() < 16 {
            self.notes.push(note);
        }
    }
}
