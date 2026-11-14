use crate::{ChangedFile, DiffChunk};

/// Check whether a (path, line) pair is a valid anchor for a review comment.
#[must_use]
pub fn is_valid_anchor(files: &[ChangedFile], path: &str, line: u32) -> bool {
    files
        .iter()
        .any(|file| file.path == path && file.right_lines.contains(&line))
}

/// Split changed files into bounded diff chunks using the existing Cururu limits.
#[must_use]
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
        // A file larger than a chunk is reviewed in several chunks, each a
        // valid patch; only a single line longer than a chunk is truncated.
        let pieces = if file.patch.len() > chunk_bytes {
            split_patch(&file.patch, chunk_bytes)
        } else {
            vec![file.patch.clone()]
        };
        for piece in pieces {
            let patch = if piece.len() > chunk_bytes {
                truncate_at_boundary(&piece, chunk_bytes)
            } else {
                piece
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
            if current_files.last() != Some(&file.path) {
                current_files.push(file.path.clone());
            }
            total += patch.len();
        }
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

/// Split one file patch into pieces of at most `max` bytes where possible.
/// Each piece repeats the file header and holds whole hunks; a hunk larger
/// than the budget becomes sub-hunks with recalculated `@@` line ranges, so
/// line numbers stay correct in every piece.
#[must_use]
pub fn split_patch(patch: &str, max: usize) -> Vec<String> {
    let lines: Vec<&str> = patch.split_inclusive('\n').collect();
    let first_hunk = lines
        .iter()
        .position(|line| line.starts_with("@@ "))
        .unwrap_or(lines.len());
    let header: String = lines[..first_hunk].concat();
    let budget = max.saturating_sub(header.len()).max(1);

    let mut units: Vec<String> = Vec::new();
    let mut hunk_start = first_hunk;
    while hunk_start < lines.len() {
        let hunk_end = lines[hunk_start + 1..]
            .iter()
            .position(|line| line.starts_with("@@ "))
            .map_or(lines.len(), |offset| hunk_start + 1 + offset);
        let hunk = &lines[hunk_start..hunk_end];
        if hunk.concat().len() <= budget {
            units.push(hunk.concat());
        } else {
            units.extend(split_hunk(hunk, budget));
        }
        hunk_start = hunk_end;
    }
    if units.is_empty() {
        return vec![patch.to_string()];
    }

    let mut pieces = Vec::new();
    let mut current = header.clone();
    for unit in units {
        if current.len() > header.len() && current.len() + unit.len() > max {
            pieces.push(std::mem::replace(&mut current, header.clone()));
        }
        current.push_str(&unit);
    }
    pieces.push(current);
    pieces
}

fn hunk_range(header: &str) -> Option<(u32, u32)> {
    let rest = header.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let new = rest.split_once(" @@")?.0;
    let start = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

/// Split one hunk into sub-hunks of about `budget` bytes with correct ranges.
fn split_hunk(hunk: &[&str], budget: usize) -> Vec<String> {
    let Some((mut old_line, mut new_line)) = hunk.first().and_then(|line| hunk_range(line)) else {
        return vec![hunk.concat()];
    };
    let mut parts = Vec::new();
    let mut body = String::new();
    let (mut old_start, mut new_start) = (old_line, new_line);
    let (mut old_count, mut new_count) = (0u32, 0u32);
    let flush = |parts: &mut Vec<String>, body: &mut String, ranges: (u32, u32, u32, u32)| {
        if !body.is_empty() {
            let (old_start, old_count, new_start, new_count) = ranges;
            parts.push(format!(
                "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n{}",
                std::mem::take(body)
            ));
        }
    };
    for line in &hunk[1..] {
        if !body.is_empty() && body.len() + line.len() + 32 > budget {
            flush(
                &mut parts,
                &mut body,
                (old_start, old_count, new_start, new_count),
            );
            (old_start, new_start, old_count, new_count) = (old_line, new_line, 0, 0);
        }
        body.push_str(line);
        match line.as_bytes().first() {
            Some(b' ') => {
                old_line += 1;
                new_line += 1;
                old_count += 1;
                new_count += 1;
            }
            Some(b'-') => {
                old_line += 1;
                old_count += 1;
            }
            Some(b'+') => {
                new_line += 1;
                new_count += 1;
            }
            _ => {}
        }
    }
    flush(
        &mut parts,
        &mut body,
        (old_start, old_count, new_start, new_count),
    );
    parts
}

fn truncate_at_boundary(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[diff truncated by cururu]\n", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::{chunk_files, is_valid_anchor, split_patch};
    use crate::{ChangedFile, DiffChunk};
    use std::fmt::Write as _;

    fn changed_file(file_path: &str, patch_text: &str, right_lines: &[u32]) -> ChangedFile {
        ChangedFile {
            path: file_path.into(),
            patch: patch_text.into(),
            right_lines: right_lines.to_vec(),
        }
    }

    #[test]
    fn chunking_preserves_file_order_indexes_and_max_diff_limit() {
        let files = [
            changed_file("a.rs", "1234", &[1]),
            changed_file("b.rs", "5678", &[1]),
            changed_file("c.rs", "9abc", &[1]),
        ];

        assert_eq!(
            chunk_files(&files, 8, 8),
            [
                DiffChunk {
                    index: 0,
                    text: "1234\n".into(),
                    files: vec!["a.rs".into()],
                },
                DiffChunk {
                    index: 1,
                    text: "5678\n".into(),
                    files: vec!["b.rs".into()],
                },
            ]
        );
    }

    #[test]
    fn large_files_are_split_into_valid_patches_instead_of_truncated() {
        let header = "diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n";
        let mut patch = String::from(header);
        // Two hunks; the second is larger than the chunk on its own.
        patch.push_str("@@ -1,2 +1,2 @@\n-old\n+new\n");
        patch.push_str("@@ -10,0 +10,40 @@\n");
        for n in 0..40 {
            let _ = writeln!(patch, "+line {n:02}");
        }
        let pieces = split_patch(&patch, 200);
        assert!(pieces.len() > 2, "{pieces:?}");
        for piece in &pieces {
            assert!(piece.starts_with(header));
            assert!(piece.len() <= 200 + 40, "{}", piece.len());
        }
        // Every added line is kept exactly once, and ranges stay correct.
        let joined = pieces.join("");
        for n in 0..40 {
            assert_eq!(joined.matches(&format!("+line {n:02}\n")).count(), 1);
        }
        let files = [ChangedFile {
            path: "big.rs".into(),
            patch: patch.clone(),
            right_lines: vec![],
        }];
        let rebuilt: Vec<u32> = pieces
            .iter()
            .flat_map(|piece| crate::parse_unified_diff(piece))
            .flat_map(|file| file.right_lines)
            .collect();
        assert_eq!(rebuilt, crate::parse_unified_diff(&patch)[0].right_lines);
        let chunks = chunk_files(&files, 200, 100_000);
        assert!(chunks.len() > 2);
        assert!(chunks.iter().all(|chunk| chunk.files == ["big.rs"]));
        assert!(
            !chunks
                .iter()
                .any(|chunk| chunk.text.contains("[diff truncated"))
        );
    }

    #[test]
    fn anchor_validation_requires_matching_path_and_right_side_line() {
        let files = [changed_file("a.rs", "patch", &[4, 5])];

        assert!(is_valid_anchor(&files, "a.rs", 4));
        assert!(!is_valid_anchor(&files, "a.rs", 6));
        assert!(!is_valid_anchor(&files, "b.rs", 4));
    }
}
