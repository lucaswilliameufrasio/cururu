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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationError {
    FindingJudgmentCountMismatch,
    InvalidDefectProbability,
    InvalidSeverityConfidence,
}

impl std::fmt::Display for EvaluationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::FindingJudgmentCountMismatch => {
                "evaluator returned a different number of judgments than findings"
            }
            Self::InvalidDefectProbability => "evaluator returned an invalid defect probability",
            Self::InvalidSeverityConfidence => "evaluator returned an invalid severity confidence",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for EvaluationError {}

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

/// Applies evaluator judgments without losing the findings or their audit data.
pub fn apply_judgments(
    findings: Vec<ReviewFinding>,
    judgments: Vec<FindingJudgment>,
    mode: EvaluationMode,
) -> Result<(Vec<ReviewFinding>, EvaluationReport), EvaluationError> {
    if findings.len() != judgments.len() {
        return Err(EvaluationError::FindingJudgmentCountMismatch);
    }

    let mut accepted = Vec::with_capacity(findings.len());
    let mut evaluated = Vec::with_capacity(findings.len());
    for (mut finding, judgment) in findings.into_iter().zip(judgments) {
        if !judgment.defect_probability.is_finite()
            || !(0.0..=1.0).contains(&judgment.defect_probability)
        {
            return Err(EvaluationError::InvalidDefectProbability);
        }
        if judgment
            .severity_confidence
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(EvaluationError::InvalidSeverityConfidence);
        }
        if mode == EvaluationMode::Filter
            && let Some(name) = judgment.severity.and_then(SeverityJudgment::as_str)
        {
            finding.severity = name.into();
        }
        let suppressed = mode == EvaluationMode::Filter
            && (judgment.defect_probability < 0.5
                || judgment.severity == Some(SeverityJudgment::Ignore));
        if !suppressed {
            accepted.push(finding.clone());
        }
        evaluated.push(EvaluatedFinding {
            finding,
            judgment,
            suppressed,
            published: false,
        });
    }

    Ok((
        accepted,
        EvaluationReport {
            model: String::new(),
            findings: evaluated,
            omitted_findings: 0,
            usage: None,
        },
    ))
}

/// Marks findings present in the final published review for the audit report.
pub fn mark_published(report: &mut EvaluationReport, findings: &[ReviewFinding]) {
    for evaluated in &mut report.findings {
        evaluated.published = findings.iter().any(|published| {
            published.path == evaluated.finding.path
                && published.line == evaluated.finding.line
                && published.title == evaluated.finding.title
                && published.message == evaluated.finding.message
                && published.source == evaluated.finding.source
                && published.rule == evaluated.finding.rule
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EvaluatedFinding, EvaluationMode, EvaluationReport, EvaluationUsage, FindingJudgment,
        SeverityJudgment, apply_judgments, mark_published,
    };
    use crate::ReviewFinding;

    fn finding() -> ReviewFinding {
        ReviewFinding {
            severity: "high".into(),
            path: "src/lib.rs".into(),
            line: Some(12),
            title: "Handle the error".into(),
            message: "The error is discarded.".into(),
            suggestion: "Return the error.".into(),
            confidence: 0.8,
            suggested_change: None,
            source: Some("llm".into()),
            rule: Some("error-handling".into()),
        }
    }

    #[test]
    fn observe_and_filter_keep_existing_judgment_semantics() {
        let candidate = finding();
        let judgment = FindingJudgment {
            defect_probability: 0.5,
            severity: Some(SeverityJudgment::Medium),
            severity_confidence: Some(0.8),
        };

        let (observed, observed_report) = apply_judgments(
            vec![candidate.clone()],
            vec![judgment.clone()],
            EvaluationMode::Observe,
        )
        .unwrap();
        assert_eq!(observed[0].severity, "high");
        assert!(!observed_report.findings[0].suppressed);

        let (filtered, filtered_report) =
            apply_judgments(vec![candidate], vec![judgment], EvaluationMode::Filter).unwrap();
        assert_eq!(filtered[0].severity, "medium");
        assert!(!filtered_report.findings[0].suppressed);
    }

    #[test]
    fn filtering_suppresses_low_probability_but_keeps_auditable_finding() {
        let (accepted, report) = apply_judgments(
            vec![finding()],
            vec![FindingJudgment {
                defect_probability: 0.49,
                severity: None,
                severity_confidence: None,
            }],
            EvaluationMode::Filter,
        )
        .unwrap();

        assert!(accepted.is_empty());
        assert!(report.findings[0].suppressed);
        assert_eq!(report.findings[0].finding.path, "src/lib.rs");
    }

    #[test]
    fn invalid_judgments_return_typed_errors() {
        assert_eq!(
            apply_judgments(vec![finding()], Vec::new(), EvaluationMode::Filter).unwrap_err(),
            super::EvaluationError::FindingJudgmentCountMismatch
        );
        assert_eq!(
            apply_judgments(
                vec![finding()],
                vec![FindingJudgment {
                    defect_probability: f32::NAN,
                    severity: None,
                    severity_confidence: None,
                }],
                EvaluationMode::Filter,
            )
            .unwrap_err(),
            super::EvaluationError::InvalidDefectProbability
        );
    }

    #[test]
    fn publication_audit_matches_the_original_finding_identity() {
        let original = finding();
        let mut report = EvaluationReport {
            model: "evaluator".into(),
            findings: vec![EvaluatedFinding {
                finding: original.clone(),
                judgment: FindingJudgment {
                    defect_probability: 0.9,
                    severity: Some(SeverityJudgment::High),
                    severity_confidence: Some(0.9),
                },
                suppressed: false,
                published: false,
            }],
            omitted_findings: 0,
            usage: None,
        };

        mark_published(&mut report, &[original]);
        assert!(report.findings[0].published);
    }

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
