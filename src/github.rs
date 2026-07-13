use crate::config::GitHubConfig;
use crate::output;
use crate::retry::retry_with_backoff;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    cfg: GitHubConfig,
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    #[allow(dead_code)]
    head: PullRef,
    base: PullRef,
}

#[derive(Debug, Deserialize)]
struct PullRef {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct IssueComment {
    id: u64,
    body: Option<String>,
    user: Option<CommentUser>,
}

#[derive(Debug, Deserialize)]
struct CommentUser {
    #[allow(dead_code)]
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Serialize)]
struct CreateIssueComment<'a> {
    body: &'a str,
}

impl GitHubClient {
    pub fn new(cfg: &GitHubConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("cururu/0.1")
            .build()?;
        Ok(Self {
            client,
            cfg: cfg.clone(),
        })
    }

    pub async fn fetch_pr_diff(&self) -> anyhow::Result<String> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        retry_with_backoff(
            || async {
                self.client
                    .get(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github.v3.diff")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await
                    .context("failed to fetch PR diff")
            },
            3,
        )
        .await
    }

    pub async fn fetch_base_sha(&self) -> anyhow::Result<String> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let pr: PullRequest = retry_with_backoff(
            || async {
                self.client
                    .get(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<PullRequest>()
                    .await
                    .context("failed to fetch PR info")
            },
            3,
        )
        .await?;
        Ok(pr.base.sha)
    }

    pub async fn fetch_config_toml(&self, base_sha: &str) -> anyhow::Result<Option<String>> {
        let url = format!(
            "{}/repos/{}/{}/contents/.cururu.toml?ref={}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, base_sha
        );
        let result = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github.raw")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await?;

        if result.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        Ok(Some(
            result
                .error_for_status()
                .context("failed to fetch .cururu.toml")?
                .text()
                .await
                .context("failed to read .cururu.toml")?,
        ))
    }

    #[allow(dead_code)]
    pub async fn fetch_head_sha(&self) -> anyhow::Result<String> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let pr: PullRequest = retry_with_backoff(
            || async {
                self.client
                    .get(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<PullRequest>()
                    .await
                    .context("failed to fetch PR head SHA")
            },
            3,
        )
        .await?;
        Ok(pr.head.sha)
    }

    pub async fn upsert_summary_comment(&self, body: &str) -> anyhow::Result<()> {
        if let Some(id) = self.find_existing_summary_comment().await? {
            self.update_issue_comment(id, body).await
        } else {
            self.create_issue_comment(body).await
        }
    }

    async fn find_existing_summary_comment(&self) -> anyhow::Result<Option<u64>> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments?per_page=100",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let comments = retry_with_backoff(
            || async {
                self.client
                    .get(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Vec<IssueComment>>()
                    .await
                    .context("failed to list PR comments")
            },
            3,
        )
        .await?;

        Ok(comments
            .into_iter()
            .find(|c| {
                let bot = c.user.as_ref().is_none_or(|u| u.kind == "Bot");
                bot && c
                    .body
                    .as_deref()
                    .unwrap_or_default()
                    .contains(output::marker())
            })
            .map(|c| c.id))
    }

    async fn create_issue_comment(&self, body: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        retry_with_backoff(
            || async {
                self.client
                    .post(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .json(&CreateIssueComment { body })
                    .send()
                    .await
                    .context("failed to send create comment request")?
                    .error_for_status()
                    .context("failed to create GitHub PR summary comment")?;
                Ok(())
            },
            3,
        )
        .await
    }

    async fn update_issue_comment(&self, id: u64, body: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/comments/{id}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo
        );
        retry_with_backoff(
            || async {
                self.client
                    .patch(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .json(&CreateIssueComment { body })
                    .send()
                    .await
                    .context("failed to send update comment request")?
                    .error_for_status()
                    .context("failed to update GitHub PR summary comment")?;
                Ok(())
            },
            3,
        )
        .await
    }

    pub fn pr_url(&self) -> String {
        format!(
            "{}/{}/pull/{}",
            self.cfg.server_url, self.cfg.repository, self.cfg.pr_number
        )
    }
}
