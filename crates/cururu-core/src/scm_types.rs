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
    use super::{FindingAnnotation, PriorReviewComment, PriorReviewFeedback, ReviewCommentDraft};

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
        assert_eq!(
            serde_json::from_str::<(ReviewCommentDraft, PriorReviewFeedback, FindingAnnotation,)>(
                &json
            )
            .unwrap(),
            values
        );
    }
}
