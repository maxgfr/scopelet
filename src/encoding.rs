//! Byte accounting uses the serializer itself, including escapes and exact numbers.
use serde::Serialize;
use std::io::{self, Write};

pub(crate) struct Counter {
    pub len: usize,
    pub limit: usize,
}

impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.len) {
            return Err(io::Error::other("encoded value exceeds budget"));
        }
        self.len += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn size(value: &impl Serialize, limit: usize) -> Option<usize> {
    let mut counter = Counter { len: 0, limit };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.len)
}
