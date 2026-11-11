//! Explicit, separately confirmed publication of an agent-review receipt.
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

use crate::{config::AppConfig, review_input, scm::ScmProvider};

const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
const MAX_COMMENT_BYTES: usize = 60_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    source: String,
    revision: String,
    diff_sha256: String,
    snapshot_sha256: String,
    scope: String,
    ignored_paths: Vec<String>,
    review: cururu_core::ReviewResult,
    /// Informational provider metadata is never used to authorize publication.
    #[serde(default, rename = "usage")]
    _usage: Option<serde::de::IgnoredAny>,
}

pub fn read_receipt(path: &Path) -> anyhow::Result<Receipt> {
    let file = std::fs::File::open(path).context("failed to open review receipt")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "review receipt must be a regular file"
    );
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_RECEIPT_BYTES + 1)?)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_RECEIPT_BYTES,
        "review receipt exceeds size limit"
    );
    serde_json::from_slice(&bytes).context("invalid review receipt")
}

pub struct Preview {
    pub body: String,
    snapshot_sha256: String,
    revision: String,
    evidence: Option<LocalEvidence>,
}

#[derive(Clone)]
pub struct LocalEvidence {
    pub sarif: std::path::PathBuf,
    pub manifest: std::path::PathBuf,
}

async fn prepare_current(
    config: &AppConfig,
    scm: &dyn ScmProvider,
    evidence: Option<&LocalEvidence>,
) -> anyhow::Result<review_input::PreparedReview> {
    let mut current = review_input::prepare_agent_pr(config, scm).await?;
    review_input::ensure_reviewable(&current)?;
    if let Some(evidence) = evidence {
        review_input::attach_local_analysis(&mut current, &evidence.sarif, &evidence.manifest)?;
    }
    Ok(current)
}

pub fn target(config: &AppConfig) -> anyhow::Result<String> {
    let url = url::Url::parse(&config.source_control.web_url)
        .context("invalid source-control web URL")?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "unsafe source-control web URL"
    );
    Ok(format!(
        "{}/{}/pull/{}",
        url.as_str().trim_end_matches('/'),
        config.source_control.repository,
        config.source_control.change_request_number
    ))
}

pub async fn preview(
    config: &AppConfig,
    scm: &dyn ScmProvider,
    receipt: &Receipt,
    evidence: Option<&LocalEvidence>,
) -> anyhow::Result<Preview> {
    let current = prepare_current(config, scm, evidence).await?;
    anyhow::ensure!(
        receipt.source == "pr"
            && receipt.revision == current.revision
            && receipt.diff_sha256 == current.diff_sha256
            && receipt.snapshot_sha256 == current.snapshot_sha256
            && receipt.scope == current.scope
            && receipt.ignored_paths == current.ignored_paths,
        "receipt does not match the current PR snapshot"
    );
    anyhow::ensure!(
        !current.chunks.is_empty() && receipt.review.files_reviewed == current.files.len(),
        "receipt does not cover the current review scope"
    );
    review_input::validate_publication_findings(&current, &receipt.review)?;
    let mut body = format!(
        "## Cururu advisory review\n\nPR: {} #{}  \nRevision: `{}`  \nSnapshot: `{}`\n\nThis is advisory, not an approval. Automated findings require human review. The receipt reports the configured diff scope, but does not prove that an agent reviewed it; ignored paths are listed below.\n\n",
        config.source_control.repository,
        config.source_control.change_request_number,
        current.revision,
        current.snapshot_sha256
    );
    if current.ignored_paths.is_empty() {
        body.push_str("Ignored paths: none.\n\n");
    } else {
        body.push_str("Ignored paths:\n<pre>");
        for path in &current.ignored_paths {
            writeln!(body, "{}", safe_text(path))?;
        }
        body.push_str("</pre>\n\n");
    }
    if receipt.review.findings.is_empty() {
        body.push_str("No findings were reported for this scope. This is not an approval.\n");
    }
    for finding in &receipt.review.findings {
        let line = finding
            .line
            .map_or_else(|| "file-level".to_string(), |number| number.to_string());
        writeln!(
            body,
            "### Finding\n<pre>Title: {}\nFile: {}\nLine: {}\nSeverity: {}\n\n{}\n\nSuggested action: {}\n</pre>",
            safe_text(&finding.title),
            safe_text(&finding.path),
            line,
            safe_text(&finding.severity),
            safe_text(&finding.message),
            safe_text(&finding.suggestion)
        )?;
        if current.policy.suggested_changes
            && let Some(change) = &finding.suggested_change
            && !change.replacement.is_empty()
            && !change.replacement.contains('\n')
            && !change.replacement.contains("```")
        {
            writeln!(
                body,
                "Suggested replacement (advisory; not applied):\n<pre>{}</pre>",
                safe_text(&change.replacement)
            )?;
        }
        anyhow::ensure!(
            body.len() <= MAX_COMMENT_BYTES,
            "review comment exceeds size limit"
        );
    }
    anyhow::ensure!(
        body.len() <= MAX_COMMENT_BYTES,
        "review comment exceeds size limit"
    );
    Ok(Preview {
        body,
        snapshot_sha256: current.snapshot_sha256,
        revision: current.revision,
        evidence: evidence.cloned(),
    })
}

fn safe_text(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        match character {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '@' => result.push_str("&#64;"),
            '\n' | '\t' => result.push(character),
            character if character.is_control() => result.push(' '),
            _ => result.push(character),
        }
    }
    result
}

/// Call only after a human explicitly confirmed this exact preview.
pub async fn publish(
    config: &AppConfig,
    scm: &dyn ScmProvider,
    preview: &Preview,
) -> anyhow::Result<()> {
    let current = prepare_current(config, scm, preview.evidence.as_ref()).await?;
    anyhow::ensure!(
        current.snapshot_sha256 == preview.snapshot_sha256 && current.revision == preview.revision,
        "PR changed after confirmation; no comment was published"
    );
    scm.create_confirmed_comment_once(&preview.body)
        .await
        .context("failed to publish review comment")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeScm {
        diff: Mutex<String>,
        posted: Mutex<Vec<String>>,
        fail: bool,
    }

    #[tokio::test]
    async fn preview_obeys_suggestion_policy_and_escapes_replacements() {
        for enabled in [false, true] {
            let mut config = AppConfig::from_local_env().unwrap();
            config.review.policy.suggested_changes = enabled;
            let scm = FakeScm {
                diff: Mutex::new("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into()),
                posted: Mutex::new(vec![]),
                fail: false,
            };
            let input = review_input::prepare_agent_pr(&config, &scm).await.unwrap();
            let receipt = Receipt {
                _usage: None,
                source: "pr".into(),
                revision: input.revision,
                diff_sha256: input.diff_sha256,
                snapshot_sha256: input.snapshot_sha256,
                scope: input.scope.into(),
                ignored_paths: input.ignored_paths,
                review: cururu_core::ReviewResult {
                    model: "fixture".into(),
                    files_reviewed: 1,
                    summary: String::new(),
                    findings: vec![cururu_core::ReviewFinding {
                        severity: "high".into(),
                        path: "a".into(),
                        line: Some(1),
                        title: "fixture".into(),
                        message: "issue".into(),
                        suggestion: "guidance".into(),
                        confidence: 0.99,
                        source: None,
                        rule: None,
                        suggested_change: Some(cururu_core::SuggestedChange {
                            replacement: "</pre>@everyone & fix()".into(),
                        }),
                    }],
                },
            };
            let preview = preview(&config, &scm, &receipt, None).await.unwrap();
            assert_eq!(
                preview
                    .body
                    .contains("&lt;/pre&gt;&#64;everyone &amp; fix()"),
                enabled
            );
            assert!(!preview.body.contains("</pre>@everyone"));
            assert!(scm.posted.lock().unwrap().is_empty());
            publish(&config, &scm, &preview).await.unwrap();
            assert_eq!(scm.posted.lock().unwrap()[0], preview.body);
        }
    }

    #[tokio::test]
    async fn forged_receipt_cannot_publish_unreviewable_diff() {
        let config = AppConfig::from_local_env().unwrap();
        for diff in [
            "diff --git a/a b/a\nBinary files a/a and b/a differ\n",
            "diff --git a/a b/a\n@@ -1 +0,0 @@\n-Subproject commit old\n",
        ] {
            let scm = FakeScm {
                diff: Mutex::new(diff.into()),
                posted: Mutex::new(vec![]),
                fail: false,
            };
            let input = review_input::prepare_agent_pr(&config, &scm).await.unwrap();
            let receipt = Receipt {
                _usage: None,
                source: "pr".into(),
                revision: input.revision,
                diff_sha256: input.diff_sha256,
                snapshot_sha256: input.snapshot_sha256.clone(),
                scope: input.scope.into(),
                ignored_paths: input.ignored_paths,
                review: cururu_core::ReviewResult {
                    model: "fixture".into(),
                    files_reviewed: 1,
                    summary: String::new(),
                    findings: vec![],
                },
            };
            assert!(preview(&config, &scm, &receipt, None).await.is_err());
            let forged_preview = Preview {
                body: "fixture".into(),
                snapshot_sha256: input.snapshot_sha256,
                revision: "revision".into(),
                evidence: None,
            };
            assert!(publish(&config, &scm, &forged_preview).await.is_err());
            assert!(scm.posted.lock().unwrap().is_empty());
        }
    }

    #[async_trait::async_trait]
    impl ScmProvider for FakeScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("revision".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((self.diff.lock().unwrap().clone(), false))
        }
        async fn create_confirmed_comment_once(&self, body: &str) -> anyhow::Result<()> {
            anyhow::ensure!(!self.fail, "synthetic failure");
            self.posted.lock().unwrap().push(body.to_owned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn local_evidence_is_required_and_rechecked_before_posting() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        let scm = FakeScm {
            diff: Mutex::new("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into()),
            posted: Mutex::new(vec![]),
            fail: false,
        };
        let dir = tempfile::tempdir().unwrap();
        let evidence = LocalEvidence {
            sarif: dir.path().join("results.sarif"),
            manifest: dir.path().join("manifest.json"),
        };
        let sarif = r#"{"runs":[]}"#;
        let manifest = r#"{"schema_version":1,"commit_sha":"revision","tools":[]}"#;
        std::fs::write(&evidence.sarif, sarif).unwrap();
        std::fs::write(&evidence.manifest, manifest).unwrap();
        let input = prepare_current(&config, &scm, Some(&evidence))
            .await
            .unwrap();
        let receipt = Receipt {
            _usage: None,
            source: "pr".into(),
            revision: input.revision,
            diff_sha256: input.diff_sha256,
            snapshot_sha256: input.snapshot_sha256,
            scope: input.scope.into(),
            ignored_paths: input.ignored_paths,
            review: cururu_core::ReviewResult {
                model: "fixture".into(),
                files_reviewed: 1,
                summary: String::new(),
                findings: vec![],
            },
        };
        assert!(preview(&config, &scm, &receipt, None).await.is_err());
        let approved = preview(&config, &scm, &receipt, Some(&evidence))
            .await
            .unwrap();
        std::fs::write(&evidence.sarif, "{\"runs\":[],\"changed\":true}").unwrap();
        assert!(publish(&config, &scm, &approved).await.is_err());
        assert!(scm.posted.lock().unwrap().is_empty());
        std::fs::write(&evidence.sarif, sarif).unwrap();
        std::fs::write(
            &evidence.manifest,
            r#"{"schema_version":1,"commit_sha":"other","tools":[]}"#,
        )
        .unwrap();
        assert!(publish(&config, &scm, &approved).await.is_err());
        assert!(scm.posted.lock().unwrap().is_empty());
        std::fs::write(&evidence.manifest, manifest).unwrap();
        publish(&config, &scm, &approved).await.unwrap();
        assert_eq!(scm.posted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn preview_and_publication_reject_stale_or_unrelated_receipts() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.max_diff_bytes = 1000;
        config.review.chunk_bytes = 1000;
        config.source_control.repository = "owner/repo".into();
        config.source_control.change_request_number = 42;
        let scm = FakeScm {
            diff: Mutex::new("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into()),
            posted: Mutex::new(vec![]),
            fail: false,
        };
        let input = review_input::prepare_agent_pr(&config, &scm).await.unwrap();
        let mut receipt = Receipt {
            _usage: None,
            source: "pr".into(),
            revision: input.revision,
            diff_sha256: input.diff_sha256,
            snapshot_sha256: input.snapshot_sha256,
            scope: input.scope.into(),
            ignored_paths: input.ignored_paths,
            review: cururu_core::ReviewResult {
                model: "agent".into(),
                files_reviewed: 1,
                summary: String::new(),
                findings: vec![cururu_core::ReviewFinding {
                    severity: "high".into(),
                    path: "a".into(),
                    line: Some(1),
                    title: "</pre>@everyone".into(),
                    message: "<script>".into(),
                    suggestion: "fix".into(),
                    confidence: 0.9,
                    suggested_change: None,
                    source: None,
                    rule: None,
                }],
            },
        };
        let approved = preview(&config, &scm, &receipt, None).await.unwrap();
        assert!(approved.body.contains("&lt;/pre&gt;&#64;everyone"));
        assert!(!approved.body.contains("<script>"));
        assert!(approved.body.contains("not an approval"));
        assert!(
            approved
                .body
                .contains("does not prove that an agent reviewed it")
        );
        assert!(target(&config).unwrap().ends_with("/owner/repo/pull/42"));
        receipt.source = "local".into();
        assert!(preview(&config, &scm, &receipt, None).await.is_err());
        receipt.source = "pr".into();
        receipt.review.findings[0].line = Some(99);
        assert!(preview(&config, &scm, &receipt, None).await.is_err());
        *scm.diff.lock().unwrap() = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+changed\n".into();
        assert!(publish(&config, &scm, &approved).await.is_err());
        assert!(scm.posted.lock().unwrap().is_empty());
        *scm.diff.lock().unwrap() = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into();
        let failing = FakeScm {
            diff: Mutex::new(scm.diff.lock().unwrap().clone()),
            posted: Mutex::new(vec![]),
            fail: true,
        };
        assert!(publish(&config, &failing, &approved).await.is_err());
        assert!(failing.posted.lock().unwrap().is_empty());
        publish(&config, &scm, &approved).await.unwrap();
        assert_eq!(scm.posted.lock().unwrap().as_slice(), &[approved.body]);
    }
}
