//! GitHub surfaces for inspecting and approving hosted publication candidates.

use super::{GitHubClient, url_encode};
use crate::hosted_publication::{CHECK_RUN_NAME, PUBLISH_ACTION_ID};
use crate::scm::CandidateCheck;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct CandidateCheckRun {
    pub id: u64,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub output: CheckRunOutput,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CheckRunOutput {
    /// Approval instructions and the run's hidden usage record.
    #[serde(default)]
    pub summary: Option<String>,
    /// The candidate preview; its SHA-256 must equal the claimed digest.
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CandidateCheckRunList {
    check_runs: Vec<CandidateCheckRun>,
}

#[derive(Debug, Deserialize)]
struct CreatedCheckRun {
    id: u64,
}

#[derive(Serialize)]
struct CheckRunOutputBody<'a> {
    title: &'a str,
    summary: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
}

#[derive(Serialize)]
struct CheckRunAction<'a> {
    label: &'a str,
    description: &'a str,
    identifier: &'a str,
}

#[derive(Serialize)]
struct CreateCheckRun<'a> {
    name: &'a str,
    head_sha: &'a str,
    external_id: &'a str,
    status: &'a str,
    conclusion: &'a str,
    output: CheckRunOutputBody<'a>,
    actions: Vec<CheckRunAction<'a>>,
}

#[derive(Serialize)]
struct UpdateCheckRun<'a> {
    conclusion: &'a str,
    output: CheckRunOutputBody<'a>,
    actions: Vec<CheckRunAction<'a>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunApproval {
    pub state: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub environments: Vec<NamedEnvironment>,
    pub user: ApprovalUser,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NamedEnvironment {
    #[serde(default)]
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalUser {
    pub login: String,
}

#[derive(Debug, Deserialize)]
struct EnvironmentDetails {
    #[serde(default)]
    protection_rules: Vec<ProtectionRule>,
}

#[derive(Debug, Deserialize)]
struct ProtectionRule {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PendingDeployment {
    pub environment: NamedEnvironment,
    #[serde(default)]
    pub current_user_can_approve: bool,
}

#[derive(Serialize)]
struct ReviewPendingDeployments<'a> {
    environment_ids: [u64; 1],
    state: &'a str,
    comment: &'a str,
}

#[derive(Debug, Deserialize)]
struct PullRequestCommit {
    sha: String,
}

/// GitHub lists at most this many commits for a pull request.
const MAX_PR_COMMITS: usize = 250;

impl GitHubClient {
    /// Usage markers from every `Cururu review` check on the PR's commits.
    pub async fn review_usage_history(&self) -> anyhow::Result<crate::review_usage::UsageHistory> {
        let mut commits = Vec::new();
        for page in 1..=3 {
            let mut batch: Vec<PullRequestCommit> = self
                .api(
                    reqwest::Method::GET,
                    &format!(
                        "/pulls/{}/commits?per_page=100&page={page}",
                        self.cfg.pr_number
                    ),
                )
                .send()
                .await
                .context("failed to list PR commits")?
                .error_for_status()
                .context("failed to read PR commits")?
                .json()
                .await
                .context("failed to parse PR commits")?;
            let last = batch.len() < 100;
            commits.append(&mut batch);
            if last {
                break;
            }
        }
        let complete = commits.len() < MAX_PR_COMMITS;
        let mut records = Vec::new();
        for chunk in commits.chunks(8) {
            let mut tasks = tokio::task::JoinSet::new();
            for commit in chunk {
                let client = self.clone();
                let sha = commit.sha.clone();
                tasks.spawn(async move { client.list_candidate_check_runs(&sha).await });
            }
            while let Some(result) = tasks.join_next().await {
                for check in result.context("usage history task failed")?? {
                    if let Some(summary) = check.output.summary.as_deref() {
                        records.extend(crate::review_usage::parse_markers(summary));
                    }
                }
            }
        }
        Ok(crate::review_usage::UsageHistory { records, complete })
    }

    pub(super) fn api(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(
                method,
                format!(
                    "{}/repos/{}/{}{path}",
                    self.cfg.api_url, self.cfg.owner, self.cfg.repo
                ),
            )
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
    }

    /// Create a completed, neutral check that shows a candidate. Not retried:
    /// a lost response may still have created the check.
    pub async fn create_candidate_check_run(
        &self,
        check: &CandidateCheck<'_>,
    ) -> anyhow::Result<u64> {
        let actions = if check.approval_action {
            vec![CheckRunAction {
                label: "Publish review",
                description: "Publish exactly this candidate",
                identifier: PUBLISH_ACTION_ID,
            }]
        } else {
            Vec::new()
        };
        let response = self
            .api(reqwest::Method::POST, "/check-runs")
            .json(&CreateCheckRun {
                name: CHECK_RUN_NAME,
                head_sha: check.head_sha,
                external_id: check.external_id,
                status: "completed",
                conclusion: "neutral",
                output: CheckRunOutputBody {
                    title: check.title,
                    summary: check.summary,
                    text: Some(check.text),
                },
                actions,
            })
            .send()
            .await
            .context("failed to send candidate check request")?;
        let status = response.status();
        anyhow::ensure!(
            status.is_success(),
            "failed to create candidate check (HTTP {})",
            status.as_u16()
        );
        Ok(response
            .json::<CreatedCheckRun>()
            .await
            .context("failed to parse candidate check")?
            .id)
    }

    pub async fn finish_candidate_check_run(
        &self,
        id: u64,
        conclusion: &str,
        title: &str,
        summary: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .api(reqwest::Method::PATCH, &format!("/check-runs/{id}"))
            .json(&UpdateCheckRun {
                conclusion,
                output: CheckRunOutputBody {
                    title,
                    summary,
                    text: None,
                },
                actions: Vec::new(),
            })
            .send()
            .await
            .context("failed to send candidate check update")?;
        anyhow::ensure!(
            response.status().is_success(),
            "failed to update candidate check (HTTP {})",
            response.status().as_u16()
        );
        Ok(())
    }

    /// Candidate checks attached to a commit, newest first as returned by GitHub.
    pub async fn list_candidate_check_runs(
        &self,
        head_sha: &str,
    ) -> anyhow::Result<Vec<CandidateCheckRun>> {
        let list: CandidateCheckRunList = self
            .api(
                reqwest::Method::GET,
                &format!(
                    "/commits/{}/check-runs?check_name={}&filter=all&per_page=100",
                    url_encode(head_sha),
                    url_encode(CHECK_RUN_NAME)
                ),
            )
            .send()
            .await
            .context("failed to list candidate checks")?
            .error_for_status()
            .context("failed to read candidate checks")?
            .json()
            .await
            .context("failed to parse candidate checks")?;
        Ok(list.check_runs)
    }

    pub async fn workflow_run_approvals(&self, run_id: u64) -> anyhow::Result<Vec<RunApproval>> {
        self.api(
            reqwest::Method::GET,
            &format!("/actions/runs/{run_id}/approvals"),
        )
        .send()
        .await
        .context("failed to list workflow run approvals")?
        .error_for_status()
        .context("failed to read workflow run approvals; grant `actions: read`")?
        .json()
        .await
        .context("failed to parse workflow run approvals")
    }

    /// Whether an environment is protected by required reviewers.
    pub async fn environment_requires_reviewers(&self, name: &str) -> anyhow::Result<bool> {
        let details: EnvironmentDetails = self
            .api(
                reqwest::Method::GET,
                &format!("/environments/{}", url_encode(name)),
            )
            .send()
            .await
            .context("failed to read approval environment")?
            .error_for_status()
            .context("failed to read approval environment; grant `actions: read`")?
            .json()
            .await
            .context("failed to parse approval environment")?;
        Ok(details
            .protection_rules
            .iter()
            .any(|rule| rule.kind == "required_reviewers"))
    }

    pub async fn pending_deployments(&self, run_id: u64) -> anyhow::Result<Vec<PendingDeployment>> {
        self.api(
            reqwest::Method::GET,
            &format!("/actions/runs/{run_id}/pending_deployments"),
        )
        .send()
        .await
        .context("failed to list pending deployments")?
        .error_for_status()
        .context("failed to read pending deployments")?
        .json()
        .await
        .context("failed to parse pending deployments")
    }

    /// Approve one environment of a waiting run as the token's user. Not retried.
    pub async fn approve_pending_deployment(
        &self,
        run_id: u64,
        environment_id: u64,
        comment: &str,
    ) -> anyhow::Result<()> {
        let response = self
            .api(
                reqwest::Method::POST,
                &format!("/actions/runs/{run_id}/pending_deployments"),
            )
            .json(&ReviewPendingDeployments {
                environment_ids: [environment_id],
                state: "approved",
                comment,
            })
            .send()
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "approval outcome is unknown; check the workflow run before retrying"
                )
            })?;
        anyhow::ensure!(
            response.status().is_success(),
            "approval failed (HTTP {}); check the workflow run before retrying",
            response.status().as_u16()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GitHubConfig;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
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
    async fn candidate_check_offers_action_only_when_requested_and_close_removes_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/check-runs"))
            .and(body_partial_json(serde_json::json!({
                "name": CHECK_RUN_NAME,
                "head_sha": "head",
                "external_id": "digest",
                "conclusion": "neutral",
                "output": {"text": "preview"},
                "actions": [{"identifier": PUBLISH_ACTION_ID}]
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": 9})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/owner/repo/check-runs/9"))
            .and(body_partial_json(
                serde_json::json!({"conclusion": "success", "actions": []}),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let github = client(&server);
        let id = github
            .create_candidate_check_run(&CandidateCheck {
                head_sha: "head",
                external_id: "digest",
                title: "title",
                summary: "summary",
                text: "preview",
                approval_action: true,
            })
            .await
            .unwrap();
        github
            .finish_candidate_check_run(id, "success", "done", "published")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn environment_protection_and_approvals_are_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/environments/cururu-publish"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "protection_rules": [{"type": "wait_timer"}, {"type": "required_reviewers"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/environments/open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "protection_rules": [{"type": "branch_policy"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/actions/runs/5/approvals"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "state": "approved",
                    "comment": "cururu-candidate:abc",
                    "environments": [{"id": 3, "name": "cururu-publish"}],
                    "user": {"login": "alice"}
                }])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/commits/head/check-runs"))
            .and(query_param("check_name", CHECK_RUN_NAME))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "check_runs": [{"id": 1, "external_id": "d", "output": {"text": "t"}, "app": {"slug": "cururu"}}]
            })))
            .mount(&server)
            .await;
        let github = client(&server);
        assert!(
            github
                .environment_requires_reviewers("cururu-publish")
                .await
                .unwrap()
        );
        assert!(!github.environment_requires_reviewers("open").await.unwrap());
        let approvals = github.workflow_run_approvals(5).await.unwrap();
        assert_eq!(approvals[0].user.login, "alice");
        assert_eq!(approvals[0].environments[0].name, "cururu-publish");
        let checks = github.list_candidate_check_runs("head").await.unwrap();
        assert_eq!(checks[0].output.text.as_deref(), Some("t"));
    }

    #[tokio::test]
    async fn usage_history_reads_markers_from_every_pr_commit_check() {
        let server = MockServer::start().await;
        let first = crate::review_usage::UsageRecord::new("r1", "c1", None);
        let second = crate::review_usage::UsageRecord::new("r2", "c2", None);
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/7/commits"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"sha": "c1"}, {"sha": "c2"}])),
            )
            .mount(&server)
            .await;
        for (sha, record) in [("c1", &first), ("c2", &second)] {
            Mock::given(method("GET"))
                .and(path(format!("/repos/owner/repo/commits/{sha}/check-runs")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "check_runs": [{"id": 1, "output": {"summary": format!("text {}", record.marker())}}]
                })))
                .mount(&server)
                .await;
        }
        let history = client(&server).review_usage_history().await.unwrap();
        assert!(history.complete);
        let mut runs: Vec<_> = history
            .records
            .iter()
            .map(|record| record.run.clone())
            .collect();
        runs.sort();
        assert_eq!(runs, ["r1", "r2"]);
    }

    #[tokio::test]
    async fn review_summary_is_a_new_comment_posted_once_per_run() {
        let server = MockServer::start().await;
        let existing = format!(
            "{}\n{}",
            crate::output::marker(),
            crate::output::run_marker("old")
        );
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": 1, "body": existing, "user": {"login": "cururu[bot]", "type": "Bot"}}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/issues/7/comments"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let github = client(&server);
        github
            .publish_review_summary("old summary", "old")
            .await
            .unwrap();
        github
            .publish_review_summary("new summary", "new")
            .await
            .unwrap();
    }
}
