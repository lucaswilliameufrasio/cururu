//! Bounded snapshots for local and hosted review entry points.
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Context;
use cururu_core::{ChangedFile, DiffChunk, ReviewInputError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{config::AppConfig, diff, review, scm::ScmProvider};

#[derive(Debug, Serialize)]
pub struct PreparedReview {
    pub source: &'static str,
    pub revision: String,
    pub diff_sha256: String,
    /// Hash of the full effective snapshot, including filters and chunk layout.
    pub snapshot_sha256: String,
    pub scope: &'static str,
    pub ignored_paths: Vec<String>,
    pub review_instructions: &'static str,
    pub review_preferences: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_context: Option<cururu_core::ContextStore>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prior_feedback: Vec<cururu_core::PriorReviewFeedback>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub analyzer_evidence: Vec<cururu_core::ReviewFinding>,
    pub files: Vec<ChangedFile>,
    pub chunks: Vec<DiffChunk>,
    #[serde(skip)]
    pub policy: crate::config::ReviewPolicy,
}

#[derive(Deserialize)]
pub struct ChunkReview {
    pub chunk_index: usize,
    pub review: cururu_core::ReviewResult,
}

/// Reject input that cannot be represented as a complete text review.
pub fn ensure_reviewable(prepared: &PreparedReview) -> anyhow::Result<()> {
    anyhow::ensure!(
        !prepared.chunks.is_empty(),
        "no changes available for review"
    );
    anyhow::ensure!(
        !prepared
            .files
            .iter()
            .any(|file| file
                .patch
                .lines()
                .any(|line| line.starts_with("Binary files ")
                    || line == "GIT binary patch"
                    || line.starts_with("+Subproject commit ")
                    || line.starts_with("-Subproject commit "))),
        "binary files and submodule changes cannot be reviewed by this adapter"
    );
    Ok(())
}

/// Attach only explicitly supplied local files; never follow paths from a manifest.
pub fn attach_local_analysis(
    prepared: &mut PreparedReview,
    sarif: &Path,
    manifest: &Path,
) -> anyhow::Result<()> {
    let manifest_raw = crate::analysis::read_bounded(manifest)?;
    let manifest: cururu_core::AnalysisManifest = serde_json::from_str(&manifest_raw)?;
    anyhow::ensure!(
        manifest.has_supported_schema_version(),
        "unsupported analysis manifest version"
    );
    anyhow::ensure!(
        manifest.commit_sha.as_deref() == Some(prepared.revision.as_str()),
        "analysis manifest must match the reviewed revision"
    );
    let sarif_raw = crate::analysis::read_bounded(sarif)?;
    let findings = cururu_engine::sarif::parse_findings(&sarif_raw, &prepared.files, 1001)?;
    anyhow::ensure!(findings.len() <= 1000, "SARIF exceeds finding limit");
    anyhow::ensure!(
        serde_json::to_vec(&findings)?.len() <= 256 * 1024,
        "SARIF findings exceed byte limit"
    );
    let mut combined = prepared.analyzer_evidence.clone();
    combined.extend(findings);
    anyhow::ensure!(
        combined.len() <= 1000 && serde_json::to_vec(&combined)?.len() <= 256 * 1024,
        "combined analyzer evidence exceeds limit"
    );
    let bytes = serde_json::to_vec(&(&prepared.snapshot_sha256, &manifest_raw, &sarif_raw))?;
    prepared.snapshot_sha256 = format!("{:x}", Sha256::digest(bytes));
    prepared.analyzer_evidence = combined;
    Ok(())
}

/// Assemble only a complete set of results tied to the current effective input.
pub fn assemble_review(
    prepared: &PreparedReview,
    snapshot_sha256: &str,
    reviews: Vec<ChunkReview>,
) -> anyhow::Result<cururu_core::ReviewResult> {
    anyhow::ensure!(
        prepared.snapshot_sha256 == snapshot_sha256,
        "review snapshot changed"
    );
    ensure_reviewable(prepared)?;
    anyhow::ensure!(
        reviews.len() == prepared.chunks.len(),
        "review is incomplete"
    );
    let mut results = Vec::with_capacity(reviews.len());
    for (chunk, mut item) in prepared.chunks.iter().zip(reviews) {
        anyhow::ensure!(
            item.chunk_index == chunk.index,
            "chunk results are missing or out of order"
        );
        validate_chunk_review(prepared, chunk, &item.review)?;
        for finding in &mut item.review.findings {
            // A model cannot establish analyzer provenance.
            finding.source = None;
            finding.rule = None;
        }
        results.push(item.review);
    }
    let candidates = cururu_core::collect_review_candidates(
        "external-agent".into(),
        prepared.files.len(),
        results,
        prepared.analyzer_evidence.clone(),
        cururu_core::CandidateOptions {
            include_suggested_changes: prepared.policy.suggested_changes,
            synthesize: prepared.policy.synthesis,
        },
    );
    Ok(crate::agent::apply_policy(candidates, &prepared.policy))
}

/// Validate a model result against both the full diff and its specific chunk.
pub fn validate_chunk_review(
    prepared: &PreparedReview,
    chunk: &DiffChunk,
    review: &cururu_core::ReviewResult,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        review.files_reviewed == chunk.files.len(),
        "chunk review reports incomplete or inconsistent file coverage"
    );
    validate_findings(prepared, review)?;
    anyhow::ensure!(
        review
            .findings
            .iter()
            .all(|finding| chunk.files.contains(&finding.path)),
        "finding is outside its review chunk"
    );
    Ok(())
}

pub fn validate_findings(
    prepared: &PreparedReview,
    result: &cururu_core::ReviewResult,
) -> anyhow::Result<()> {
    anyhow::ensure!(result.findings.len() <= 1000, "too many findings");
    for finding in &result.findings {
        anyhow::ensure!(
            cururu_core::Severity::from_name(&finding.severity).is_some(),
            "invalid finding severity"
        );
        anyhow::ensure!(
            finding.confidence.is_finite() && (0.0..=1.0).contains(&finding.confidence),
            "invalid finding confidence"
        );
        anyhow::ensure!(
            !finding.title.trim().is_empty() && !finding.message.trim().is_empty(),
            "finding title and message are required"
        );
        let file = prepared
            .files
            .iter()
            .find(|file| file.path == finding.path)
            .context("finding references a file outside the review diff")?;
        if let Some(line) = finding.line {
            anyhow::ensure!(
                file.right_lines.contains(&line),
                "finding references a line outside the review diff"
            );
        }
    }
    Ok(())
}

/// Publication may include unchanged-line analyzer findings only when they
/// exactly match evidence collected again for the current snapshot.
pub fn validate_publication_findings(
    prepared: &PreparedReview,
    review: &cururu_core::ReviewResult,
) -> anyhow::Result<()> {
    let mut normalized = review.clone();
    let evidence = prepared
        .analyzer_evidence
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;
    for finding in &mut normalized.findings {
        let original = serde_json::to_value(&*finding)?;
        if evidence.contains(&original) {
            finding.line = None;
        }
    }
    validate_findings(prepared, &normalized)
}

fn prepare(
    config: &AppConfig,
    raw: &str,
    revision: String,
    source: &'static str,
) -> anyhow::Result<PreparedReview> {
    let headers = raw
        .lines()
        .filter(|line| line.starts_with("diff --git "))
        .count();
    let parsed = diff::parse_unified_diff(raw);
    anyhow::ensure!(
        headers == parsed.len() && (raw.is_empty() || headers > 0),
        "diff contains unsupported or malformed file headers"
    );
    let ignored_paths: Vec<String> = parsed
        .iter()
        .filter(|file| config.review.ignore.is_match(&file.path))
        .map(|file| file.path.clone())
        .collect();
    let files = diff::filter_ignored(parsed, &config.review.ignore);
    if let Err(error) = review::ensure_review_diff_limits(
        &files,
        config.review.chunk_bytes,
        config.review.max_diff_bytes,
    ) {
        if source == "pr" && error == ReviewInputError::DiffExceedsMaximum {
            return Err(review::DiffTotalLimit { head_sha: revision }.into());
        }
        return Err(error.into());
    }
    let chunks = diff::chunk_files(
        &files,
        config.review.chunk_bytes,
        config.review.max_diff_bytes,
    );
    let raw_hash = format!("{:x}", Sha256::digest(raw.as_bytes()));
    let preferences = serde_json::json!({
        "language": config.review.language,
        "tone": config.review.tone,
        "technical_level": config.review.technical_level,
        "suggestion_detail": config.review.suggestion_detail,
        "suggested_changes": config.review.policy.suggested_changes,
        "focus": config.review.policy.focus,
    });
    let snapshot = serde_json::to_vec(&(
        source,
        if source == "pr" {
            config.source_control.repository.as_str()
        } else {
            ""
        },
        if source == "pr" {
            config.source_control.api_url.as_str()
        } else {
            ""
        },
        if source == "pr" {
            config.source_control.change_request_number
        } else {
            0
        },
        &revision,
        &raw_hash,
        &ignored_paths,
        &files,
        &chunks,
        cururu_engine::DEFAULT_REVIEW_PROMPT,
        &preferences,
        (
            config.review.policy.minimum_confidence,
            config.review.policy.max_findings,
            &config.review.policy.allowed_severities,
            config.review.policy.synthesis,
            config.review.policy.suggested_changes,
        ),
    ))?;
    Ok(PreparedReview {
        source,
        revision,
        diff_sha256: raw_hash,
        snapshot_sha256: format!("{:x}", Sha256::digest(snapshot)),
        scope: if source == "local" {
            "tracked staged and unstaged changes against HEAD; excludes untracked files"
        } else {
            "hosted pull request diff"
        },
        ignored_paths,
        review_instructions: cururu_engine::DEFAULT_REVIEW_PROMPT,
        review_preferences: preferences,
        repository_context: None,
        prior_feedback: Vec::new(),
        analyzer_evidence: Vec::new(),
        files,
        chunks,
        policy: config.review.policy.clone(),
    })
}

pub async fn prepare_pr(
    config: &AppConfig,
    scm: &dyn ScmProvider,
) -> anyhow::Result<PreparedReview> {
    let head = scm.fetch_head_sha().await?;
    let (raw, truncated) = scm
        .fetch_diff_with_limit(config.review.max_diff_bytes.saturating_mul(2))
        .await
        .context("failed to fetch change-request diff")?;
    if truncated {
        return Err(review::DiffDownloadLimit { head_sha: head }.into());
    }
    review::ensure_review_head_unchanged(&head, &scm.fetch_head_sha().await?)?;
    prepare(config, &raw, head, "pr")
}

/// Add configured explicit base-revision context for agent-initiated PR reviews.
pub async fn prepare_agent_pr(
    config: &AppConfig,
    scm: &dyn ScmProvider,
) -> anyhow::Result<PreparedReview> {
    let mut prepared = prepare_pr(config, scm).await?;
    let mut context = review::fetch_repo_context(config, scm).await?;
    if config.context.auto.enabled {
        review::append_auto_context(config, scm, &mut context, &prepared.files).await?;
    }
    let feedback = scm.fetch_prior_review_feedback().await?;
    anyhow::ensure!(
        feedback.len() <= 1000,
        "prior review feedback exceeds entry limit"
    );
    let feedback_bytes = serde_json::to_vec(&feedback)?;
    anyhow::ensure!(
        feedback_bytes.len() <= 256 * 1024,
        "prior review feedback exceeds byte limit"
    );
    let evidence = if config.analysis.enabled && config.analysis.check_runs {
        let annotations = scm
            .list_finding_annotations(&prepared.revision, &config.analysis.check_run_names)
            .await?;
        let evidence = cururu_core::annotations_to_findings(&annotations, &prepared.files);
        anyhow::ensure!(
            evidence.len() <= config.analysis.max_findings,
            "analyzer evidence exceeds finding limit"
        );
        anyhow::ensure!(
            serde_json::to_vec(&evidence)?.len() <= 256 * 1024,
            "analyzer evidence exceeds byte limit"
        );
        evidence
    } else {
        Vec::new()
    };
    review::ensure_review_head_unchanged(&prepared.revision, &scm.fetch_head_sha().await?)?;
    let bytes = serde_json::to_vec(&(&prepared.snapshot_sha256, &context, &feedback, &evidence))?;
    prepared.snapshot_sha256 = format!("{:x}", Sha256::digest(bytes));
    prepared.repository_context = Some(context);
    prepared.prior_feedback = feedback;
    prepared.analyzer_evidence = evidence;
    Ok(prepared)
}

pub fn prepare_local(config: &AppConfig, root: &Path) -> anyhow::Result<PreparedReview> {
    let revision = git_head(root)?;
    let mut child = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "core.quotePath=false",
            "-c",
            "core.fsmonitor=false",
            "--no-pager",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "HEAD",
            "--",
        ])
        .stdin(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start git diff")?;
    let mut bytes = Vec::new();
    let limit = config.review.max_diff_bytes.saturating_mul(2);
    let read_result = child
        .stdout
        .take()
        .context("git diff stdout unavailable")?
        .take(u64::try_from(limit.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes);
    if read_result.is_err() || bytes.len() > limit {
        let _ = child.kill();
        let _ = child.wait();
        read_result.context("failed to read git diff")?;
        anyhow::bail!("local diff exceeds the configured review download limit");
    }
    anyhow::ensure!(
        child.wait()?.success(),
        "git diff failed; verify this repository has a HEAD commit"
    );
    let raw = String::from_utf8(bytes).context("local diff is not UTF-8")?;
    anyhow::ensure!(
        git_head(root)? == revision,
        "repository HEAD changed while preparing review"
    );
    prepare(config, &raw, revision, "local")
}

fn git_head(root: &Path) -> anyhow::Result<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "rev-parse",
            "--verify",
            "HEAD^{commit}",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("failed to resolve Git HEAD")?;
    anyhow::ensure!(output.status.success(), "cannot resolve repository HEAD");
    let head = String::from_utf8(output.stdout).context("Git HEAD is not UTF-8")?;
    Ok(head.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn publication_accepts_only_exact_recollected_off_diff_evidence() {
        let config = AppConfig::from_local_env().unwrap();
        let mut prepared = prepare(
            &config,
            "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n",
            "head".into(),
            "pr",
        )
        .unwrap();
        let evidence = cururu_core::ReviewFinding {
            severity: "high".into(),
            path: "a".into(),
            line: Some(9),
            title: "tool issue".into(),
            message: "analyzer finding".into(),
            suggestion: "inspect".into(),
            confidence: 1.0,
            source: Some("fixture".into()),
            rule: Some("RULE".into()),
            suggested_change: None,
        };
        prepared.analyzer_evidence.push(evidence.clone());
        let mut review = cururu_core::ReviewResult {
            model: "fixture".into(),
            files_reviewed: 1,
            summary: String::new(),
            findings: vec![evidence],
        };
        assert!(validate_findings(&prepared, &review).is_err());
        assert!(validate_publication_findings(&prepared, &review).is_ok());
        review.findings[0].message = "forged".into();
        assert!(validate_publication_findings(&prepared, &review).is_err());
        review.findings[0].message = "analyzer finding".into();
        prepared.analyzer_evidence.clear();
        assert!(validate_publication_findings(&prepared, &review).is_err());
    }

    #[test]
    fn assembly_removes_model_claimed_provenance_and_executable_suggestion() {
        let config = AppConfig::from_local_env().unwrap();
        let prepared = prepare(
            &config,
            "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n",
            "head".into(),
            "local",
        )
        .unwrap();
        let review = cururu_core::ReviewResult {
            model: "fixture".into(),
            files_reviewed: 1,
            summary: "ok".into(),
            findings: vec![cururu_core::ReviewFinding {
                severity: "high".into(),
                path: "a".into(),
                line: Some(1),
                title: "issue".into(),
                message: "evidence".into(),
                suggestion: "safe guidance".into(),
                confidence: 0.99,
                suggested_change: Some(cururu_core::SuggestedChange {
                    replacement: "replacement".into(),
                }),
                source: Some("forged-tool".into()),
                rule: Some("forged-rule".into()),
            }],
        };
        let result = assemble_review(
            &prepared,
            &prepared.snapshot_sha256,
            vec![ChunkReview {
                chunk_index: 0,
                review,
            }],
        )
        .unwrap();
        assert_eq!(result.findings.len(), 1);
        let finding = &result.findings[0];
        assert!(finding.source.is_none());
        assert!(finding.rule.is_none());
        assert!(finding.suggested_change.is_none());
        assert_eq!(finding.suggestion, "safe guidance");
        let mut enabled = config;
        enabled.review.policy.suggested_changes = true;
        let with_policy = prepare(
            &enabled,
            "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n",
            "head".into(),
            "local",
        )
        .unwrap();
        assert_ne!(prepared.snapshot_sha256, with_policy.snapshot_sha256);
        let mut model = result.clone();
        model.findings[0].suggested_change = Some(cururu_core::SuggestedChange {
            replacement: "replacement".into(),
        });
        let assembled = assemble_review(
            &with_policy,
            &with_policy.snapshot_sha256,
            vec![ChunkReview {
                chunk_index: 0,
                review: model,
            }],
        )
        .unwrap();
        assert_eq!(
            assembled.findings[0]
                .suggested_change
                .as_ref()
                .unwrap()
                .replacement,
            "replacement"
        );
    }

    #[test]
    fn chunk_review_rejects_inconsistent_reported_coverage() {
        let config = AppConfig::from_local_env().unwrap();
        let prepared = prepare(
            &config,
            "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n",
            "head".into(),
            "local",
        )
        .unwrap();
        let mut review = cururu_core::ReviewResult {
            model: "fixture".into(),
            files_reviewed: 0,
            summary: String::new(),
            findings: vec![],
        };
        assert!(validate_chunk_review(&prepared, &prepared.chunks[0], &review).is_err());
        review.files_reviewed = 2;
        assert!(validate_chunk_review(&prepared, &prepared.chunks[0], &review).is_err());
        review.files_reviewed = 1;
        assert!(validate_chunk_review(&prepared, &prepared.chunks[0], &review).is_ok());
    }

    #[test]
    fn explicit_sarif_requires_matching_manifest_and_binds_input() {
        let dir = tempfile::tempdir().unwrap();
        let sarif = dir.path().join("evidence.sarif");
        let manifest = dir.path().join("manifest.json");
        let raw = r#"{"runs":[{"tool":{"driver":{"name":"fixture"}},"results":[{"ruleId":"X","level":"warning","message":{"text":"issue"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"a"},"region":{"startLine":1}}}]}]}]}"#;
        std::fs::write(&sarif, raw).unwrap();
        std::fs::write(&manifest, r#"{"schema_version":1,"commit_sha":"head","tools":[{"name":"fixture","status":"passed","sarif_path":"/not-followed"}]}"#).unwrap();
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        let make = || {
            prepare(
                &config,
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n",
                "head".into(),
                "local",
            )
            .unwrap()
        };
        let mut first = make();
        attach_local_analysis(&mut first, &sarif, &manifest).unwrap();
        assert_eq!(first.analyzer_evidence.len(), 1);
        let review = cururu_core::ReviewResult {
            model: "fixture".into(),
            files_reviewed: 1,
            summary: String::new(),
            findings: vec![],
        };
        let assembled = assemble_review(
            &first,
            &first.snapshot_sha256,
            vec![ChunkReview {
                chunk_index: 0,
                review,
            }],
        )
        .unwrap();
        assert_eq!(assembled.findings.len(), 1);
        assert_eq!(assembled.findings[0].source.as_deref(), Some("fixture"));
        std::fs::write(&sarif, "{}").unwrap();
        let mut malformed = make();
        assert!(attach_local_analysis(&mut malformed, &sarif, &manifest).is_err());
        std::fs::write(&sarif, raw).unwrap();
        let mut full = make();
        full.analyzer_evidence = vec![first.analyzer_evidence[0].clone(); 1000];
        let original_hash = full.snapshot_sha256.clone();
        assert!(attach_local_analysis(&mut full, &sarif, &manifest).is_err());
        assert_eq!(full.snapshot_sha256, original_hash);
        assert_eq!(full.analyzer_evidence.len(), 1000);
        std::fs::write(&sarif, raw.replace("issue", "changed")).unwrap();
        let mut changed = make();
        attach_local_analysis(&mut changed, &sarif, &manifest).unwrap();
        assert_ne!(first.snapshot_sha256, changed.snapshot_sha256);
        std::fs::write(
            &manifest,
            r#"{"schema_version":1,"commit_sha":"other","tools":[]}"#,
        )
        .unwrap();
        assert!(attach_local_analysis(&mut make(), &sarif, &manifest).is_err());
        std::fs::write(&manifest, r#"{"schema_version":1,"tools":[]}"#).unwrap();
        assert!(attach_local_analysis(&mut make(), &sarif, &manifest).is_err());
    }

    struct ContextScm(&'static str);
    struct AutoScm(&'static str);
    struct EvidenceScm(&'static str);
    #[async_trait::async_trait]
    impl ScmProvider for EvidenceScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
        async fn list_finding_annotations(
            &self,
            revision: &str,
            _: &[String],
        ) -> anyhow::Result<Vec<cururu_core::FindingAnnotation>> {
            assert_eq!(revision, "head");
            Ok(vec![cururu_core::FindingAnnotation {
                path: "a".into(),
                line: Some(1),
                severity: "warning".into(),
                title: Some("Tool".into()),
                message: self.0.into(),
                details: None,
            }])
        }
    }

    #[tokio::test]
    async fn check_run_evidence_is_opt_in_bounded_and_snapshot_bound() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        let disabled = prepare_agent_pr(&config, &EvidenceScm("one"))
            .await
            .unwrap();
        assert!(disabled.analyzer_evidence.is_empty());
        config.analysis.enabled = true;
        config.analysis.check_runs = true;
        config.analysis.max_findings = 5;
        let first = prepare_agent_pr(&config, &EvidenceScm("one"))
            .await
            .unwrap();
        assert_eq!(first.analyzer_evidence.len(), 1);
        let second = prepare_agent_pr(&config, &EvidenceScm("two"))
            .await
            .unwrap();
        assert_ne!(first.snapshot_sha256, second.snapshot_sha256);
        config.analysis.max_findings = 0;
        assert!(
            prepare_agent_pr(&config, &EvidenceScm("one"))
                .await
                .is_err()
        );
    }
    struct FeedbackScm<'a>(&'a str);
    #[async_trait::async_trait]
    impl ScmProvider for FeedbackScm<'_> {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
        async fn fetch_prior_review_feedback(
            &self,
        ) -> anyhow::Result<Vec<cururu_core::PriorReviewFeedback>> {
            Ok(vec![cururu_core::PriorReviewFeedback {
                location: "a:1".into(),
                comments: vec![cururu_core::PriorReviewComment {
                    author: "reviewer".into(),
                    body: self.0.into(),
                }],
            }])
        }
    }

    #[tokio::test]
    async fn prior_feedback_is_bounded_and_invalidates_snapshot() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        let first = prepare_agent_pr(&config, &FeedbackScm("first"))
            .await
            .unwrap();
        assert_eq!(first.prior_feedback[0].comments[0].body, "first");
        let changed = prepare_agent_pr(&config, &FeedbackScm("changed"))
            .await
            .unwrap();
        assert_ne!(first.snapshot_sha256, changed.snapshot_sha256);
        let oversized = "x".repeat(256 * 1024);
        assert!(
            prepare_agent_pr(&config, &FeedbackScm(&oversized))
                .await
                .is_err()
        );
    }
    #[async_trait::async_trait]
    impl ScmProvider for AutoScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_base_sha(&self) -> anyhow::Result<String> {
            Ok("base".into())
        }
        async fn list_repository_paths_at_revision(
            &self,
            revision: &str,
        ) -> anyhow::Result<Vec<String>> {
            assert_eq!(revision, "base");
            Ok(vec!["src/a".into()])
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/src/a b/src/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
        async fn fetch_file_at_ref(&self, path: &str, revision: &str) -> anyhow::Result<String> {
            assert_eq!((path, revision), ("src/a", "base"));
            Ok(self.0.into())
        }
    }

    #[tokio::test]
    async fn agent_auto_context_obeys_opt_in_limits_and_invalidates_snapshot() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        let disabled = prepare_agent_pr(&config, &AutoScm("xéz")).await.unwrap();
        assert!(disabled.repository_context.unwrap().files.is_empty());
        config.context.auto.enabled = true;
        config.context.auto.max_bytes = 2;
        config.context.auto.per_file_bytes = 2;
        let first = prepare_agent_pr(&config, &AutoScm("xéz")).await.unwrap();
        let store = first.repository_context.as_ref().unwrap();
        assert_eq!(store.files[0].content, "x");
        assert_eq!(store.truncated, ["src/a"]);
        let changed = prepare_agent_pr(&config, &AutoScm("yéz")).await.unwrap();
        assert_ne!(first.snapshot_sha256, changed.snapshot_sha256);
    }

    struct AutoFailureScm {
        present: bool,
        tree_fails: bool,
    }

    #[async_trait::async_trait]
    impl ScmProvider for AutoFailureScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_base_sha(&self) -> anyhow::Result<String> {
            Ok("base".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/src/a b/src/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
        async fn list_repository_paths_at_revision(&self, _: &str) -> anyhow::Result<Vec<String>> {
            anyhow::ensure!(!self.tree_fails, "fixture tree failure");
            Ok(if self.present {
                vec!["src/a".into()]
            } else {
                vec![]
            })
        }
        async fn fetch_file_at_ref(&self, _: &str, _: &str) -> anyhow::Result<String> {
            anyhow::bail!("private fixture failure")
        }
    }

    #[tokio::test]
    async fn auto_context_distinguishes_missing_base_file_from_read_or_tree_failure() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        config.context.auto.enabled = true;
        let missing = prepare_agent_pr(
            &config,
            &AutoFailureScm {
                present: false,
                tree_fails: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(missing.repository_context.unwrap().skipped, ["src/a"]);
        let error = prepare_agent_pr(
            &config,
            &AutoFailureScm {
                present: true,
                tree_fails: false,
            },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("failed to fetch automatic base context"));
        assert!(!error.contains("private fixture failure"));
        assert!(
            prepare_agent_pr(
                &config,
                &AutoFailureScm {
                    present: false,
                    tree_fails: true
                }
            )
            .await
            .is_err()
        );
    }
    #[async_trait::async_trait]
    impl ScmProvider for ContextScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_base_sha(&self) -> anyhow::Result<String> {
            Ok("base".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
        async fn list_repository_paths_at_revision(
            &self,
            revision: &str,
        ) -> anyhow::Result<Vec<String>> {
            assert_eq!(revision, "base");
            Ok(vec!["rules.md".into()])
        }
        async fn fetch_file_at_ref(&self, path: &str, revision: &str) -> anyhow::Result<String> {
            assert_eq!((path, revision), ("rules.md", "base"));
            Ok(self.0.into())
        }
    }

    #[tokio::test]
    async fn agent_pr_context_is_bounded_and_bound_to_snapshot() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        config.context.additional = vec!["rules.md".into()];
        config.context.max_bytes = 4;
        let first = prepare_agent_pr(&config, &ContextScm("abcdef"))
            .await
            .unwrap();
        let context = first.repository_context.as_ref().unwrap();
        assert_eq!(context.files[0].content, "abcd");
        assert_eq!(context.truncated, ["rules.md"]);
        let changed = prepare_agent_pr(&config, &ContextScm("wxyzef"))
            .await
            .unwrap();
        assert_ne!(first.snapshot_sha256, changed.snapshot_sha256);
    }

    struct FakePr {
        calls: AtomicUsize,
        changing: bool,
    }

    #[async_trait::async_trait]
    impl ScmProvider for FakePr {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(if self.changing && n > 0 {
                "other"
            } else {
                "head"
            }
            .into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn hosted_preparation_binds_head_and_enforces_limits() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        let scm = FakePr {
            calls: AtomicUsize::new(0),
            changing: false,
        };
        let prepared = prepare_pr(&config, &scm).await.unwrap();
        assert_eq!(prepared.revision, "head");
        assert_eq!(prepared.files[0].path, "a");
        assert_eq!(prepared.chunks.len(), 1);
        let mut other_host = config.clone();
        other_host.source_control.api_url = "https://other.example.invalid".into();
        let other_snapshot = prepare_pr(
            &other_host,
            &FakePr {
                calls: AtomicUsize::new(0),
                changing: false,
            },
        )
        .await
        .unwrap();
        assert_ne!(prepared.snapshot_sha256, other_snapshot.snapshot_sha256);
        let mut changed_preferences = config.clone();
        changed_preferences.review.language = "en-US".into();
        let preference_snapshot = prepare_pr(
            &changed_preferences,
            &FakePr {
                calls: AtomicUsize::new(0),
                changing: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(preference_snapshot.review_preferences["language"], "en-US");
        assert_ne!(
            prepared.snapshot_sha256,
            preference_snapshot.snapshot_sha256
        );
        let mut result = cururu_core::ReviewResult {
            model: "agent".into(),
            files_reviewed: 1,
            summary: String::new(),
            findings: vec![cururu_core::ReviewFinding {
                severity: "high".into(),
                path: "a".into(),
                line: Some(1),
                title: "Issue".into(),
                message: "Evidence".into(),
                suggestion: String::new(),
                confidence: 0.8,
                suggested_change: None,
                source: None,
                rule: None,
            }],
        };
        assert!(validate_findings(&prepared, &result).is_ok());
        let assembled = assemble_review(
            &prepared,
            &prepared.snapshot_sha256,
            vec![ChunkReview {
                chunk_index: 0,
                review: result.clone(),
            }],
        )
        .unwrap();
        assert_eq!(assembled.findings.len(), 1);
        let hosted_candidates = crate::agent::collect_candidates(
            "fixture".into(),
            prepared.files.len(),
            vec![cururu_engine::ChunkResult {
                review: result.clone(),
                usage: None,
            }],
            &config.review.policy,
            vec![],
        );
        let hosted = crate::agent::apply_policy(hosted_candidates, &config.review.policy);
        assert_eq!(
            serde_json::to_value(&assembled.findings).unwrap(),
            serde_json::to_value(&hosted.findings).unwrap()
        );
        let mut strict = config.clone();
        strict.review.policy.minimum_confidence = 0.95;
        let strict_input = prepare_pr(
            &strict,
            &FakePr {
                calls: AtomicUsize::new(0),
                changing: false,
            },
        )
        .await
        .unwrap();
        assert_ne!(prepared.snapshot_sha256, strict_input.snapshot_sha256);
        assert!(
            assemble_review(
                &strict_input,
                &prepared.snapshot_sha256,
                vec![ChunkReview {
                    chunk_index: 0,
                    review: result.clone()
                }]
            )
            .is_err()
        );
        let filtered = assemble_review(
            &strict_input,
            &strict_input.snapshot_sha256,
            vec![ChunkReview {
                chunk_index: 0,
                review: result.clone(),
            }],
        )
        .unwrap();
        assert!(filtered.findings.is_empty());
        let strict_hosted = crate::agent::apply_policy(
            crate::agent::collect_candidates(
                "fixture".into(),
                strict_input.files.len(),
                vec![cururu_engine::ChunkResult {
                    review: result.clone(),
                    usage: None,
                }],
                &strict.review.policy,
                vec![],
            ),
            &strict.review.policy,
        );
        assert!(strict_hosted.findings.is_empty());
        assert!(
            assemble_review(
                &prepared,
                "stale",
                vec![ChunkReview {
                    chunk_index: 0,
                    review: result.clone()
                }]
            )
            .is_err()
        );
        assert!(assemble_review(&prepared, &prepared.snapshot_sha256, vec![]).is_err());
        assert!(
            assemble_review(
                &prepared,
                &prepared.snapshot_sha256,
                vec![ChunkReview {
                    chunk_index: 1,
                    review: result.clone()
                }]
            )
            .is_err()
        );
        result.findings[0].line = Some(99);
        assert!(validate_findings(&prepared, &result).is_err());
        result.findings[0].line = None;
        result.findings[0].path = "unrelated".into();
        assert!(validate_findings(&prepared, &result).is_err());
        let changed = FakePr {
            calls: AtomicUsize::new(0),
            changing: true,
        };
        assert!(prepare_pr(&config, &changed).await.is_err());
        config.review.max_diff_bytes = 10;
        assert!(
            prepare_pr(
                &config,
                &FakePr {
                    calls: AtomicUsize::new(0),
                    changing: false
                }
            )
            .await
            .unwrap_err()
            .downcast_ref::<review::DiffTotalLimit>()
            .is_some()
        );
    }

    #[test]
    fn local_preparation_reads_tracked_changes_not_untracked_files() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        };
        run(&["init", "-q"]);
        std::fs::write(dir.path().join("a.txt"), "old\n").unwrap();
        run(&["add", "a.txt"]);
        run(&[
            "-c",
            "user.name=Cururu Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ]);
        std::fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
        run(&["add", "a.txt"]);
        std::fs::write(dir.path().join("a.txt"), "unstaged\n").unwrap();
        std::fs::write(dir.path().join("untracked.txt"), "private\n").unwrap();
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        let result = prepare_local(&config, dir.path()).unwrap();
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].patch.contains("+unstaged"));
        assert!(!result.files[0].patch.contains("private"));
        assert_eq!(result.diff_sha256.len(), 64);
        config.review.max_diff_bytes = 5;
        assert!(prepare_local(&config, dir.path()).is_err());
        assert!(
            prepare(
                &config,
                "diff --git \"a/a\" \"b/a\"\n",
                "head".into(),
                "local"
            )
            .is_err()
        );
    }
}
