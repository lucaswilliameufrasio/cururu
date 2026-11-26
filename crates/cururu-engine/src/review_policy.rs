//! Pure profile values. Hosts assemble runtime configuration and permissions.

use anyhow::bail;
use cururu_core::{FailOn, Severity};

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewPolicy {
    pub profile: String,
    pub minimum_confidence: f32,
    pub max_findings: usize,
    pub fail_on: FailOn,
    pub allowed_severities: Vec<Severity>,
    pub suggested_changes: bool,
    pub incremental: Incremental,
    pub synthesis: bool,
    pub focus: Vec<String>,
}

impl Default for ReviewPolicy {
    fn default() -> Self {
        Self {
            profile: "balanced".into(),
            minimum_confidence: 0.65,
            max_findings: 30,
            fail_on: FailOn::Off,
            allowed_severities: Severity::all(),
            suggested_changes: false,
            incremental: Incremental::Off,
            synthesis: false,
            focus: Vec::new(),
        }
    }
}

/// `policy.incremental`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Incremental {
    /// Review every run in full (`false`, the default).
    #[default]
    Off,
    /// Skip a run when the current head already has a Cururu summary
    /// (`true`); a new push is still reviewed in full.
    SameHead,
    /// Like `SameHead`, and re-review only files whose PR-diff patch changed
    /// since the last published review (`"changed_files"`).
    ChangedFiles,
}

impl Incremental {
    /// Whether a head that already has a Cururu summary is skipped.
    #[must_use]
    pub const fn skips_reviewed_head(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether only files changed since the last review are sent.
    #[must_use]
    pub const fn changed_files(self) -> bool {
        matches!(self, Self::ChangedFiles)
    }
}

pub fn profile_defaults(name: &str) -> anyhow::Result<ReviewPolicy> {
    let mut policy = ReviewPolicy {
        profile: name.to_string(),
        ..ReviewPolicy::default()
    };
    match name.trim().to_ascii_lowercase().as_str() {
        "balanced" => {}
        "strict" => {
            policy.minimum_confidence = 0.8;
            policy.max_findings = 50;
            policy.fail_on = FailOn::High;
        }
        "security" => {
            policy.minimum_confidence = 0.75;
            policy.max_findings = 40;
            policy.fail_on = FailOn::High;
            policy.allowed_severities = vec![Severity::Critical, Severity::High, Severity::Medium];
            policy.focus = vec!["security".into()];
        }
        "minimal" => {
            policy.minimum_confidence = 0.8;
            policy.max_findings = 10;
            policy.allowed_severities = vec![Severity::Critical, Severity::High];
        }
        _ => bail!("invalid review.profile: {name}"),
    }
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_preserves_values_and_does_not_enable_side_effects() {
        assert_eq!(
            profile_defaults("balanced").unwrap(),
            ReviewPolicy::default()
        );
        for (name, confidence, count, threshold) in [
            ("strict", 0.8_f32, 50, FailOn::High),
            ("security", 0.75, 40, FailOn::High),
            ("minimal", 0.8, 10, FailOn::Off),
        ] {
            let policy = profile_defaults(name).unwrap();
            assert_eq!(policy.minimum_confidence.to_bits(), confidence.to_bits());
            assert_eq!(policy.max_findings, count);
            assert_eq!(policy.fail_on, threshold);
            assert!(
                policy.incremental == Incremental::Off
                    && !policy.synthesis
                    && !policy.suggested_changes
            );
        }
        assert_eq!(profile_defaults("security").unwrap().focus, ["security"]);
        assert_eq!(
            profile_defaults("minimal").unwrap().allowed_severities,
            [Severity::Critical, Severity::High]
        );
        assert_eq!(profile_defaults(" STRICT ").unwrap().profile, " STRICT ");
        assert_eq!(
            profile_defaults("unknown").unwrap_err().to_string(),
            "invalid review.profile: unknown"
        );
    }
}
