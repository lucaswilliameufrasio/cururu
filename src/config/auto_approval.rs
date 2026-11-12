use super::Severity;

/// Opt-in policy for approving a change request without a human reviewer.
/// Every rule must pass; any doubt means a human review is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoApprovalConfig {
    pub enabled: bool,
    /// When non-empty, every changed path must match one of these globs.
    pub allowed_paths: Vec<String>,
    /// Any changed path matching one of these globs requires human review.
    pub blocked_paths: Vec<String>,
    pub max_changed_files: usize,
    pub max_changed_lines: usize,
    /// Findings at or above this severity block approval.
    pub block_on: Severity,
}

impl Default for AutoApprovalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_paths: Vec::new(),
            blocked_paths: Vec::new(),
            max_changed_files: 20,
            max_changed_lines: 400,
            block_on: Severity::Low,
        }
    }
}
