use crate::ReviewFinding;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Static-analysis results associated with one reviewed change.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisReport {
    pub status: String,
    pub tools: Vec<AnalysisTool>,
    pub findings: Vec<ReviewFinding>,
}

impl AnalysisReport {
    /// Build a report while applying Cururu's existing analysis-status precedence.
    #[must_use]
    pub fn from_parts(tools: Vec<AnalysisTool>, findings: Vec<ReviewFinding>) -> Self {
        let status = if tools.iter().any(|tool| tool.status == "failed") {
            "failed"
        } else if tools.iter().any(|tool| tool.status == "not_run") {
            "partial"
        } else if tools.is_empty() && findings.is_empty() {
            "no_evidence"
        } else {
            "passed"
        };

        Self {
            status: status.into(),
            tools,
            findings,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct AnalysisTool {
    pub name: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{AnalysisReport, AnalysisTool};
    use crate::ReviewFinding;

    #[test]
    fn analysis_report_round_trips_for_desktop_consumers() {
        let report = AnalysisReport {
            status: "complete".into(),
            tools: vec![AnalysisTool {
                name: "static-check".into(),
                status: "succeeded".into(),
                exit_code: Some(0),
                message: None,
            }],
            findings: vec![ReviewFinding {
                severity: "medium".into(),
                path: "src/lib.rs".into(),
                line: Some(7),
                title: "Potential issue".into(),
                message: "The result should be checked.".into(),
                suggestion: "Handle the result.".into(),
                confidence: 1.0,
                suggested_change: None,
                source: Some("static-check".into()),
                rule: Some("result-check".into()),
            }],
        };

        let json = serde_json::to_string(&report).unwrap();
        let decoded: AnalysisReport = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status, "complete");
        assert_eq!(decoded.tools, report.tools);
        assert_eq!(decoded.findings.len(), 1);
        assert_eq!(decoded.findings[0].path, "src/lib.rs");
        assert_eq!(decoded.findings[0].source.as_deref(), Some("static-check"));
    }

    #[test]
    fn report_status_keeps_existing_precedence_and_empty_case() {
        let failed = AnalysisTool {
            name: "compiler".into(),
            status: "failed".into(),
            exit_code: Some(1),
            message: None,
        };
        let not_run = AnalysisTool {
            name: "linter".into(),
            status: "not_run".into(),
            exit_code: None,
            message: None,
        };

        assert_eq!(
            AnalysisReport::from_parts(vec![], vec![]).status,
            "no_evidence"
        );
        assert_eq!(
            AnalysisReport::from_parts(vec![], vec![finding()]).status,
            "passed"
        );
        assert_eq!(
            AnalysisReport::from_parts(vec![not_run.clone()], vec![]).status,
            "partial"
        );
        assert_eq!(
            AnalysisReport::from_parts(vec![failed.clone(), not_run], vec![]).status,
            "failed"
        );
        assert_eq!(
            AnalysisReport::from_parts(vec![failed], vec![]).status,
            "failed"
        );
    }

    fn finding() -> ReviewFinding {
        ReviewFinding {
            severity: "low".into(),
            path: "src/lib.rs".into(),
            line: None,
            title: "Finding".into(),
            message: "Message".into(),
            suggestion: "Suggestion".into(),
            confidence: 1.0,
            suggested_change: None,
            source: None,
            rule: None,
        }
    }
}
