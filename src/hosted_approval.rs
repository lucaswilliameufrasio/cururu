//! CLI approval of a hosted publication candidate.
//!
//! The operator's own GitHub token reads the candidate check for the PR's
//! current head, the preview is verified against the digest it claims, shown,
//! and approved only after an interactive confirmation. App candidates are
//! approved with a `/cururu approve <digest>` comment authored by the operator;
//! Action candidates by approving the run's protected environment with a
//! comment naming the digest, which the publication job enforces.

use crate::config::AppConfig;
use crate::github::{CandidateCheckRun, GitHubClient};
use crate::hosted_publication::{ACTION_APPROVAL_PREFIX, APPROVAL_COMMAND, preview_digest};
use anyhow::Context;
use std::io::{IsTerminal, Write as _};

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedCandidate {
    pub digest: String,
    pub preview: String,
    /// Workflow run awaiting environment approval (Action candidates only).
    pub run_id: Option<u64>,
}

/// Keep only checks whose preview hashes to the digest they claim.
pub fn verified_candidates(checks: &[CandidateCheckRun]) -> Vec<VerifiedCandidate> {
    checks
        .iter()
        .filter_map(|check| {
            let preview = check.output.text.clone()?;
            let external = check.external_id.as_deref()?;
            let (claimed, run_id) = match external.split_once(":run=") {
                Some((digest, run)) => (digest, Some(run.parse().ok()?)),
                None => (external, None),
            };
            let digest = preview_digest(&preview);
            (claimed == digest).then_some(VerifiedCandidate {
                digest,
                preview,
                run_id,
            })
        })
        .collect()
}

fn config_for(pr: Option<u64>) -> anyhow::Result<AppConfig> {
    let Some(pr) = pr else {
        return AppConfig::from_env();
    };
    let mut config = AppConfig::from_local_env()?;
    anyhow::ensure!(
        !config.source_control.repository.is_empty(),
        "repository not configured; set CURURU_REPOSITORY or add an origin remote to this Git checkout"
    );
    config.source_control.token = std::env::var("CURURU_SCM_TOKEN")
        .ok()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .filter(|token| !token.is_empty())
        .context("set CURURU_SCM_TOKEN (or GITHUB_TOKEN) to your own GitHub token")?;
    config.source_control.change_request_number = pr;
    Ok(config)
}

pub async fn approve(pr: Option<u64>, environment: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        "approval requires an interactive terminal; nothing was approved"
    );
    let config = config_for(pr)?;
    let github = GitHubClient::from_scm_config(&config.source_control)?;
    let head = github.fetch_head_sha().await?;
    let mut checks = github.list_candidate_check_runs(&head).await?;
    checks.sort_by_key(|check| std::cmp::Reverse(check.id));
    let candidates = verified_candidates(&checks);
    let candidate = candidates.into_iter().next().context(
        "no inspectable Cururu candidate exists for the PR's current head; nothing was approved",
    )?;
    eprintln!(
        "Target: {} #{} @ {head}\nDigest: {}\n\n{}\n",
        config.source_control.repository,
        config.source_control.change_request_number,
        candidate.digest,
        candidate.preview
    );
    eprint!("Type APPROVE to publish this exact candidate, or anything else to cancel: ");
    std::io::stderr().flush()?;
    let mut confirmation = String::new();
    std::io::stdin().read_line(&mut confirmation)?;
    anyhow::ensure!(confirmation.trim_end() == "APPROVE", "approval cancelled");
    crate::review::ensure_review_head_unchanged(&head, &github.fetch_head_sha().await?)?;
    if let Some(run_id) = candidate.run_id {
        let pending = github.pending_deployments(run_id).await?;
        let deployment = pending
            .iter()
            .find(|deployment| deployment.environment.name == environment)
            .with_context(|| {
                format!("run {run_id} is not waiting for the `{environment}` environment")
            })?;
        anyhow::ensure!(
            deployment.current_user_can_approve,
            "your token cannot approve the `{environment}` environment"
        );
        github
            .approve_pending_deployment(
                run_id,
                deployment.environment.id,
                &format!("{ACTION_APPROVAL_PREFIX}{}", candidate.digest),
            )
            .await?;
        println!("Approved run {run_id}; its publication job publishes only this candidate.");
    } else {
        github
            .create_confirmed_comment_once(&format!("{APPROVAL_COMMAND} {}", candidate.digest))
            .await?;
        println!("Approval submitted. The Cururu App publishes after rechecking the candidate.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(external_id: &str, text: &str) -> CandidateCheckRun {
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "external_id": external_id,
            "output": {"text": text}
        }))
        .unwrap()
    }

    #[test]
    fn only_checks_whose_preview_matches_the_claimed_digest_are_offered() {
        let digest = preview_digest("preview");
        let verified = verified_candidates(&[
            check(&preview_digest("other"), "preview"),
            check(&format!("{digest}:run=42"), "preview"),
            check(&digest, "preview"),
            check(&format!("{digest}:run=x"), "preview"),
        ]);
        assert_eq!(
            verified,
            vec![
                VerifiedCandidate {
                    digest: digest.clone(),
                    preview: "preview".into(),
                    run_id: Some(42)
                },
                VerifiedCandidate {
                    digest,
                    preview: "preview".into(),
                    run_id: None
                },
            ]
        );
    }
}
