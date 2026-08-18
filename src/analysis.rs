use crate::{agent::ReviewFinding, config::AnalysisConfig, diff::ChangedFile};
use anyhow::Context;
use globset::{Glob, GlobSetBuilder};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisReport {
    pub status: String,
    pub tools: Vec<AnalysisTool>,
    pub findings: Vec<ReviewFinding>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnalysisTool {
    pub name: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    commit_sha: Option<String>,
    #[serde(default)]
    tools: Vec<ManifestTool>,
}

#[derive(Debug, Deserialize)]
struct ManifestTool {
    name: String,
    status: String,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    sarif_path: Option<String>,
}

pub fn load_evidence(
    config: &AnalysisConfig,
    changed_files: &[ChangedFile],
    expected_head: &str,
) -> anyhow::Result<AnalysisReport> {
    if !config.enabled {
        return Ok(AnalysisReport {
            status: "disabled".into(),
            tools: Vec::new(),
            findings: Vec::new(),
        });
    }

    let mut tools = Vec::new();
    let mut sarif_paths = config.sarif_paths.clone();
    if let Some(manifest_path) = &config.manifest {
        let raw = std::fs::read_to_string(manifest_path)
            .with_context(|| format!("failed to read analysis manifest {manifest_path}"))?;
        let manifest: Manifest = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse analysis manifest {manifest_path}"))?;
        if manifest.schema_version != 1 {
            anyhow::bail!(
                "unsupported analysis manifest version {} (expected 1)",
                manifest.schema_version
            );
        }
        if config.require_current_head
            && manifest
                .commit_sha
                .as_deref()
                .is_some_and(|sha| sha != expected_head)
        {
            return Ok(AnalysisReport {
                status: "stale".into(),
                tools: Vec::new(),
                findings: Vec::new(),
            });
        }
        for tool in manifest.tools {
            if let Some(path) = tool.sarif_path {
                sarif_paths.push(path);
            }
            tools.push(AnalysisTool {
                name: tool.name,
                status: tool.status,
                exit_code: tool.exit_code,
                message: tool.message,
            });
        }
    }

    let findings = load_sarif_paths(&sarif_paths, changed_files, config.max_findings)?;
    let status = if tools.iter().any(|tool| tool.status == "failed") {
        "failed"
    } else if tools.iter().any(|tool| tool.status == "not_run") {
        "partial"
    } else if tools.is_empty() && findings.is_empty() {
        "no_evidence"
    } else {
        "passed"
    };
    Ok(AnalysisReport {
        status: status.into(),
        tools,
        findings,
    })
}

fn load_sarif_paths(
    configured_paths: &[String],
    changed_files: &[ChangedFile],
    max_findings: usize,
) -> anyhow::Result<Vec<ReviewFinding>> {
    let paths = expand_paths(configured_paths)?;
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
                if findings.len() >= max_findings {
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
            manifest: None,
            sarif_paths: vec![path.to_string_lossy().into()],
            max_findings: 10,
            require_current_head: true,
        };
        let changed = vec![ChangedFile {
            path: "src/main.rs".into(),
            patch: String::new(),
            right_lines: vec![7],
        }];
        let report = load_evidence(&config, &changed, "head").unwrap();
        assert_eq!(report.status, "passed");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].severity, "high");
        assert_eq!(report.findings[0].rule.as_deref(), Some("SEC001"));
    }

    #[test]
    fn reports_tool_failure_and_rejects_stale_manifest() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("analysis.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"commit_sha":"old","tools":[{"name":"compiler","status":"failed","exit_code":101,"message":"compilation failed"}]}"#,
        )
        .unwrap();
        let config = AnalysisConfig {
            enabled: true,
            manifest: Some(path.to_string_lossy().into()),
            sarif_paths: vec![],
            max_findings: 10,
            require_current_head: true,
        };
        let changed = Vec::new();
        let report = load_evidence(&config, &changed, "new").unwrap();
        assert_eq!(report.status, "stale");

        std::fs::write(
            &path,
            r#"{"schema_version":1,"commit_sha":"new","tools":[{"name":"compiler","status":"failed","exit_code":101,"message":"compilation failed"}]}"#,
        )
        .unwrap();
        let report = load_evidence(&config, &changed, "new").unwrap();
        assert_eq!(report.status, "failed");
        assert_eq!(report.tools[0].exit_code, Some(101));
    }
}
