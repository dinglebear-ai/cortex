//! Recover original structured records in one bounded scan per source file.
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(super) fn recover<'a>(
    rows: impl Iterator<Item = (i64, Option<&'a str>, Option<&'a str>)>,
) -> HashMap<i64, String> {
    let mut wanted: HashMap<&str, HashSet<usize>> = HashMap::new();
    let mut sources = Vec::new();
    for (id, path, metadata) in rows {
        let Some(path) = path else { continue };
        let line = metadata
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.get("line_no").and_then(|v| v.as_u64()))
            .and_then(|n| usize::try_from(n).ok());
        if let Some(line) = line {
            wanted.entry(path).or_default().insert(line);
            sources.push((id, path, line));
        }
    }
    let mut recovered = HashMap::new();
    for (path, lines) in wanted {
        if let Ok(records) = crate::scanner::read_transcript_lines(Path::new(path), &lines) {
            for (id, source, line) in &sources {
                if *source == path
                    && let Some(record) = records.get(line)
                {
                    recovered.insert(*id, record.clone());
                }
            }
        }
    }
    recovered
}
