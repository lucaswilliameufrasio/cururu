//! Pure parsing of the established Cururu SARIF subset. Hosts bound inputs/I/O.

use cururu_core::{ChangedFile, ReviewFinding, normalize_analysis_path, sarif_finding};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Sarif {
    #[serde(default)]
    runs: Vec<SarifRun>,
}

#[derive(Debug, Deserialize)]
struct SarifRun {
    tool: SarifTool,
    #[serde(default)]
    results: Vec<SarifResult>,
}

#[derive(Debug, Deserialize)]
struct SarifTool {
    driver: SarifDriver,
}

#[derive(Debug, Deserialize)]
struct SarifDriver {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SarifResult {
    #[serde(rename = "ruleId")]
    rule_id: Option<String>,
    level: Option<String>,
    message: SarifMessage,
    #[serde(default)]
    locations: Vec<SarifLocation>,
}

#[derive(Debug, Deserialize)]
struct SarifMessage {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SarifLocation {
    #[serde(rename = "physicalLocation")]
    physical_location: Option<SarifPhysicalLocation>,
}

#[derive(Debug, Deserialize)]
struct SarifPhysicalLocation {
    #[serde(rename = "artifactLocation")]
    artifact_location: Option<SarifArtifactLocation>,
    region: Option<SarifRegion>,
}

#[derive(Debug, Deserialize)]
struct SarifArtifactLocation {
    uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SarifRegion {
    #[serde(rename = "startLine")]
    start_line: Option<u32>,
}

/// Parse a supplied string without reading any referenced resource. The limit
/// preserves the CLI's historical behavior (zero still allows a first finding).
pub fn parse_findings(
    raw: &str,
    changed_files: &[ChangedFile],
    max_findings: usize,
) -> Result<Vec<ReviewFinding>, serde_json::Error> {
    let sarif: Sarif = serde_json::from_str(raw)?;
    let mut findings = Vec::new();
    for run in sarif.runs {
        for result in run.results {
            let Some(location) = result.locations.first() else {
                continue;
            };
            let Some(physical) = &location.physical_location else {
                continue;
            };
            let Some(path) = physical
                .artifact_location
                .as_ref()
                .and_then(|location| location.uri.as_deref())
                .map(normalize_analysis_path)
            else {
                continue;
            };
            if !changed_files.iter().any(|file| file.path == path) {
                continue;
            }
            let line = physical
                .region
                .as_ref()
                .and_then(|region| region.start_line);
            let message = result
                .message
                .text
                .unwrap_or_else(|| "Static analysis reported an issue.".into());
            let rule = result.rule_id.unwrap_or_else(|| "unknown".into());
            let tool = run.tool.driver.name.clone();
            findings.push(sarif_finding(
                &tool,
                &rule,
                result.level.as_deref(),
                path,
                line,
                message,
            ));
            if findings.len() >= max_findings {
                return Ok(findings);
            }
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_preserves_filtering_defaults_locations_order_and_legacy_limits() {
        let files = vec![ChangedFile {
            path: "src/lib.rs".to_string(),
            patch: String::new(),
            right_lines: vec![3],
        }];
        let raw = r#"{"runs":[{"tool":{"driver":{"name":"check"}},"results":[{"message":{},"locations":[]},{"ruleId":"first","level":"error","message":{"text":"First"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"file://./src/lib.rs"},"region":{"startLine":3}}}]},{"message":{},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/lib.rs"}}}]},{"message":{},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"other.rs"}}}]}]}]}"#;
        let findings = parse_findings(raw, &files, 100).unwrap();
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].rule.as_deref(), Some("first"));
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[0].severity, "high");
        assert_eq!(findings[1].rule.as_deref(), Some("unknown"));
        assert_eq!(findings[1].message, "Static analysis reported an issue.");
        assert_eq!(parse_findings(raw, &files, 0).unwrap().len(), 1);
        assert_eq!(parse_findings(raw, &files, 1).unwrap().len(), 1);
        assert!(parse_findings("not-json", &files, 100).is_err());
        assert!(parse_findings("{}", &files, 100).unwrap().is_empty());
    }
}
