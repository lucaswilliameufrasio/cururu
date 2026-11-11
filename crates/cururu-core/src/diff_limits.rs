use crate::ChangedFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewInputError {
    DiffExceedsMaximum,
    FileExceedsChunkLimit,
}

impl std::fmt::Display for ReviewInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DiffExceedsMaximum => {
                "filtered change-request diff exceeds review.max_diff_bytes; increase the limit or split the change"
            }
            Self::FileExceedsChunkLimit => {
                "a changed file exceeds review.chunk_bytes; increase the chunk size or split the change"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ReviewInputError {}

/// Rejects review inputs that would exceed configured diff or per-file limits.
pub fn ensure_review_diff_limits(
    files: &[ChangedFile],
    chunk_bytes: usize,
    max_diff_bytes: usize,
) -> Result<(), ReviewInputError> {
    let total_bytes = files
        .iter()
        .map(|file| file.patch.len())
        .fold(0usize, usize::saturating_add);
    if total_bytes > max_diff_bytes {
        return Err(ReviewInputError::DiffExceedsMaximum);
    }
    if files.iter().any(|file| file.patch.len() > chunk_bytes) {
        return Err(ReviewInputError::FileExceedsChunkLimit);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ReviewInputError, ensure_review_diff_limits};
    use crate::ChangedFile;

    #[test]
    fn rejects_review_diff_limits_with_existing_error_messages() {
        let files = [ChangedFile {
            path: "src/a.rs".into(),
            patch: "diff bytes".into(),
            right_lines: vec![1],
        }];

        assert_eq!(
            ensure_review_diff_limits(&files, 100, 5).unwrap_err(),
            ReviewInputError::DiffExceedsMaximum
        );
        assert!(
            ensure_review_diff_limits(&files, 5, 100)
                .unwrap_err()
                .to_string()
                .contains("review.chunk_bytes")
        );
    }
}
