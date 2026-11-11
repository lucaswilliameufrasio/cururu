use crate::{agent::ReviewFinding, config::AnalysisConfig, diff::ChangedFile, scm::ScmProvider};
use anyhow::Context;
use cururu_core::annotations_to_findings;
pub use cururu_core::{AnalysisManifest, AnalysisReport, AnalysisTool};
use globset::{Glob, GlobSetBuilder};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_ANALYSIS_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub fn read_bounded(path: &Path) -> anyhow::Result<String> {
    let file = std::fs::File::open(path).context("failed to open analysis file")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "analysis input must be a regular file"
    );
    let mut bytes = Vec::new();
    file.take(MAX_ANALYSIS_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        u64::try_from(bytes.len())? <= MAX_ANALYSIS_FILE_BYTES,
        "analysis input exceeds byte limit"
    );
    String::from_utf8(bytes).context("analysis input must be UTF-8")
}

pub async fn load_evidence(
    config: &AnalysisConfig,
    changed_files: &[ChangedFile],
    expected_head: &str,
    source_control: &dyn ScmProvider,
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
        let raw = read_bounded(Path::new(manifest_path))
            .with_context(|| format!("failed to read analysis manifest {manifest_path}"))?;
        let manifest: AnalysisManifest = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse analysis manifest {manifest_path}"))?;
        if !manifest.has_supported_schema_version() {
            anyhow::bail!(
                "unsupported analysis manifest version {} (expected {})",
                manifest.schema_version,
                AnalysisManifest::CURRENT_SCHEMA_VERSION
            );
        }
        if config.require_current_head && manifest.is_stale_for(expected_head) {
            return Ok(AnalysisReport {
                status: "stale".into(),
                tools: Vec::new(),
                findings: Vec::new(),
            });
        }
        let (manifest_tools, manifest_sarif_paths) = manifest.into_analysis_data();
        tools.extend(manifest_tools);
        sarif_paths.extend(manifest_sarif_paths);
    }

    let mut findings = load_sarif_paths(&sarif_paths, changed_files, config.max_findings)?;
    if config.check_runs {
        let annotations = source_control
            .list_finding_annotations(expected_head, &config.check_run_names)
            .await?;
        let check_findings = annotations_to_findings(&annotations, changed_files);
        anyhow::ensure!(
            check_findings.len() <= config.max_findings.saturating_sub(findings.len()),
            "analyzer evidence exceeds finding limit; increase analysis.max_findings or reduce evidence"
        );
        findings.extend(check_findings);
        if !annotations.is_empty()
            && tools.iter().all(|tool| tool.status != "failed")
            && tools.iter().all(|tool| tool.status != "not_run")
        {
            tools.push(AnalysisTool {
                name: "check-runs".into(),
                status: "failed".into(),
                exit_code: None,
                message: Some(format!("{} annotation(s) reported", annotations.len())),
            });
        }
    }

    Ok(AnalysisReport::from_parts(tools, findings))
}

fn load_sarif_paths(
    configured_paths: &[String],
    changed_files: &[ChangedFile],
    max_findings: usize,
) -> anyhow::Result<Vec<ReviewFinding>> {
    let paths = expand_paths(configured_paths)?;
    let mut findings = Vec::new();
    for path in paths {
        let raw = read_bounded(&path)
            .with_context(|| format!("failed to read SARIF file {}", path.display()))?;
        let parsed = cururu_engine::sarif::parse_findings(
            &raw,
            changed_files,
            max_findings
                .saturating_sub(findings.len())
                .saturating_add(1),
        )
        .with_context(|| format!("failed to parse SARIF file {}", path.display()))?;
        anyhow::ensure!(
            parsed.len() <= max_findings.saturating_sub(findings.len()),
            "analyzer evidence exceeds finding limit; increase analysis.max_findings or reduce evidence"
        );
        findings.extend(parsed);
    }
    Ok(findings)
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
    use crate::config::GitHubConfig;
    use crate::github::GitHubClient;
    use tempfile::tempdir;

    #[test]
    fn analysis_input_rejects_oversized_and_invalid_utf8() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("evidence.sarif");
        std::fs::write(
            &path,
            vec![b'x'; usize::try_from(MAX_ANALYSIS_FILE_BYTES).unwrap() + 1],
        )
        .unwrap();
        assert!(read_bounded(&path).is_err());
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_bounded(&path).is_err());
        std::fs::write(&path, "{}\n").unwrap();
        assert_eq!(read_bounded(&path).unwrap(), "{}\n");
    }

    fn dummy_github() -> GitHubClient {
        GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: "https://api.github.com".into(),
            server_url: "https://github.com".into(),
        })
        .unwrap()
    }

    fn base_config() -> AnalysisConfig {
        AnalysisConfig {
            enabled: true,
            manifest: None,
            sarif_paths: vec![],
            max_findings: 10,
            require_current_head: true,
            check_runs: false,
            check_run_names: vec![],
        }
    }

    #[tokio::test]
    async fn malformed_sarif_is_not_treated_as_empty_evidence() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("malformed.sarif");
        std::fs::write(&path, "{}").unwrap();
        let mut config = base_config();
        config.sarif_paths = vec![path.to_string_lossy().into()];
        assert!(
            load_evidence(&config, &[], "head", &dummy_github())
                .await
                .is_err()
        );
        std::fs::write(&path, r#"{"runs":[]}"#).unwrap();
        assert!(
            load_evidence(&config, &[], "head", &dummy_github())
                .await
                .is_ok()
        );
    }

    struct AnnotationScm;

    #[async_trait::async_trait]
    impl ScmProvider for AnnotationScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }

        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((String::new(), false))
        }

        async fn list_finding_annotations(
            &self,
            _: &str,
            _: &[String],
        ) -> anyhow::Result<Vec<cururu_core::FindingAnnotation>> {
            Ok(vec![cururu_core::FindingAnnotation {
                path: "src/main.rs".into(),
                line: Some(7),
                severity: "warning".into(),
                message: "annotation".into(),
                title: Some("fixture".into()),
                details: None,
            }])
        }
    }

    #[tokio::test]
    async fn evidence_limit_is_never_silently_truncated() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("first.sarif");
        let second = directory.path().join("second.sarif");
        let raw = r#"{"runs":[{"tool":{"driver":{"name":"fixture"}},"results":[{"message":{"text":"finding"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/main.rs"},"region":{"startLine":7}}}]}]}]}"#;
        std::fs::write(&first, raw).unwrap();
        std::fs::write(&second, raw).unwrap();
        let changed = vec![ChangedFile {
            path: "src/main.rs".into(),
            patch: String::new(),
            right_lines: vec![7],
        }];
        let mut config = base_config();
        config.sarif_paths = vec![first.to_string_lossy().into()];
        config.max_findings = 1;
        assert_eq!(
            load_evidence(&config, &changed, "head", &AnnotationScm)
                .await
                .unwrap()
                .findings
                .len(),
            1
        );
        config.max_findings = 0;
        assert!(
            load_evidence(&config, &changed, "head", &AnnotationScm)
                .await
                .is_err()
        );
        config.max_findings = 1;
        config.sarif_paths.push(second.to_string_lossy().into());
        assert!(
            load_evidence(&config, &changed, "head", &AnnotationScm)
                .await
                .is_err()
        );
        config.sarif_paths.pop();
        config.check_runs = true;
        assert!(
            load_evidence(&config, &changed, "head", &AnnotationScm)
                .await
                .is_err()
        );
        config.max_findings = 2;
        assert_eq!(
            load_evidence(&config, &changed, "head", &AnnotationScm)
                .await
                .unwrap()
                .findings
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn parses_sarif_and_filters_to_changed_files() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("analysis.sarif");
        std::fs::write(
            &path,
            r#"{"runs":[{"tool":{"driver":{"name":"demo-linter"}},"results":[{"ruleId":"SEC001","level":"error","message":{"text":"Bad input"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/main.rs"},"region":{"startLine":7}}}]},{"ruleId":"OTHER","level":"warning","message":{"text":"Ignored"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/other.rs"},"region":{"startLine":2}}}]}]}]}"#,
        )
        .unwrap();
        let mut config = base_config();
        config.sarif_paths = vec![path.to_string_lossy().into()];
        let changed = vec![ChangedFile {
            path: "src/main.rs".into(),
            patch: String::new(),
            right_lines: vec![7],
        }];
        let report = load_evidence(&config, &changed, "head", &dummy_github())
            .await
            .unwrap();
        assert_eq!(report.status, "passed");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].severity, "high");
        assert_eq!(report.findings[0].rule.as_deref(), Some("SEC001"));
    }

    #[tokio::test]
    async fn reports_tool_failure_and_rejects_stale_manifest() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("analysis.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"commit_sha":"old","tools":[{"name":"compiler","status":"failed","exit_code":101,"message":"compilation failed"}]}"#,
        )
        .unwrap();
        let mut config = base_config();
        config.manifest = Some(path.to_string_lossy().into());
        let changed = Vec::new();
        let report = load_evidence(&config, &changed, "new", &dummy_github())
            .await
            .unwrap();
        assert_eq!(report.status, "stale");

        std::fs::write(
            &path,
            r#"{"schema_version":1,"commit_sha":"new","tools":[{"name":"compiler","status":"failed","exit_code":101,"message":"compilation failed"}]}"#,
        )
        .unwrap();
        let report = load_evidence(&config, &changed, "new", &dummy_github())
            .await
            .unwrap();
        assert_eq!(report.status, "failed");
        assert_eq!(report.tools[0].exit_code, Some(101));
    }

    #[test]
    fn annotations_are_filtered_to_changed_files() {
        let annotations = vec![
            crate::scm::FindingAnnotation {
                path: "src/main.rs".into(),
                line: Some(3),
                severity: "failure".into(),
                message: "panic".into(),
                title: Some("unwrap".into()),
                details: None,
            },
            crate::scm::FindingAnnotation {
                path: "src/other.rs".into(),
                line: Some(1),
                severity: "warning".into(),
                message: "x".into(),
                title: Some("clippy".into()),
                details: None,
            },
        ];
        let changed = vec![ChangedFile {
            path: "src/main.rs".into(),
            patch: String::new(),
            right_lines: vec![3],
        }];
        let findings = annotations_to_findings(&annotations, &changed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, "high");
        assert_eq!(findings[0].source.as_deref(), Some("check-runs"));
        assert_eq!(findings[0].line, Some(3));
    }
}
