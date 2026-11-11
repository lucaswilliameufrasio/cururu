use crate::{ChangedFile, FindingAnnotation, ReviewFinding, normalize_analysis_path};

/// Convert SCM check-run annotations into review findings for changed files.
#[must_use]
pub fn annotations_to_findings(
    annotations: &[FindingAnnotation],
    changed_files: &[ChangedFile],
) -> Vec<ReviewFinding> {
    let mut findings = Vec::new();
    for annotation in annotations {
        let path = normalize_analysis_path(&annotation.path);
        if !changed_files.iter().any(|file| file.path == path) {
            continue;
        }
        let rule = annotation
            .title
            .clone()
            .unwrap_or_else(|| "check-run".into());
        let severity = match annotation.severity.as_str() {
            "failure" => "high",
            "warning" => "medium",
            _ => "low",
        };
        let message = if annotation.message.is_empty() {
            annotation.details.clone().unwrap_or_else(|| rule.clone())
        } else {
            annotation.message.clone()
        };
        findings.push(ReviewFinding {
            severity: severity.into(),
            path,
            line: annotation.line,
            title: format!("check-run: {rule}"),
            message,
            suggestion:
                "See the check-run annotation and project configuration for the recommended fix."
                    .into(),
            confidence: 1.0,
            suggested_change: None,
            source: Some("check-runs".into()),
            rule: Some(rule),
        });
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::annotations_to_findings;
    use crate::{ChangedFile, FindingAnnotation};

    #[test]
    fn maps_only_changed_files_and_preserves_annotation_semantics() {
        let annotations = [
            FindingAnnotation {
                path: "./src/main.rs".into(),
                line: Some(3),
                severity: "failure".into(),
                message: String::new(),
                title: Some("unwrap".into()),
                details: Some("panic risk".into()),
            },
            FindingAnnotation {
                path: "src/other.rs".into(),
                line: Some(1),
                severity: "warning".into(),
                message: "not in diff".into(),
                title: Some("lint".into()),
                details: None,
            },
        ];
        let changed_files = [ChangedFile {
            path: "src/main.rs".into(),
            patch: String::new(),
            right_lines: vec![3],
        }];

        let findings = annotations_to_findings(&annotations, &changed_files);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, "high");
        assert_eq!(findings[0].path, "src/main.rs");
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[0].title, "check-run: unwrap");
        assert_eq!(findings[0].message, "panic risk");
        assert_eq!(findings[0].source.as_deref(), Some("check-runs"));
        assert_eq!(findings[0].rule.as_deref(), Some("unwrap"));
    }

    #[test]
    fn defaults_missing_title_and_message_and_normalizes_file_uri() {
        let annotations = [FindingAnnotation {
            path: "file://./src/lib.rs".into(),
            line: None,
            severity: "notice".into(),
            message: String::new(),
            title: None,
            details: None,
        }];
        let changed_files = [ChangedFile {
            path: "src/lib.rs".into(),
            patch: String::new(),
            right_lines: vec![],
        }];

        let findings = annotations_to_findings(&annotations, &changed_files);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, "low");
        assert_eq!(findings[0].title, "check-run: check-run");
        assert_eq!(findings[0].message, "check-run");
    }
}
