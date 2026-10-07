use anyhow::bail;
use globset::GlobSet;

pub use cururu_core::{FailOn, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentMode {
    Inline,
    Summary,
}

impl CommentMode {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "inline" => Some(Self::Inline),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub max_diff_bytes: usize,
    pub chunk_bytes: usize,
    pub ignore: GlobSet,
    pub language: String,
    pub tone: String,
    pub technical_level: String,
    pub suggestion_detail: String,
    pub comment_mode: CommentMode,
    pub recommendations: bool,
    pub policy: ReviewPolicy,
}

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

pub(super) fn validate_confidence(value: f32) -> anyhow::Result<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        bail!("policy.minimum_confidence must be finite and between 0 and 1")
    }
}

pub(super) fn profile_defaults(name: &str) -> anyhow::Result<ReviewPolicy> {
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
