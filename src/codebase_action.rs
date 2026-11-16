//! Whole-codebase review in the GitHub Action: review the committed files at
//! the workflow's commit and post the report as an issue, directly or, when
//! `publication.require_approval` is on, after the protected environment
//! approves exactly that report.

use crate::action_publication::{append, verified_approvers, write_candidate_file};
use crate::config::AppConfig;
use crate::github::GitHubClient;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

const KIND: &str = "codebase";

/// A codebase report waiting for publication approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodebaseCandidate {
    pub version: u32,
    pub kind: String,
    pub repository: String,
    pub revision: String,
    pub title: String,
    pub body: String,
}

impl CodebaseCandidate {
    /// SHA-256 of exactly what would be published.
    pub fn digest(&self) -> String {
        hex::encode(Sha256::digest(format!("{}\n\n{}", self.title, self.body)))
    }
}

/// GitHub accepts at most 65536 characters, not bytes, in an issue or
/// comment body; keep room for the heading the App adds.
fn fits_github_body(body: &str) -> bool {
    body.chars().count() <= 60_000
}

/// Whether the candidate file holds a codebase report.
pub fn is_codebase_candidate(path: &Path) -> anyhow::Result<bool> {
    let raw = std::fs::read(path).context("failed to read the review candidate")?;
    let value: serde_json::Value =
        serde_json::from_slice(&raw).context("review candidate is not JSON")?;
    Ok(value.get("kind").and_then(serde_json::Value::as_str) == Some(KIND))
}

/// Globs from `CURURU_CODEBASE_PATHS`, one per line or comma separated.
fn selected_paths() -> Vec<String> {
    std::env::var("CURURU_CODEBASE_PATHS")
        .unwrap_or_default()
        .split(['\n', ','])
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The commit to review: `CURURU_CODEBASE_REVISION`, else the workflow's.
fn revision() -> anyhow::Result<String> {
    std::env::var("CURURU_CODEBASE_REVISION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .map(|value| value.trim().to_owned())
        .context("set CURURU_CODEBASE_REVISION or run inside a GitHub workflow (GITHUB_SHA)")
}

/// Review the selected committed files at `revision`, read through `scm`
/// (fixed at that revision), and build the report to publish. Shared by the
/// Action and the App.
pub async fn build_report(
    config: &AppConfig,
    scm: &dyn crate::scm::ScmProvider,
    revision: &str,
    paths: &[String],
) -> anyhow::Result<CodebaseCandidate> {
    let prepared =
        crate::review_input::prepare_codebase(config, scm, revision.to_owned(), paths).await?;
    tracing::info!(
        files = prepared.files.len(),
        chunks = prepared.chunks.len(),
        revision,
        "reviewing the codebase"
    );
    let run = crate::review_with_api(config, &prepared).await?;
    let result = crate::review_input::assemble_review_with_unconfirmed(
        &prepared,
        &prepared.snapshot_sha256,
        run.reviews,
        &run.unconfirmed,
    )?;
    let link_base = format!(
        "{}/{}/blob/{revision}",
        config.source_control.web_url.trim_end_matches('/'),
        config.source_control.repository
    );
    let report = crate::output::render_report(&crate::output::ReportInput {
        review: &result,
        scope: prepared.scope,
        revision,
        coverage_unconfirmed: &run.coverage_unconfirmed,
        ignored_paths: &prepared.ignored_paths,
        unreviewable_paths: &prepared.unreviewable_paths,
        truncated_paths: &prepared.truncated_paths,
        dropped_findings: run.dropped_findings,
        link_base: Some(&link_base),
    });
    let selection = if paths.is_empty() {
        "all files".to_owned()
    } else {
        paths.join(", ")
    };
    let candidate = CodebaseCandidate {
        version: 1,
        kind: KIND.into(),
        repository: config.source_control.repository.clone(),
        revision: revision.to_owned(),
        title: format!(
            "Cururu codebase review: {} finding(s) at {}",
            result.findings.len(),
            &revision[..12]
        ),
        body: format!(
            "{report}\nSelection: {selection}.\n\n<!-- cururu:codebase-review:v1 revision={revision} -->\n"
        ),
    };
    anyhow::ensure!(
        fits_github_body(&candidate.body),
        "the codebase report is longer than GitHub accepts; select fewer paths or lower policy.max_findings"
    );
    Ok(candidate)
}

/// Review the codebase at the workflow's commit and publish or present the
/// report.
pub async fn review() -> anyhow::Result<()> {
    let mut config = AppConfig::from_repository_env()?;
    let github = GitHubClient::from_scm_config(&config.source_control)?;
    let revision = revision()?;
    let scm = crate::codebase::AtRevision::new(&github, revision.clone())?;
    crate::merge_repository_config(&mut config, &scm).await?;
    config.require_llm_api_key()?;
    let paths = selected_paths();
    let candidate = build_report(&config, &scm, &revision, &paths).await?;
    append(
        "GITHUB_STEP_SUMMARY",
        &format!("## {}\n\n{}\n", candidate.title, candidate.body),
    )?;
    if config.publication.require_approval {
        let path = crate::action_publication::candidate_path();
        write_candidate_file(&path, &serde_json::to_vec_pretty(&candidate)?)?;
        append(
            "GITHUB_OUTPUT",
            &format!(
                "review_status=pending_approval\ncandidate_digest={}\ncandidate_path={}\n",
                candidate.digest(),
                path.display()
            ),
        )?;
        println!(
            "Cururu: codebase report {} prepared; nothing was published. Approve the publication job to open the issue.",
            candidate.digest()
        );
        return Ok(());
    }
    let url = github
        .create_issue(&candidate.title, &candidate.body)
        .await?;
    append(
        "GITHUB_OUTPUT",
        &format!("review_status=completed\nissue_url={url}\n"),
    )?;
    println!("Cururu: codebase report published at {url}");
    Ok(())
}

/// Publish an approved codebase report from the Action's publication job.
pub async fn publish_approved_from_env() -> anyhow::Result<()> {
    let config = AppConfig::from_repository_env()?;
    let github = GitHubClient::from_scm_config(&config.source_control)?;
    let digest = std::env::var("CURURU_CANDIDATE_DIGEST").context(
        "CURURU_CANDIDATE_DIGEST is required; pass the review job's candidate_digest output",
    )?;
    publish_approved(
        &github,
        &crate::action_publication::candidate_path(),
        &digest,
    )
    .await
}

/// Open the issue for a codebase report approved through the protected
/// environment. The report is bound to its digest, so only the inspected
/// content is published.
pub async fn publish_approved(
    github: &GitHubClient,
    path: &Path,
    expected_digest: &str,
) -> anyhow::Result<()> {
    let candidate: CodebaseCandidate = serde_json::from_slice(
        &std::fs::read(path).context("failed to read the review candidate")?,
    )
    .context("codebase candidate is invalid")?;
    let digest = candidate.digest();
    anyhow::ensure!(
        digest == expected_digest.to_ascii_lowercase(),
        "the codebase report does not match the review job's digest; nothing was published"
    );
    let approvers = verified_approvers(github, &digest).await?;
    let url = github
        .create_issue(&candidate.title, &candidate.body)
        .await?;
    append(
        "GITHUB_OUTPUT",
        &format!("review_status=completed\nissue_url={url}\n"),
    )?;
    println!(
        "Cururu: codebase report {digest} approved by {} published at {url}",
        approvers.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_size_limit_counts_characters_not_bytes() {
        // 50000 two-byte characters: 100000 bytes, within the limit.
        assert!(super::fits_github_body(&"ç".repeat(50_000)));
        assert!(!super::fits_github_body(&"a".repeat(60_001)));
    }
}
