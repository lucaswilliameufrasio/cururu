//! GitHub operations for opt-in auto-approval.

use super::GitHubClient;
use crate::auto_approval::NOTICE_MARKER;
use crate::scm::ApprovalOutcome;
use anyhow::Context;
use serde::{Deserialize, Serialize};

const APPROVAL_MARKER: &str = "<!-- cururu:auto-approval:approved head=";

#[derive(Debug, Deserialize)]
struct ReviewSummary {
    id: u64,
    state: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<crate::scm::ScmIdentity>,
}

#[derive(Serialize)]
struct ApproveReview<'a> {
    commit_id: &'a str,
    body: &'a str,
    event: &'a str,
}

#[derive(Serialize)]
struct DismissReview<'a> {
    message: &'a str,
    event: &'a str,
}

#[derive(Serialize)]
struct StatusCheck<'a> {
    name: &'a str,
    head_sha: &'a str,
    status: &'a str,
    conclusion: &'a str,
    output: StatusCheckOutput<'a>,
}

#[derive(Serialize)]
struct StatusCheckOutput<'a> {
    title: &'a str,
    summary: &'a str,
}

#[derive(Deserialize)]
struct GitHubError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct WorkflowPermissions {
    can_approve_pull_request_reviews: bool,
}

impl GitHubClient {
    /// Whether the repository lets GitHub Actions approve pull requests.
    pub async fn actions_can_approve_pull_requests(&self) -> anyhow::Result<bool> {
        let permissions: WorkflowPermissions = self
            .api(reqwest::Method::GET, "/actions/permissions/workflow")
            .send()
            .await
            .context("failed to read workflow permissions")?
            .error_for_status()
            .context("GitHub refused to read workflow permissions")?
            .json()
            .await
            .context("invalid workflow permissions")?;
        Ok(permissions.can_approve_pull_request_reviews)
    }

    /// Submit an `APPROVE` review bound to `head`. Not retried: a lost
    /// response may already have approved. A refusal by GitHub (for example
    /// Actions not allowed to approve, or approving one's own PR) is reported
    /// as `NotPermitted` so a human review is requested instead.
    pub async fn approve_change_request(
        &self,
        head: &str,
        body: &str,
    ) -> anyhow::Result<ApprovalOutcome> {
        let response = self
            .api(
                reqwest::Method::POST,
                &format!("/pulls/{}/reviews", self.cfg.pr_number),
            )
            .json(&ApproveReview {
                commit_id: head,
                body,
                event: "APPROVE",
            })
            .send()
            .await
            .map_err(|_| {
                anyhow::anyhow!("approval outcome is unknown; check the PR before retrying")
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(ApprovalOutcome::Approved);
        }
        if matches!(status.as_u16(), 403 | 422) {
            let detail = response
                .json::<GitHubError>()
                .await
                .map(|error| {
                    let extra = error
                        .errors
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                        .join("; ");
                    if extra.is_empty() {
                        error.message
                    } else {
                        format!("{}: {extra}", error.message)
                    }
                })
                .unwrap_or_default();
            let detail: String = detail.chars().take(300).collect();
            return Ok(ApprovalOutcome::NotPermitted(format!(
                "HTTP {}{}",
                status.as_u16(),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            )));
        }
        anyhow::bail!(
            "approval failed (HTTP {}); check the PR before retrying",
            status.as_u16()
        )
    }

    /// Dismiss Cururu's own auto-approvals that were given for another head.
    pub async fn dismiss_stale_auto_approvals(&self, head: &str) -> anyhow::Result<usize> {
        let own_login = self.current_login().await;
        let current = format!("{APPROVAL_MARKER}{head} -->");
        let mut dismissed = 0;
        for page in 1..=10 {
            let reviews: Vec<ReviewSummary> = self
                .api(
                    reqwest::Method::GET,
                    &format!(
                        "/pulls/{}/reviews?per_page=100&page={page}",
                        self.cfg.pr_number
                    ),
                )
                .send()
                .await
                .context("failed to list pull request reviews")?
                .error_for_status()
                .context("failed to read pull request reviews")?
                .json()
                .await
                .context("invalid pull request review list")?;
            let last = reviews.len() < 100;
            for review in reviews {
                let body = review.body.as_deref().unwrap_or_default();
                if review.state == "APPROVED"
                    && body.contains(APPROVAL_MARKER)
                    && !body.contains(&current)
                    && Self::user_is_cururu(review.user.as_ref(), own_login.as_deref())
                {
                    self.api(
                        reqwest::Method::PUT,
                        &format!(
                            "/pulls/{}/reviews/{}/dismissals",
                            self.cfg.pr_number, review.id
                        ),
                    )
                    .json(&DismissReview {
                        message: "New commits since Cururu's auto-approval; re-evaluating.",
                        event: "DISMISS",
                    })
                    .send()
                    .await
                    .context("failed to dismiss a stale Cururu approval")?
                    .error_for_status()
                    .context("GitHub refused to dismiss a stale Cururu approval")?;
                    dismissed += 1;
                }
            }
            if last {
                return Ok(dismissed);
            }
        }
        anyhow::bail!("too many reviews to safely dismiss stale Cururu approvals")
    }

    pub async fn upsert_auto_approval_notice(&self, body: &str) -> anyhow::Result<()> {
        self.upsert_marked_comment(NOTICE_MARKER, body).await
    }

    /// Create a completed check with a conclusion and summary.
    pub async fn create_status_check(
        &self,
        name: &str,
        head: &str,
        conclusion: &str,
        title: &str,
        summary: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .api(reqwest::Method::POST, "/check-runs")
            .json(&StatusCheck {
                name,
                head_sha: head,
                status: "completed",
                conclusion,
                output: StatusCheckOutput { title, summary },
            })
            .send()
            .await
            .context("failed to send status check")?;
        anyhow::ensure!(
            response.status().is_success(),
            "failed to create status check (HTTP {})",
            response.status().as_u16()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GitHubConfig;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> GitHubClient {
        GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 7,
            api_url: server.uri(),
            server_url: server.uri(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn approval_is_bound_to_head_and_refusals_are_not_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/7/reviews"))
            .and(body_partial_json(
                serde_json::json!({"commit_id": "good", "event": "APPROVE"}),
            ))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/7/reviews"))
            .and(body_partial_json(
                serde_json::json!({"commit_id": "denied"}),
            ))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "message": "Unprocessable Entity",
                "errors": ["GitHub Actions is not permitted to approve pull requests."]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/7/reviews"))
            .and(body_partial_json(
                serde_json::json!({"commit_id": "broken"}),
            ))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let github = client(&server);
        assert_eq!(
            github.approve_change_request("good", "b").await.unwrap(),
            ApprovalOutcome::Approved
        );
        let ApprovalOutcome::NotPermitted(detail) =
            github.approve_change_request("denied", "b").await.unwrap()
        else {
            panic!("expected refusal");
        };
        assert!(detail.contains("HTTP 422") && detail.contains("not permitted to approve"));
        assert!(github.approve_change_request("broken", "b").await.is_err());
    }

    #[tokio::test]
    async fn only_own_stale_auto_approvals_are_dismissed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"login": "cururu[bot]"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/7/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": 1, "state": "APPROVED", "body": format!("{APPROVAL_MARKER}old -->"), "user": {"login": "cururu[bot]", "type": "Bot"}},
                {"id": 2, "state": "APPROVED", "body": format!("{APPROVAL_MARKER}new -->"), "user": {"login": "cururu[bot]", "type": "Bot"}},
                {"id": 3, "state": "APPROVED", "body": format!("{APPROVAL_MARKER}old -->"), "user": {"login": "mallory", "type": "User"}},
                {"id": 4, "state": "COMMENTED", "body": format!("{APPROVAL_MARKER}old -->"), "user": {"login": "cururu[bot]", "type": "Bot"}},
                {"id": 5, "state": "APPROVED", "body": "lgtm", "user": {"login": "cururu[bot]", "type": "Bot"}}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/repos/owner/repo/pulls/7/reviews/1/dismissals"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            client(&server)
                .dismiss_stale_auto_approvals("new")
                .await
                .unwrap(),
            1
        );
    }
}
