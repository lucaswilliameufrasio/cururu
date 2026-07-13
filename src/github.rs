use crate::config::GitHubConfig;
use crate::output;
use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    cfg: GitHubConfig,
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    head: PullHead,
}

#[derive(Debug, Deserialize)]
struct PullHead {
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
            .user_agent("pullfrog-rs/0.1")
            .build()?;
        Ok(Self { client, cfg: cfg.clone() })
    }

    pub async fn fetch_pr_diff(&self) -> anyhow::Result<String> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let text = self.client
            .get(url)
            .header("Accept", "application/vnd.github.v3.diff")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(text)
    }

    pub async fn fetch_head_sha(&self) -> anyhow::Result<String> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let pr = self.client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await?
            .error_for_status()?
            .json::<PullRequest>()
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
        let comments = self.client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<IssueComment>>()
            .await?;

        Ok(comments.into_iter().find(|c| {
            let bot = c.user.as_ref().map(|u| u.kind == "Bot").unwrap_or(true);
            bot && c.body.as_deref().unwrap_or_default().contains(output::marker())
        }).map(|c| c.id))
    }

    async fn create_issue_comment(&self, body: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        self.client
            .post(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .json(&CreateIssueComment { body })
            .send()
            .await?
            .error_for_status()
            .context("failed to create GitHub PR summary comment")?;
        Ok(())
    }

    async fn update_issue_comment(&self, id: u64, body: &str) -> anyhow::Result<()> {
        let url = format!("{}/repos/{}/{}/issues/comments/{id}", self.cfg.api_url, self.cfg.owner, self.cfg.repo);
        self.client
            .patch(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .json(&CreateIssueComment { body })
            .send()
            .await?
            .error_for_status()
            .context("failed to update GitHub PR summary comment")?;
        Ok(())
    }

    pub fn pr_url(&self) -> String {
        format!("{}/{}/pull/{}", self.cfg.server_url, self.cfg.repository, self.cfg.pr_number)
    }
}
