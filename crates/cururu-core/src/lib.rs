//! Provider- and UI-independent domain contracts shared by Cururu applications.
//!
//! This first extraction intentionally contains only stable serialized review
//! data types. Provider adapters, orchestration, SCM, configuration loading,
//! and desktop UI remain outside this crate.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod evaluation;
mod quality;
mod scm_types;
mod severity;

pub use evaluation::{
    EvaluatedFinding, EvaluationMode, EvaluationReport, EvaluationUsage, FindingJudgment,
    SeverityJudgment,
};
pub use quality::{QualityReport, evaluate_quality};
pub use scm_types::{
    FindingAnnotation, PriorReviewComment, PriorReviewFeedback, ReviewCommentDraft,
};
pub use severity::{FailOn, Severity};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub model: String,
    pub files_reviewed: usize,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewFinding {
    pub severity: String,
    pub path: String,
    pub line: Option<u32>,
    pub title: String,
    pub message: String,
    pub suggestion: String,
    pub confidence: f32,
    #[serde(default)]
    pub suggested_change: Option<SuggestedChange>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub rule: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SuggestedChange {
    pub replacement: String,
}

/// A changed repository file with its unified patch and valid new-side lines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
    /// Line numbers (1-based) present in the new (right) side of the diff.
    pub right_lines: Vec<u32>,
}

/// A bounded piece of a diff submitted for review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DiffChunk {
    pub index: usize,
    pub text: String,
    pub files: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SuggestedChangeInput {
    Structured { replacement: String },
    Legacy(String),
}

impl<'de> Deserialize<'de> for SuggestedChange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match SuggestedChangeInput::deserialize(deserializer)? {
            SuggestedChangeInput::Structured { replacement }
            | SuggestedChangeInput::Legacy(replacement) => Self { replacement },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ChangedFile, DiffChunk, SuggestedChange};

    #[test]
    fn suggested_change_deserializes_structured_and_legacy_values() {
        let structured: SuggestedChange =
            serde_json::from_str(r#"{"replacement":"new code"}"#).unwrap();
        let legacy: SuggestedChange = serde_json::from_str(r#""new code""#).unwrap();
        assert_eq!(structured.replacement, legacy.replacement);
    }

    #[test]
    fn diff_domain_values_round_trip_through_json() {
        let file = ChangedFile {
            path: "src/lib.rs".into(),
            patch: "@@ -1 +1 @@".into(),
            right_lines: vec![1],
        };
        let chunk = DiffChunk {
            index: 0,
            text: file.patch.clone(),
            files: vec![file.path.clone()],
        };

        assert_eq!(
            serde_json::from_str::<ChangedFile>(&serde_json::to_string(&file).unwrap()).unwrap(),
            file
        );
        assert_eq!(
            serde_json::from_str::<DiffChunk>(&serde_json::to_string(&chunk).unwrap()).unwrap(),
            chunk
        );
    }
}
