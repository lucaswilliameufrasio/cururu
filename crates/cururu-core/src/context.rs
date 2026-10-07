use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Repository files and diagnostics selected as context for a review.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextStore {
    pub files: Vec<ContextFile>,
    pub truncated: Vec<String>,
    pub skipped: Vec<String>,
}

impl ContextStore {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextFile {
    pub label: String,
    pub path: String,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::{ContextFile, ContextStore};

    #[test]
    fn context_store_round_trips_with_diagnostics() {
        let store = ContextStore {
            files: vec![ContextFile {
                label: "Conventions".into(),
                path: "CONTRIBUTING.md".into(),
                content: "Check errors before returning.".into(),
            }],
            truncated: vec!["large.md".into()],
            skipped: vec!["missing.md".into()],
        };

        let json = serde_json::to_string(&store).unwrap();
        let decoded: ContextStore = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.files.len(), 1);
        assert_eq!(decoded.files[0].path, "CONTRIBUTING.md");
        assert_eq!(decoded.files[0].content, "Check errors before returning.");
        assert_eq!(decoded.truncated, ["large.md"]);
        assert_eq!(decoded.skipped, ["missing.md"]);
        assert!(!decoded.is_empty());
        assert!(
            ContextStore {
                files: Vec::new(),
                truncated: Vec::new(),
                skipped: Vec::new(),
            }
            .is_empty()
        );
    }
}
