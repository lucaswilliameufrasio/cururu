use crate::ReviewFinding;

/// Convert one parsed SARIF result into Cururu's provider-neutral finding model.
#[must_use]
pub fn sarif_finding(
    tool: &str,
    rule: &str,
    level: Option<&str>,
    path: String,
    line: Option<u32>,
    message: String,
) -> ReviewFinding {
    let severity = match level.unwrap_or("warning").to_ascii_lowercase().as_str() {
        "error" | "failure" => "high",
        "warning" | "warn" => "medium",
        _ => "low",
    };
    ReviewFinding {
        severity: severity.into(),
        path,
        line,
        title: format!("{tool}: {rule}"),
        message,
        suggestion:
            "See the analyzer diagnostic and project configuration for the recommended fix.".into(),
        confidence: 1.0,
        suggested_change: None,
        source: Some(tool.into()),
        rule: Some(rule.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::sarif_finding;

    #[test]
    fn maps_sarif_levels_and_preserves_finding_fields() {
        let finding = sarif_finding(
            "demo-linter",
            "SEC001",
            Some("ERROR"),
            "src/main.rs".into(),
            Some(7),
            "Bad input".into(),
        );

        assert_eq!(finding.severity, "high");
        assert_eq!(finding.path, "src/main.rs");
        assert_eq!(finding.line, Some(7));
        assert_eq!(finding.title, "demo-linter: SEC001");
        assert_eq!(finding.message, "Bad input");
        assert!((finding.confidence - 1.0).abs() < f32::EPSILON);
        assert_eq!(finding.source.as_deref(), Some("demo-linter"));
        assert_eq!(finding.rule.as_deref(), Some("SEC001"));
        assert!(finding.suggested_change.is_none());
    }

    #[test]
    fn defaults_missing_level_to_warning_and_unknown_levels_to_low() {
        assert_eq!(
            sarif_finding("tool", "rule", None, "a.rs".into(), None, "msg".into()).severity,
            "medium"
        );
        assert_eq!(
            sarif_finding(
                "tool",
                "rule",
                Some("note"),
                "a.rs".into(),
                None,
                "msg".into()
            )
            .severity,
            "low"
        );
    }
}
