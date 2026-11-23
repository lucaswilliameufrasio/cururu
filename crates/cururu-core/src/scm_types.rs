use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A comment Cururu intends to publish at a changed source location.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ReviewCommentDraft {
    pub path: String,
    pub line: Option<u32>,
    pub body: String,
}

/// Earlier review feedback grouped by its source location.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct PriorReviewFeedback {
    pub location: String,
    pub comments: Vec<PriorReviewComment>,
    /// True for a resolved thread kept because a person replied to it: a
    /// decision recorded by the author or a reviewer, sent in compact form
    /// (the finding's title line and the latest human reply).
    #[serde(default)]
    pub resolved: bool,
    /// The finding a resolved thread decided, for deterministic matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<DecidedFinding>,
}

/// The finding of a resolved review thread with a human reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DecidedFinding {
    pub path: String,
    /// The thread's line on the current diff, when it is anchored to one.
    #[serde(default)]
    pub line: Option<u32>,
    /// The finding's title, without its severity.
    pub title: String,
    /// True when the commented code changed after the thread (the host marks
    /// the thread outdated), so the decision may no longer apply.
    pub code_changed: bool,
}

/// A single comment included in prior review feedback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct PriorReviewComment {
    pub author: String,
    pub body: String,
}

/// A tool-produced annotation associated with a source location.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct FindingAnnotation {
    pub path: String,
    pub line: Option<u32>,
    pub severity: String,
    pub title: Option<String>,
    pub message: String,
    pub details: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        DecidedFinding, FindingAnnotation, PriorReviewComment, PriorReviewFeedback,
        ReviewCommentDraft,
    };

    #[test]
    fn scm_review_values_round_trip_through_json() {
        let values = (
            ReviewCommentDraft {
                path: "src/lib.rs".into(),
                line: Some(8),
                body: "Consider handling this case".into(),
            },
            PriorReviewFeedback {
                location: "src/lib.rs:8".into(),
                comments: vec![PriorReviewComment {
                    author: "reviewer".into(),
                    body: "Please add a test".into(),
                }],
                resolved: true,
                decided: Some(DecidedFinding {
                    path: "src/lib.rs".into(),
                    line: Some(8),
                    title: "Missing test".into(),
                    code_changed: false,
                }),
            },
            FindingAnnotation {
                path: "src/lib.rs".into(),
                line: Some(8),
                severity: "medium".into(),
                title: Some("Missing case".into()),
                message: "This branch is not handled".into(),
                details: None,
            },
        );

        let json = serde_json::to_string(&values).unwrap();
        // Feedback written before `resolved` and `decided` still reads.
        let legacy: PriorReviewFeedback =
            serde_json::from_str(r#"{"location":"a","comments":[]}"#).unwrap();
        assert!(!legacy.resolved && legacy.decided.is_none());
        assert_eq!(
            serde_json::from_str::<(ReviewCommentDraft, PriorReviewFeedback, FindingAnnotation,)>(
                &json
            )
            .unwrap(),
            values
        );
    }
}
