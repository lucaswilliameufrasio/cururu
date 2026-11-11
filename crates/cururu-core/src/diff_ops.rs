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
    use super::{chunk_files, is_valid_anchor};
    use crate::{ChangedFile, DiffChunk};

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
    fn anchor_validation_requires_matching_path_and_right_side_line() {
        let files = [changed_file("a.rs", "patch", &[4, 5])];

        assert!(is_valid_anchor(&files, "a.rs", 4));
        assert!(!is_valid_anchor(&files, "a.rs", 6));
        assert!(!is_valid_anchor(&files, "b.rs", 4));
    }
}
