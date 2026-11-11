use globset::{Glob, GlobSetBuilder};
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

/// Match a repository-relative path against an exact context path or glob.
///
/// Patterns without glob metacharacters retain exact-match semantics. Invalid
/// glob patterns do not match, preserving the application's existing behavior.
#[must_use]
pub fn context_path_matches(path: &str, pattern: &str) -> bool {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        let glob = Glob::new(pattern).ok();
        let matcher = glob.and_then(|glob| {
            let mut builder = GlobSetBuilder::new();
            builder.add(glob);
            builder.build().ok()
        });
        matcher.is_some_and(|set| set.is_match(path))
    } else {
        path == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextFile, ContextStore, context_path_matches};

    #[test]
    fn context_path_matching_preserves_exact_glob_and_invalid_pattern_behavior() {
        assert!(context_path_matches("CONTRIBUTING.md", "CONTRIBUTING.md"));
        assert!(!context_path_matches("src/lib.rs", "src/main.rs"));
        assert!(context_path_matches("src/lib.rs", "src/*.rs"));
        assert!(!context_path_matches("src/lib.rs", "src/["));
    }

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
