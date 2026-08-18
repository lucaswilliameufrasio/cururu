use crate::agent::ReviewResult;
use crate::config::{FailOn, Severity};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QualityReport {
    pub status: String,
    pub passed: bool,
    pub findings_count: usize,
    pub critical_count: usize,
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    pub highest_severity: String,
}

pub fn evaluate(review: &ReviewResult, fail_on: FailOn) -> QualityReport {
    let mut counts = [0usize; 4];
    for finding in &review.findings {
        if let Some(severity) = Severity::from_name(&finding.severity) {
            counts[severity.rank() as usize] += 1;
        }
    }

    let highest = counts.iter().position(|count| *count > 0).map_or_else(
        || "none".to_string(),
        |rank| severity_name(rank).to_string(),
    );
    let passed = fail_on == FailOn::Off
        || counts
            .iter()
            .enumerate()
            .all(|(rank, count)| *count == 0 || rank > fail_on.rank() as usize);

    QualityReport {
        status: if fail_on == FailOn::Off {
            "disabled"
        } else if passed {
            "passed"
        } else {
            "failed"
        }
        .into(),
        passed,
        findings_count: review.findings.len(),
        critical_count: counts[0],
        high_count: counts[1],
        medium_count: counts[2],
        low_count: counts[3],
        highest_severity: highest,
    }
}

const fn severity_name(rank: usize) -> &'static str {
    match rank {
        0 => "critical",
        1 => "high",
        2 => "medium",
        3 => "low",
        _ => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ReviewFinding;

    fn review(severities: &[&str]) -> ReviewResult {
        ReviewResult {
            model: "test".into(),
            files_reviewed: 1,
            summary: String::new(),
            findings: severities
                .iter()
                .map(|severity| ReviewFinding {
                    severity: (*severity).into(),
                    path: "src/lib.rs".into(),
                    line: Some(1),
                    title: "finding".into(),
                    message: "message".into(),
                    suggestion: "fix".into(),
                    confidence: 0.9,
                    suggested_change: None,
                    source: None,
                    rule: None,
                })
                .collect(),
        }
    }

    #[test]
    fn disabled_gate_is_report_only() {
        let report = evaluate(&review(&["critical"]), FailOn::Off);
        assert_eq!(report.status, "disabled");
        assert!(report.passed);
    }

    #[test]
    fn high_gate_fails_on_critical_or_high() {
        assert!(!evaluate(&review(&["critical"]), FailOn::High).passed);
        assert!(!evaluate(&review(&["high"]), FailOn::High).passed);
        assert!(evaluate(&review(&["medium"]), FailOn::High).passed);
    }
}
