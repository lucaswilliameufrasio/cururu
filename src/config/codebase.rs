/// Whole-codebase review settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodebaseConfig {
    /// Most file content collected for one codebase review.
    pub max_bytes: usize,
}

impl Default for CodebaseConfig {
    fn default() -> Self {
        Self {
            max_bytes: crate::codebase::DEFAULT_MAX_BYTES,
        }
    }
}
