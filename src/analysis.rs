use crate::{agent::ReviewFinding, config::AnalysisConfig, diff::ChangedFile};
use anyhow::Context;
use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;
use std::path::{Path, PathBuf};

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

pub fn load_findings(
    config: &AnalysisConfig,
    changed_files: &[ChangedFile],
) -> anyhow::Result<Vec<ReviewFinding>> {
    if !config.enabled || config.sarif_paths.is_empty() {
        return Ok(Vec::new());
    }

    let paths = expand_paths(&config.sarif_paths)?;
    let mut findings = Vec::new();
    for path in paths {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read SARIF file {}", path.display()))?;
        let sarif: Sarif = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse SARIF file {}", path.display()))?;
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
                    .map(normalize_path)
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
                findings.push(ReviewFinding {
                    severity: normalize_level(result.level.as_deref()),
                    path,
                    line,
                    title: format!("{tool}: {rule}"),
                    message,
                    suggestion: "See the analyzer diagnostic and project configuration for the recommended fix.".into(),
                    confidence: 1.0,
                    suggested_change: None,
                    source: Some(tool),
                    rule: Some(rule),
                });
                if findings.len() >= config.max_findings {
                    return Ok(findings);
                }
            }
        }
    }
    Ok(findings)
}

fn normalize_level(level: Option<&str>) -> String {
    match level.unwrap_or("warning").to_ascii_lowercase().as_str() {
        "error" | "failure" => "high",
        "warning" | "warn" => "medium",
        _ => "low",
    }
    .into()
}

fn normalize_path(uri: &str) -> String {
    uri.strip_prefix("file://")
        .unwrap_or(uri)
        .trim_start_matches("./")
        .to_string()
}

fn expand_paths(patterns: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    let mut files = Vec::new();
    let matcher = builder.build()?;
    for pattern in patterns {
        let direct = PathBuf::from(pattern);
        if direct.is_file() {
            files.push(direct);
        }
    }
    collect_files(Path::new("."), &matcher, &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_files(
    directory: &Path,
    matcher: &globset::GlobSet,
    files: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, matcher, files)?;
        } else if matcher.is_match(&path) {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_sarif_and_filters_to_changed_files() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("analysis.sarif");
        std::fs::write(
            &path,
            r#"{"runs":[{"tool":{"driver":{"name":"demo-linter"}},"results":[{"ruleId":"SEC001","level":"error","message":{"text":"Bad input"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/main.rs"},"region":{"startLine":7}}}]},{"ruleId":"OTHER","level":"warning","message":{"text":"Ignored"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/other.rs"},"region":{"startLine":2}}}]}]}]}"#,
        )
        .unwrap();
        let config = AnalysisConfig {
            enabled: true,
            sarif_paths: vec![path.to_string_lossy().into()],
            max_findings: 10,
        };
        let changed = vec![ChangedFile {
            path: "src/main.rs".into(),
            patch: String::new(),
            right_lines: vec![7],
        }];
        let findings = load_findings(&config, &changed).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, "high");
        assert_eq!(findings[0].rule.as_deref(), Some("SEC001"));
    }
}
