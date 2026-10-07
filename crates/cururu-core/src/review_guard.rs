/// Error returned when a review's analyzed head no longer matches the current head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewHeadChanged;

impl std::fmt::Display for ReviewHeadChanged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "PR head changed during review; any already-published findings remain anchored to the analyzed revision, and a fresh review is required",
        )
    }
}

impl std::error::Error for ReviewHeadChanged {}

/// Ensures the revision being published is still the one that was analyzed.
pub fn ensure_review_head_unchanged(
    expected: &str,
    current: &str,
) -> Result<(), ReviewHeadChanged> {
    if expected == current {
        Ok(())
    } else {
        Err(ReviewHeadChanged)
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_review_head_unchanged;

    #[test]
    fn accepts_the_analyzed_revision_and_rejects_a_changed_head() {
        assert!(ensure_review_head_unchanged("abc", "abc").is_ok());

        let error = ensure_review_head_unchanged("abc", "def").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("already-published findings remain anchored")
        );
    }
}
