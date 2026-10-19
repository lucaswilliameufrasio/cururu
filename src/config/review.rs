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

pub use cururu_engine::review_policy::ReviewPolicy;

pub(super) use cururu_engine::config::validate_confidence;

pub(super) use cururu_engine::review_policy::profile_defaults;
