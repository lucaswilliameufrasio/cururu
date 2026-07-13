use globset::GlobSet;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffChunk {
    pub index: usize,
    pub text: String,
    pub files: Vec<String>,
}

pub fn parse_unified_diff(diff: &str) -> Vec<ChangedFile> {
    let header_re = Regex::new(r"(?m)^diff --git a/(.*?) b/(.*?)$").expect("valid regex");
    let mut files = Vec::new();
    let matches: Vec<_> = header_re.find_iter(diff).collect();

    for (idx, m) in matches.iter().enumerate() {
        let start = m.start();
        let end = matches.get(idx + 1).map_or(diff.len(), regex::Match::start);
        let patch = &diff[start..end];
        let path = header_re
            .captures(m.as_str())
            .and_then(|caps| caps.get(2))
            .map(|v| v.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        files.push(ChangedFile {
            path,
            patch: patch.to_string(),
        });
    }

    files
}

pub fn filter_ignored(files: Vec<ChangedFile>, ignore: &GlobSet) -> Vec<ChangedFile> {
    files
        .into_iter()
        .filter(|f| !ignore.is_match(&f.path))
        .collect()
}

pub fn chunk_files(
    files: &[ChangedFile],
    chunk_bytes: usize,
    max_diff_bytes: usize,
) -> Vec<DiffChunk> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_files = Vec::new();
    let mut total = 0usize;

    for file in files {
        let patch = if file.patch.len() > chunk_bytes {
            truncate_at_boundary(&file.patch, chunk_bytes)
        } else {
            file.patch.clone()
        };

        if total + patch.len() > max_diff_bytes {
            break;
        }

        if !current.is_empty() && current.len() + patch.len() > chunk_bytes {
            chunks.push(DiffChunk {
                index: chunks.len(),
                text: std::mem::take(&mut current),
                files: std::mem::take(&mut current_files),
            });
        }

        current.push_str(&patch);
        current.push('\n');
        current_files.push(file.path.clone());
        total += patch.len();
    }

    if !current.is_empty() {
        chunks.push(DiffChunk {
            index: chunks.len(),
            text: current,
            files: current_files,
        });
    }

    chunks
}

fn truncate_at_boundary(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[diff truncated by cururu]\n", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use globset::GlobSetBuilder;

    #[test]
    fn parses_multiple_files() {
        let diff = "diff --git a/a.rs b/a.rs\n+one\ndiff --git a/b.rs b/b.rs\n+two\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[1].path, "b.rs");
    }

    #[test]
    fn ignores_lockfiles() {
        let mut builder = GlobSetBuilder::new();
        builder.add(globset::Glob::new("**/Cargo.lock").unwrap());
        let set = builder.build().unwrap();
        let files = vec![ChangedFile {
            path: "Cargo.lock".into(),
            patch: "x".into(),
        }];
        assert!(filter_ignored(files, &set).is_empty());
    }
}
