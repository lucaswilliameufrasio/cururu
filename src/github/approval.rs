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

impl GitHubClient {
    fn api(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
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
}
