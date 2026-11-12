//! GitHub Action journey: a review job prepares a candidate; a separate job,
//! gated by a GitHub Environment with required reviewers, publishes it.

use crate::config::AppConfig;
use crate::github::{GitHubClient, RunApproval};
use crate::hosted_publication::{
    self, ACTION_APPROVAL_PREFIX, DEFAULT_APPROVAL_ENVIRONMENT, HostedCandidate,
};
use crate::scm::{CandidateCheck, ScmProvider};
use anyhow::Context;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use tracing::warn;

const DEFAULT_CANDIDATE_PATH: &str = "cururu-candidate.json";

pub fn candidate_path() -> PathBuf {
    std::env::var_os("CURURU_CANDIDATE_PATH")
        .filter(|path| !path.is_empty())
        .map_or_else(|| PathBuf::from(DEFAULT_CANDIDATE_PATH), PathBuf::from)
}

pub fn approval_environment() -> String {
    std::env::var("CURURU_APPROVAL_ENVIRONMENT")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_APPROVAL_ENVIRONMENT.into())
}

fn append(variable: &str, lines: &str) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os(variable) else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {variable}"))?;
    file.write_all(lines.as_bytes())?;
    Ok(())
}

/// Store the candidate for the publication job and show it for inspection.
/// Nothing is commented on the pull request.
pub async fn prepare(
    candidate: &HostedCandidate,
    source_control: &dyn ScmProvider,
) -> anyhow::Result<()> {
    let digest = candidate.digest();
    let preview = candidate.preview();
    let approvable = candidate.approvable();
    let instructions = candidate.approval_instructions(false);
    let check_summary = candidate.check_summary(false);
    append(
        "GITHUB_STEP_SUMMARY",
        &format!("## Cururu review\n\n{instructions}\n\n{preview}\n"),
    )?;
    if approvable {
        let path = candidate_path();
        write_candidate_file(&path, &serde_json::to_vec_pretty(candidate)?)?;
        append(
            "GITHUB_OUTPUT",
            &format!(
                "review_status=pending_approval\ncandidate_digest={digest}\ncandidate_path={}\n",
                path.display()
            ),
        )?;
    } else {
        append("GITHUB_OUTPUT", "review_status=not_approvable\n")?;
    }
    let external_id = std::env::var("GITHUB_RUN_ID")
        .map_or_else(|_| digest.clone(), |run| format!("{digest}:run={run}"));
    // The job summary is the authoritative inspection surface for the run; the
    // check additionally lets the CLI find the candidate. It needs `checks: write`.
    if let Err(error) = source_control
        .create_candidate_check(&CandidateCheck {
            head_sha: &candidate.head_sha,
            external_id: &external_id,
            title: if approvable {
                "Awaiting publication approval"
            } else {
                "Candidate cannot be approved"
            },
            summary: &check_summary,
            text: &preview,
            approval_action: false,
        })
        .await
    {
        warn!(%error, "could not create the Cururu candidate check; the job summary still shows the candidate");
    }
    Ok(())
}

/// Write the candidate so the next workflow step can read it. The Action runs
/// in a container as root, while `actions/upload-artifact` runs as the runner
/// user, so the file must be world-readable. Its content is not secret: the
/// same preview is in the job summary and the PR check.
fn write_candidate_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, bytes)
        .with_context(|| format!("failed to write candidate to {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
            .with_context(|| format!("failed to make {} readable", path.display()))?;
    }
    Ok(())
}

pub fn read_candidate(path: &Path, expected_digest: &str) -> anyhow::Result<HostedCandidate> {
    let raw = std::fs::read(path)
        .with_context(|| format!("failed to read candidate at {}", path.display()))?;
    let candidate: HostedCandidate =
        serde_json::from_slice(&raw).context("candidate file is not a Cururu candidate")?;
    anyhow::ensure!(
        candidate.digest() == expected_digest.trim().to_ascii_lowercase(),
        "candidate file does not match the digest produced by the review job"
    );
    anyhow::ensure!(
        candidate.approvable(),
        "candidate exceeds the inspection limit and cannot be published"
    );
    Ok(candidate)
}

/// Decide whether the run's approvals authorize this candidate.
///
/// At least one approval of `environment` is required. An approval whose
/// comment names a candidate (as the CLI does) must name this one.
pub fn approved_by(
    approvals: &[RunApproval],
    environment: &str,
    digest: &str,
) -> anyhow::Result<Vec<String>> {
    let relevant: Vec<&RunApproval> = approvals
        .iter()
        .filter(|approval| {
            approval.state == "approved"
                && approval
                    .environments
                    .iter()
                    .any(|candidate| candidate.name == environment)
        })
        .collect();
    anyhow::ensure!(
        !relevant.is_empty(),
        "no approval of the `{environment}` environment was found for this run; nothing was published"
    );
    for approval in &relevant {
        if let Some(named) = approval.comment.as_deref().and_then(named_digest) {
            anyhow::ensure!(
                named == digest,
                "an approval names a different candidate; nothing was published"
            );
        }
    }
    Ok(relevant
        .iter()
        .map(|approval| approval.user.login.clone())
        .collect())
}

fn named_digest(comment: &str) -> Option<String> {
    let start = comment.find(ACTION_APPROVAL_PREFIX)? + ACTION_APPROVAL_PREFIX.len();
    Some(
        comment[start..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect::<String>()
            .to_ascii_lowercase(),
    )
}

/// Publish the approved candidate from the protected publication job.
pub async fn publish_approved(current: &AppConfig, github: &GitHubClient) -> anyhow::Result<()> {
    let digest = std::env::var("CURURU_CANDIDATE_DIGEST").context(
        "CURURU_CANDIDATE_DIGEST is required; pass the review job's candidate_digest output",
    )?;
    let candidate = read_candidate(&candidate_path(), &digest)?;
    let digest = candidate.digest();
    let environment = approval_environment();
    anyhow::ensure!(
        github.environment_requires_reviewers(&environment).await?,
        "environment `{environment}` must require reviewers before Cururu can publish; nothing was published"
    );
    let run_id: u64 = std::env::var("GITHUB_RUN_ID")
        .context("GITHUB_RUN_ID is required")?
        .parse()
        .context("GITHUB_RUN_ID must be numeric")?;
    let approvers = approved_by(
        &github.workflow_run_approvals(run_id).await?,
        &environment,
        &digest,
    )?;
    hosted_publication::ensure_candidate_current(&candidate, current, github).await?;
    hosted_publication::publish_candidate(&candidate, github, None)
        .await
        .context("publication did not complete and its outcome may be partial; inspect the PR before re-running this job")?;
    append("GITHUB_OUTPUT", "review_status=completed\n")?;
    if let Ok(checks) = github.list_candidate_check_runs(&candidate.head_sha).await
        && let Some(check) = checks.iter().find(|check| {
            check
                .external_id
                .as_deref()
                .is_some_and(|id| id.starts_with(&digest))
        })
    {
        let _ = github
            .finish_candidate_check_run(
                check.id,
                "success",
                "Published",
                &format!("Published after approval by {}.", approvers.join(", ")),
            )
            .await;
    }
    println!(
        "Cururu: published candidate {digest} approved by {}",
        approvers.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::RunApproval;

    fn approval(state: &str, environment: &str, comment: Option<&str>) -> RunApproval {
        serde_json::from_value(serde_json::json!({
            "state": state,
            "comment": comment,
            "environments": [{"id": 1, "name": environment}],
            "user": {"login": "alice"}
        }))
        .unwrap()
    }

    #[test]
    fn approval_requires_environment_and_matching_named_candidate() {
        let digest = "ab".repeat(32);
        assert!(approved_by(&[], "cururu-publish", &digest).is_err());
        assert!(
            approved_by(
                &[approval("rejected", "cururu-publish", None)],
                "cururu-publish",
                &digest
            )
            .is_err()
        );
        assert!(
            approved_by(
                &[approval("approved", "other", None)],
                "cururu-publish",
                &digest
            )
            .is_err()
        );
        assert_eq!(
            approved_by(
                &[approval("approved", "cururu-publish", None)],
                "cururu-publish",
                &digest
            )
            .unwrap(),
            vec!["alice".to_string()]
        );
        let named = format!("{ACTION_APPROVAL_PREFIX}{digest}");
        assert!(
            approved_by(
                &[approval("approved", "cururu-publish", Some(&named))],
                "cururu-publish",
                &digest
            )
            .is_ok()
        );
        let other = format!("{ACTION_APPROVAL_PREFIX}{}", "cd".repeat(32));
        assert!(
            approved_by(
                &[
                    approval("approved", "cururu-publish", None),
                    approval("approved", "cururu-publish", Some(&other))
                ],
                "cururu-publish",
                &digest
            )
            .is_err()
        );
    }

    #[test]
    fn candidate_file_must_match_the_review_job_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidate.json");
        let candidate = crate::hosted_publication::tests::sample("en-US");
        write_candidate_file(&path, &serde_json::to_vec(&candidate).unwrap()).unwrap();
        assert_eq!(
            read_candidate(&path, &candidate.digest()).unwrap(),
            candidate
        );
        assert!(read_candidate(&path, &"0".repeat(64)).is_err());
        let mut tampered = candidate.clone();
        tampered.summary.push_str(" tampered");
        write_candidate_file(&path, &serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert!(read_candidate(&path, &candidate.digest()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn candidate_file_is_readable_by_other_workflow_users() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidate.json");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_candidate_file(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }
}
