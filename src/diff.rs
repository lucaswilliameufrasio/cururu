use globset::GlobSet;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
    /// Line numbers (1-based) present in the new (RIGHT) side of the diff.
    pub right_lines: Vec<u32>,
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
        let raw = &diff[start..end];
        let path = header_re
            .captures(m.as_str())
            .and_then(|caps| caps.get(2))
            .map_or_else(|| "unknown".to_string(), |v| v.as_str().to_string());
        let right_lines = new_file_right_lines(raw);
        files.push(ChangedFile {
            path,
            patch: raw.to_string(),
            right_lines,
        });
    }

    files
}

/// Track the right-side (new) line numbers covered by a unified diff.
///
/// Hunks look like:
///   @@ -<old>,<oldcount> +<new>,<newcount> @@
/// Every following line is either context (` `), addition (`+`), or deletion
/// (`-`). Deletions only advance the old line counter; context and additions
/// advance both. The new-side line number is recorded for every line that
/// exists in the new file (context and additions), so review comments can be
/// anchored to those lines.
fn new_file_right_lines(diff: &str) -> Vec<u32> {
    let hunk_re = Regex::new(r"(?m)^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@.*$").expect("valid regex");
    let mut lines = Vec::new();

    // Collect hunk header match ranges so each body can be sliced precisely.
    let hunks: Vec<_> = hunk_re.find_iter(diff).collect();
    for (idx, m) in hunks.iter().enumerate() {
        let hunk_new_start: u32 = hunk_re
            .captures(m.as_str())
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str().parse().ok())
            .unwrap_or(0);

        let body_start = m.end();
        let body_end = hunks.get(idx + 1).map_or(diff.len(), regex::Match::start);
        let body = &diff[body_start..body_end];

        let mut new_line = hunk_new_start;
        for line in body.lines() {
            let Some(first) = line.chars().next() else {
                continue;
            };
            match first {
                // Additions and context lines exist on the new side.
                '+' | ' ' => {
                    lines.push(new_line);
                    new_line += 1;
                }
                // Deletions and "\ No newline..." markers advance only old side.
                '-' | '\\' => {}
                _ => break,
            }
        }
    }

    lines
}

pub fn filter_ignored(files: Vec<ChangedFile>, ignore: &GlobSet) -> Vec<ChangedFile> {
    files
        .into_iter()
        .filter(|f| !ignore.is_match(&f.path))
        .collect()
}

/// Check whether a (path, line) pair is a valid anchor for a review comment:
/// the file must be in the diff and the line must exist on the right side.
pub fn is_valid_anchor(files: &[ChangedFile], path: &str, line: u32) -> bool {
    files
        .iter()
        .any(|f| f.path == path && f.right_lines.contains(&line))
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
            right_lines: vec![],
        }];
        assert!(filter_ignored(files, &set).is_empty());
    }

    #[test]
    fn tracks_added_and_context_lines() {
        // Hunk starts at new line 10. Three additions + one context line.
        let diff = "\
diff --git a/a.rs b/a.rs
@@ -1,0 +10,4 @@
+fn alpha() {}
+fn beta() {}
+fn gamma() {}
 fn kept() {}
";
        let files = parse_unified_diff(diff);
        assert_eq!(files[0].right_lines, vec![10, 11, 12, 13]);
    }

    #[test]
    fn skips_deletions_on_right_side() {
        let diff = "\
diff --git a/a.rs b/a.rs
@@ -5,3 +5,3 @@
-old_line
 fn ctx() {}
+fn added() {}
";
        let files = parse_unified_diff(diff);
        // Deletion does not advance the new side, so lines are 5 (ctx), 6 (added).
        assert_eq!(files[0].right_lines, vec![5, 6]);
    }

    #[test]
    fn validates_anchors() {
        let files = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
@@ -1,0 +1,1 @@
+fn main() {}
",
        );
        assert!(is_valid_anchor(&files, "a.rs", 1));
        assert!(!is_valid_anchor(&files, "a.rs", 2));
        assert!(!is_valid_anchor(&files, "other.rs", 1));
    }
}
