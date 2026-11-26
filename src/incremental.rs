//! Incremental review by changed files (`policy.incremental = "changed_files"`).
//!
//! Every published summary records, in a hidden marker, the SHA-256 of each
//! file's patch on the PR diff that the review fully covered. On the next
//! push only the files whose patch changed (or that were not fully covered)
//! are sent to the model, each with its whole PR patch, so findings stay
//! anchored on the PR diff. Comparing patches instead of commits works across
//! force-pushes and rebases and ignores changes merged from the base branch.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Marker prefix in a Cururu summary comment.
const PREFIX: &str = "<!-- cururu:files:v1 ";
/// Hex characters kept of each patch hash (128 bits).
const HASH_CHARS: usize = 32;
/// Largest marker written. A larger state is not recorded, so the next
/// review is complete.
pub const MAX_MARKER_CHARS: usize = 32_000;

/// The files a published review fully covered, by path, with their patch
/// hashes, and the head it reviewed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewedFiles {
    pub head: String,
    pub patches: BTreeMap<String, String>,
}

/// What an incremental review sends again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Head of the review the comparison is based on.
    pub since_head: String,
    /// Files whose patch changed or that were not fully covered.
    pub changed: Vec<String>,
    /// Files with the same patch as in that review, not sent again.
    pub unchanged: Vec<String>,
}

fn short(hash: &str) -> &str {
    hash.get(..HASH_CHARS).unwrap_or(hash)
}

/// The hidden marker for `state`, or None when it would exceed
/// [`MAX_MARKER_CHARS`]. Every `-` in the JSON is written as `-`, so a
/// path cannot close the HTML comment.
pub fn render_marker(state: &ReviewedFiles) -> Option<String> {
    let patches: BTreeMap<&str, &str> = state
        .patches
        .iter()
        .map(|(path, hash)| (path.as_str(), short(hash)))
        .collect();
    let json = serde_json::json!({"head": state.head, "files": patches}).to_string();
    let mut marker = String::from(PREFIX);
    let _ = write!(marker, "{} -->", json.replace('-', "\\u002d"));
    (marker.len() <= MAX_MARKER_CHARS).then_some(marker)
}

/// The state recorded in a summary body, if it has a valid marker.
pub fn parse_marker(body: &str) -> Option<ReviewedFiles> {
    let start = body.find(PREFIX)? + PREFIX.len();
    let end = start + body[start..].find(" -->")?;
    let value: serde_json::Value = serde_json::from_str(&body[start..end]).ok()?;
    let head = value.get("head")?.as_str()?.to_owned();
    let patches = value
        .get("files")?
        .as_object()?
        .iter()
        .map(|(path, hash)| Some((path.clone(), hash.as_str()?.to_owned())))
        .collect::<Option<BTreeMap<_, _>>>()?;
    Some(ReviewedFiles { head, patches })
}

/// Split the current files (`current`: path to full patch hash) into those
/// to review again and those unchanged since `previous`.
pub fn select(previous: &ReviewedFiles, current: &BTreeMap<String, String>) -> Selection {
    let (unchanged, changed): (Vec<String>, Vec<String>) =
        current.keys().cloned().partition(|path| {
            previous
                .patches
                .get(path)
                .is_some_and(|hash| hash == short(&current[path]))
        });
    Selection {
        since_head: previous.head.clone(),
        changed,
        unchanged,
    }
}

/// The state to record after a review at `head`: every current file the
/// review covered, or that an earlier review covered with the same patch,
/// minus the files this review did not fully cover.
pub fn next_state(
    head: &str,
    current: &BTreeMap<String, String>,
    not_covered: &[String],
) -> ReviewedFiles {
    ReviewedFiles {
        head: head.to_owned(),
        patches: current
            .iter()
            .filter(|(path, _)| !not_covered.contains(path))
            .map(|(path, hash)| (path.clone(), short(hash).to_owned()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items
            .iter()
            .map(|(path, hash)| ((*path).to_owned(), hash.repeat(64)))
            .collect()
    }

    #[test]
    fn marker_round_trips_and_cannot_be_closed_by_a_path() {
        let state = next_state("abc", &hashes(&[("a-->b.rs", "1"), ("src/x.rs", "2")]), &[]);
        let marker = render_marker(&state).unwrap();
        assert!(!marker[PREFIX.len()..marker.len() - 4].contains("--"));
        let body = format!("summary\n{marker}\nmore");
        assert_eq!(parse_marker(&body), Some(state));
        assert_eq!(parse_marker("no marker"), None);
        assert_eq!(parse_marker("<!-- cururu:files:v1 {bad -->"), None);
    }

    #[test]
    fn only_changed_or_uncovered_files_are_selected() {
        let previous = next_state(
            "old",
            &hashes(&[("a", "1"), ("b", "2"), ("c", "3")]),
            &["c".into()],
        );
        let current = hashes(&[("a", "1"), ("b", "9"), ("c", "3"), ("d", "4")]);
        let selection = select(&previous, &current);
        assert_eq!(selection.since_head, "old");
        assert_eq!(selection.unchanged, ["a"]);
        // b changed, c was not covered last time, d is new.
        assert_eq!(selection.changed, ["b", "c", "d"]);
    }

    #[test]
    fn an_oversized_state_is_not_recorded() {
        let many: BTreeMap<String, String> = (0..2_000)
            .map(|index| (format!("src/module_{index:05}.rs"), "f".repeat(64)))
            .collect();
        assert!(render_marker(&next_state("h", &many, &[])).is_none());
    }
}
