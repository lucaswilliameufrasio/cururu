use crate::ReviewFinding;

/// Sort findings by the existing Cururu severity, path, and line order.
pub fn sort_review_findings(findings: &mut [ReviewFinding]) {
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
}

/// Deduplicate findings at the same location while preferring analyzer evidence.
#[must_use]
pub fn deduplicate_review_findings(findings: Vec<ReviewFinding>) -> Vec<ReviewFinding> {
    let mut unique: Vec<ReviewFinding> = Vec::with_capacity(findings.len());
    for finding in findings {
        let matches = unique.iter().position(|existing| {
            existing.path == finding.path
                && existing.line == finding.line
                && same_rule(existing, &finding)
                && titles_overlap(existing, &finding)
        });
        match matches {
            Some(index) => {
                let existing = &mut unique[index];
                if merge_prefers(&finding, existing) {
                    *existing = finding;
                }
            }
            None => unique.push(finding),
        }
    }
    unique
}

fn same_rule(a: &ReviewFinding, b: &ReviewFinding) -> bool {
    match (&a.rule, &b.rule) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

fn titles_overlap(a: &ReviewFinding, b: &ReviewFinding) -> bool {
    match (a.rule.as_deref(), b.rule.as_deref()) {
        (Some(x), Some(y)) => x == y,
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => a.title.eq_ignore_ascii_case(&b.title),
    }
}

fn merge_prefers(candidate: &ReviewFinding, existing: &ReviewFinding) -> bool {
    let candidate_is_tool = candidate.source.is_some();
    let existing_is_tool = existing.source.is_some();
    match (candidate_is_tool, existing_is_tool) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate.confidence > existing.confidence,
    }
}

fn severity_rank(severity: &str) -> u8 {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::{deduplicate_review_findings, sort_review_findings};
    use crate::ReviewFinding;

    fn finding(severity: &str, path: &str, line: u32, title: &str) -> ReviewFinding {
        ReviewFinding {
            severity: severity.into(),
            path: path.into(),
            line: Some(line),
            title: title.into(),
            message: title.into(),
            suggestion: String::new(),
            confidence: 0.8,
            suggested_change: None,
            source: None,
            rule: None,
        }
    }

    #[test]
    fn sorting_preserves_severity_path_and_line_order() {
        let mut findings = vec![
            finding("low", "z.rs", 4, "low"),
            finding("high", "b.rs", 8, "later"),
            finding("unknown", "a.rs", 1, "unknown"),
            finding("high", "a.rs", 9, "high"),
        ];

        sort_review_findings(&mut findings);

        assert_eq!(
            findings
                .iter()
                .map(|finding| (
                    finding.severity.as_str(),
                    finding.path.as_str(),
                    finding.line
                ))
                .collect::<Vec<_>>(),
            [
                ("high", "a.rs", Some(9)),
                ("high", "b.rs", Some(8)),
                ("low", "z.rs", Some(4)),
                ("unknown", "a.rs", Some(1)),
            ]
        );
    }

    #[test]
    fn deduplication_prefers_tool_findings_and_keeps_distinct_rules() {
        let mut llm = finding("high", "src/lib.rs", 7, "possible issue");
        llm.confidence = 0.99;
        let mut tool = finding("medium", "src/lib.rs", 7, "tool issue");
        tool.source = Some("clippy".into());
        tool.rule = Some("unused_must_use".into());
        let mut other_rule = tool.clone();
        other_rule.rule = Some("needless_return".into());

        let deduplicated = deduplicate_review_findings(vec![llm, tool, other_rule]);

        assert_eq!(deduplicated.len(), 2);
        assert_eq!(deduplicated[0].source.as_deref(), Some("clippy"));
        assert_eq!(deduplicated[0].severity, "medium");
        assert_ne!(deduplicated[0].rule, deduplicated[1].rule);
    }
}
