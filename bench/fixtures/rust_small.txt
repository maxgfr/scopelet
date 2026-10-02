//! Compiled once per operation, shared by streaming scans and saved-source recovery.
use crate::model::Record;
use anyhow::{Result, ensure};
use regex::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};

pub(crate) struct Search {
    patterns: Vec<Regex>,
    set: Option<RegexSet>,
    all: bool,
    context: usize,
}
impl Search {
    pub(crate) fn new(patterns: &[String], all: bool, regex: bool, context: usize) -> Result<Self> {
        ensure!(
            !patterns.is_empty() && patterns.len() <= 64,
            "provide 1..64 search patterns"
        );
        ensure!(context <= 1000, "context must be <= 1000 lines");
        let strings: Vec<_> = patterns
            .iter()
            .map(|p| if regex { p.clone() } else { regex::escape(p) })
            .collect();
        let patterns = strings
            .iter()
            .map(|p| RegexBuilder::new(p).multi_line(true).crlf(true).build())
            .collect::<Result<Vec<_>, _>>()?;
        let set = if strings.len() > 1 {
            RegexSetBuilder::new(&strings)
                .multi_line(true)
                .crlf(true)
                .build()
                .ok()
        } else {
            None
        };
        Ok(Self {
            patterns,
            set,
            all,
            context,
        })
    }
    fn any(&self, text: &str) -> bool {
        self.set.as_ref().map_or_else(
            || self.patterns.iter().any(|p| p.is_match(text)),
            |set| set.is_match(text),
        )
    }
    pub(crate) fn apply(&self, records: Vec<Record>) -> Vec<Record> {
        let mut out = Vec::new();
        for mut r in records {
            let content = r.searchable_ref();
            let matched = if self.all {
                self.set.as_ref().map_or_else(
                    || self.patterns.iter().all(|p| p.is_match(&content)),
                    |set| set.matches(&content).len() == self.patterns.len(),
                )
            } else {
                self.any(&content)
            };
            if !matched {
                continue;
            }
            if r.value.is_some()
                || self
                    .patterns
                    .iter()
                    .any(|p| p.find_iter(&content).any(|m| m.as_str().contains('\n')))
            {
                out.push(r);
                continue;
            }
            let text = std::mem::take(&mut r.text);
            let lines: Vec<_> = text.split_inclusive('\n').collect();
            let mut spans: Vec<(usize, usize)> = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if self.any(line) {
                    let start = i.saturating_sub(self.context);
                    let end = (i + self.context + 1).min(lines.len());
                    if let Some(last) = spans.last_mut().filter(|last| start <= last.1) {
                        last.1 = last.1.max(end);
                    } else {
                        spans.push((start, end));
                    }
                }
            }
            if spans.is_empty() {
                r.text = text;
                out.push(r);
                continue;
            }
            for (start, end) in spans {
                let base = r.start_line.unwrap_or(1);
                out.push(Record {
                    text: lines[start..end].concat(),
                    start_line: Some(base + start),
                    end_line: Some(base + end - 1),
                    ..r.clone()
                });
            }
        }
        out
    }
}
