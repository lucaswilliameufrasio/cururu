use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Severity levels assigned to actionable review findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![Self::Critical, Self::High, Self::Medium, Self::Low]
    }
}

/// Optional threshold for turning review findings into a failing quality gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FailOn {
    Off,
    Critical,
    High,
    Medium,
    Low,
}

impl FailOn {
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(Self::Off),
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Off => u8::MAX,
            Self::Critical => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FailOn, Severity};

    #[test]
    fn severity_names_and_order_are_provider_independent() {
        assert_eq!(Severity::from_name(" HIGH "), Some(Severity::High));
        assert_eq!(Severity::Critical.rank(), 0);
        assert_eq!(Severity::Low.rank(), 3);
        assert_eq!(Severity::all().len(), 4);
    }

    #[test]
    fn fail_on_threshold_parses_off_alias_and_levels() {
        assert_eq!(FailOn::from_name("none"), Some(FailOn::Off));
        assert_eq!(FailOn::from_name("medium").unwrap().rank(), 2);
    }
}
