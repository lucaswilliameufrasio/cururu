use crate::ReviewFinding;
use serde::{Deserialize, Serialize};

/// Controls whether evaluator results are recorded only or affect review output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationMode {
    Observe,
    Filter,
}

impl EvaluationMode {
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "observe" => Some(Self::Observe),
            "filter" => Some(Self::Filter),
            _ => None,
        }
    }
}

/// A provider-neutral judgment about one review finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindingJudgment {
    pub defect_probability: f32,
    pub severity: Option<SeverityJudgment>,
    pub severity_confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeverityJudgment {
    Critical,
    High,
    Medium,
    Low,
    Ignore,
}

impl SeverityJudgment {
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Critical => Some("critical"),
            Self::High => Some("high"),
            Self::Medium => Some("medium"),
            Self::Low => Some("low"),
            Self::Ignore => None,
        }
    }
}

/// The original finding and its evaluator result, retained for auditability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatedFinding {
    pub finding: ReviewFinding,
    pub judgment: FindingJudgment,
    pub suppressed: bool,
    #[serde(default)]
    pub published: bool,
}

/// Evaluation metadata associated with a review run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub model: String,
    pub findings: Vec<EvaluatedFinding>,
    #[serde(default)]
    pub omitted_findings: usize,
    pub usage: Option<EvaluationUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationUsage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::{
        EvaluatedFinding, EvaluationReport, EvaluationUsage, FindingJudgment, SeverityJudgment,
    };
    use crate::ReviewFinding;

    #[test]
    fn evaluation_report_round_trips_with_legacy_default_fields() {
        let report = EvaluationReport {
            model: "evaluator-model".into(),
            findings: vec![EvaluatedFinding {
                finding: ReviewFinding {
                    severity: "high".into(),
                    path: "src/lib.rs".into(),
                    line: Some(12),
                    title: "Handle the error".into(),
                    message: "The error is discarded.".into(),
                    suggestion: "Return the error.".into(),
                    confidence: 0.8,
                    suggested_change: None,
                    source: Some("llm".into()),
                    rule: None,
                },
                judgment: FindingJudgment {
                    defect_probability: 0.9,
                    severity: Some(SeverityJudgment::High),
                    severity_confidence: Some(0.75),
                },
                suppressed: false,
                published: true,
            }],
            omitted_findings: 2,
            usage: Some(EvaluationUsage {
                input_tokens: Some(120),
                output_tokens: Some(35),
            }),
        };

        let json = serde_json::to_string(&report).unwrap();
        let decoded: EvaluationReport = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.model, report.model);
        assert_eq!(decoded.findings.len(), 1);
        assert_eq!(decoded.findings[0].finding.path, "src/lib.rs");
        assert_eq!(decoded.findings[0].judgment, report.findings[0].judgment);
        assert!(decoded.findings[0].published);
        assert_eq!(decoded.omitted_findings, 2);
        assert_eq!(decoded.usage.unwrap().input_tokens, Some(120));
    }

    #[test]
    fn evaluation_report_accepts_older_missing_audit_fields() {
        let report: EvaluationReport =
            serde_json::from_str(r#"{"model":"legacy","findings":[],"usage":null}"#).unwrap();

        assert_eq!(report.omitted_findings, 0);
        assert!(report.findings.is_empty());
    }
}
