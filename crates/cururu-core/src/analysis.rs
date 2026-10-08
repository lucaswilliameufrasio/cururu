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

/// Versioned, provider-neutral manifest linking analyzer runs to their SARIF files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct AnalysisManifest {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub tools: Vec<AnalysisManifestTool>,
}

impl AnalysisManifest {
    /// Report whether this manifest identifies a different analyzed revision.
    #[must_use]
    pub fn is_stale_for(&self, expected_head: &str) -> bool {
        self.commit_sha
            .as_deref()
            .is_some_and(|commit_sha| commit_sha != expected_head)
    }
}

/// One analyzer's execution metadata in an [`AnalysisManifest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct AnalysisManifestTool {
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub sarif_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{AnalysisManifest, AnalysisManifestTool, AnalysisReport, AnalysisTool};
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
    fn analysis_manifest_round_trips_and_defaults_optional_fields() {
        let manifest: AnalysisManifest = serde_json::from_str(
            r#"{"schema_version":1,"tools":[{"name":"clippy","status":"succeeded","exit_code":0,"sarif_path":"target/clippy.sarif"}]}"#,
        )
        .unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.commit_sha, None);
        assert_eq!(manifest.tools[0].message, None);
        assert_eq!(
            manifest.tools[0].sarif_path.as_deref(),
            Some("target/clippy.sarif")
        );

        let complete = AnalysisManifest {
            schema_version: 1,
            commit_sha: Some("abc123".into()),
            tools: vec![AnalysisManifestTool {
                name: "clippy".into(),
                status: "succeeded".into(),
                exit_code: Some(0),
                message: Some("clean".into()),
                sarif_path: Some("target/clippy.sarif".into()),
            }],
        };
        let encoded = serde_json::to_string(&complete).unwrap();
        assert_eq!(
            serde_json::from_str::<AnalysisManifest>(&encoded).unwrap(),
            complete
        );
    }

    #[test]
    fn manifest_freshness_requires_a_present_matching_commit_sha() {
        let manifest_without_sha: AnalysisManifest =
            serde_json::from_str(r#"{"schema_version":1,"tools":[]}"#).unwrap();
        let manifest_with_sha = AnalysisManifest {
            schema_version: 1,
            commit_sha: Some("abc123".into()),
            tools: Vec::new(),
        };

        assert!(!manifest_without_sha.is_stale_for("def456"));
        assert!(!manifest_with_sha.is_stale_for("abc123"));
        assert!(manifest_with_sha.is_stale_for("def456"));
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
