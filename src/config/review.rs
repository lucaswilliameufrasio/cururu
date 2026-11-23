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

/// Which resolved review threads enter the prior feedback sent to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PriorFeedbackResolved {
    /// Resolved threads with at least one human reply, as recorded decisions.
    #[default]
    WithHumanReply,
    /// No resolved thread (the behavior before 6.3.0).
    None,
}

impl PriorFeedbackResolved {
    pub const NAMES: &'static [&'static str] = &["with_human_reply", "none"];

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "with_human_reply" => Some(Self::WithHumanReply),
            "none" => Some(Self::None),
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
    pub prior_feedback_resolved: PriorFeedbackResolved,
    pub policy: ReviewPolicy,
}

pub use cururu_engine::review_policy::ReviewPolicy;

pub(super) use cururu_engine::config::validate_confidence;

pub(super) use cururu_engine::review_policy::profile_defaults;
