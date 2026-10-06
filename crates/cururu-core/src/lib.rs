//! Provider- and UI-independent domain contracts shared by Cururu applications.
//!
//! This first extraction intentionally contains only stable serialized review
//! data types. Provider adapters, orchestration, SCM, configuration loading,
//! and desktop UI remain outside this crate.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    use super::SuggestedChange;

    #[test]
    fn suggested_change_deserializes_structured_and_legacy_values() {
        let structured: SuggestedChange =
            serde_json::from_str(r#"{"replacement":"new code"}"#).unwrap();
        let legacy: SuggestedChange = serde_json::from_str(r#""new code""#).unwrap();
        assert_eq!(structured.replacement, legacy.replacement);
    }
}
