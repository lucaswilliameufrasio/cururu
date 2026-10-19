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
    pub incremental: bool,
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
            incremental: false,
            synthesis: false,
            focus: Vec::new(),
        }
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
            assert!(!policy.incremental && !policy.synthesis && !policy.suggested_changes);
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
