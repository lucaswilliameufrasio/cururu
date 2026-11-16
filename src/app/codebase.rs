//! `/cururu review codebase [globs...]` in an issue: review the committed
//! files at the default branch's head and answer in that issue, directly or,
//! when `publication.require_approval` is on, after a separate approval of
//! exactly that report (check button or `/cururu approve <digest>`).

use super::{AppState, StoredCandidate, github_context, merge_shared_repository_config};
use crate::codebase::AtRevision;
use crate::codebase_action::CodebaseCandidate;
use crate::config::AppConfig;
use crate::github::GitHubClient;
use crate::scm::{CandidateCheck, ScmProvider};
use anyhow::Context;
use serde_json::Value;
use tracing::{info, warn};

const COMMAND: &str = "/cururu review codebase";

/// The globs of a codebase review command, or `None` when `body` is not one.
pub fn parse_command(body: &str) -> Option<Vec<String>> {
    let rest = body.trim().strip_prefix(COMMAND)?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let paths: Vec<String> = rest.split_whitespace().map(str::to_owned).collect();
    (paths.len() <= 50).then_some(paths)
}

/// Whether a stored candidate is a codebase report.
pub fn is_codebase_candidate(stored: &StoredCandidate) -> bool {
    serde_json::from_str::<Value>(&stored.candidate)
        .is_ok_and(|value| value.get("kind").and_then(Value::as_str) == Some("codebase"))
}

/// Configuration for an issue event, read from `revision` (the default
/// branch's head when `None`); `None` when `login` lacks write access.
async fn config_at(
    state: &AppState,
    payload: &Value,
    issue: u64,
    login: &str,
    revision: Option<String>,
) -> anyhow::Result<Option<(AppConfig, GitHubClient, String)>> {
    let (mut config, github, installation) = github_context(state, payload, Some(issue)).await?;
    if !github.user_can_review(login).await? {
        return Ok(None);
    }
    anyhow::ensure!(
        !config.llm.api_key.is_empty(),
        "LLM_API_KEY is required by the Cururu GitHub App worker"
    );
    let revision = match revision {
        Some(revision) => revision,
        None => github.default_branch_head().await?,
    };
    let at = AtRevision::new(&github, revision.clone())?;
    merge_shared_repository_config(state, &mut config, &at, installation).await?;
    Ok(Some((config, github, revision)))
}

fn comment(candidate: &CodebaseCandidate) -> String {
    format!("## {}\n\n{}", candidate.title, candidate.body)
}

/// Run a codebase review requested in an issue.
pub async fn review_command(
    state: &AppState,
    payload: &Value,
    login: &str,
    paths: Vec<String>,
) -> anyhow::Result<()> {
    let issue = payload["issue"]["number"]
        .as_u64()
        .context("issue comment has no issue number")?;
    let Some((config, github, revision)) = config_at(state, payload, issue, login, None).await?
    else {
        info!("ignored a codebase review requested without write access");
        return Ok(());
    };
    let at = AtRevision::new(&github, revision.clone())?;
    let candidate = crate::codebase_action::build_report(&config, &at, &revision, &paths).await?;
    if !config.publication.require_approval {
        github.create_issue_comment(&comment(&candidate)).await?;
        info!("Cururu App answered a codebase review in the issue");
        return Ok(());
    }
    let digest = candidate.digest();
    let repository = config.source_control.repository.clone();
    if !state
        .store
        .store_candidate(
            &repository,
            issue,
            &digest,
            &revision,
            &serde_json::to_string(&candidate)?,
        )
        .await?
    {
        info!("Cururu App codebase candidate was already handled");
        return Ok(());
    }
    let instructions = format!(
        "Nothing was published. Inspect the report below, then press **Publish review** or comment `/cururu approve {digest}` in the issue."
    );
    let check_id = github
        .create_candidate_check(&CandidateCheck {
            head_sha: &revision,
            external_id: &digest,
            title: "Awaiting publication approval",
            summary: &instructions,
            // The check text is exactly what the digest covers.
            text: &format!("{}\n\n{}", candidate.title, candidate.body),
            approval_action: true,
            conclusion: "neutral",
        })
        .await?;
    state
        .store
        .set_candidate_check_run(&repository, &digest, check_id)
        .await?;
    // A short notice with no review content, so the requester knows where to
    // look; the report itself waits for the approval.
    github
        .create_issue_comment(&format!(
            "Cururu prepared a codebase report for `{}`; it is published here after approval. Inspect it in the `Cururu review` check on that commit, then press **Publish review** or comment `/cururu approve {digest}`.",
            &revision[..12]
        ))
        .await?;
    info!("Cururu App codebase candidate awaits approval");
    Ok(())
}

/// Publish an approved codebase report as a comment in its issue.
pub async fn publish_approved(
    state: &AppState,
    payload: &Value,
    stored: &StoredCandidate,
    approver: &str,
) -> anyhow::Result<()> {
    let candidate: CodebaseCandidate =
        serde_json::from_str(&stored.candidate).context("stored candidate is invalid")?;
    anyhow::ensure!(
        candidate.digest() == stored.digest,
        "stored candidate does not match its digest"
    );
    let Some((config, github, _)) = config_at(
        state,
        payload,
        stored.pr_number,
        approver,
        Some(candidate.revision.clone()),
    )
    .await?
    else {
        info!("ignored approval by an account without write access");
        return Ok(());
    };
    let repository = config.source_control.repository;
    if !state
        .store
        .claim_candidate(&repository, &stored.digest, approver)
        .await?
    {
        info!("ignored approval of a codebase candidate that is not pending or has expired");
        return Ok(());
    }
    let published = github.create_issue_comment(&comment(&candidate)).await;
    let (record, conclusion, title) = if published.is_ok() {
        ("published", "success", "Published")
    } else {
        ("uncertain", "neutral", "Publication failed")
    };
    state
        .store
        .finish_candidate(&repository, &stored.digest, record)
        .await?;
    if let Some(check) = stored.check_run_id
        && let Err(error) = ScmProvider::finish_candidate_check(
            &github,
            check,
            conclusion,
            title,
            &format!("{title} after approval by {approver}."),
        )
        .await
    {
        warn!(%error, "could not update the Cururu codebase candidate check");
    }
    published
}

#[cfg(test)]
mod tests {
    use super::parse_command;

    #[test]
    fn the_command_takes_optional_globs() {
        assert_eq!(parse_command("/cururu review codebase"), Some(vec![]));
        assert_eq!(
            parse_command("  /cururu review codebase src/** docs/*.md "),
            Some(vec!["src/**".to_owned(), "docs/*.md".to_owned()])
        );
        assert_eq!(parse_command("/cururu review"), None);
        assert_eq!(parse_command("/cururu review codebases"), None);
        let many = format!("/cururu review codebase {}", "a ".repeat(51));
        assert_eq!(parse_command(&many), None);
    }
}
