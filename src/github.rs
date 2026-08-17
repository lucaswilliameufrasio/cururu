use crate::config::GitHubConfig;
use crate::output;
use crate::retry::retry_with_backoff;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn url_encode(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace('&', "%26")
        .replace('+', "%2B")
        .replace(' ', "%20")
}

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
    #[serde(rename = "ref")]
    name: String,
}

#[derive(Debug, Deserialize)]
struct GitRef {
    object: GitRefObject,
}

#[derive(Debug, Deserialize)]
struct GitRefObject {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct CollaboratorPermission {
    permission: String,
}

#[derive(Debug, Deserialize)]
struct AuthenticatedUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct IssueComment {
    id: u64,
    body: Option<String>,
    user: Option<CommentUser>,
}

#[derive(Debug, Deserialize)]
pub struct CommentUser {
    pub login: String,
}

#[derive(Debug, Serialize)]
struct CreateIssueComment<'a> {
    body: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub body: Option<String>,
    pub user: Option<CommentUser>,
    pub path: String,
    #[allow(dead_code)]
    pub line: Option<u32>,
    #[allow(dead_code)]
    pub subject_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReviewCommentBody<'a> {
    body: &'a str,
}

#[derive(Debug, Clone)]
pub struct ReviewCommentDraft {
    pub path: String,
    pub line: Option<u32>,
    pub body: String,
}

#[derive(Debug, Serialize)]
struct CreateReviewComment<'a> {
    body: &'a str,
    commit_id: &'a str,
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject_type: Option<&'a str>,
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
        let pr_url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let pr: PullRequest = retry_with_backoff(
            || async {
                self.client
                    .get(&pr_url)
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

        // Resolve the base branch head so .cururu.toml is read from the current
        // base branch state, not the possibly-stale merge base recorded on the PR.
        let ref_url = format!(
            "{}/repos/{}/{}/git/ref/heads/{}",
            self.cfg.api_url,
            self.cfg.owner,
            self.cfg.repo,
            url_encode(&pr.base.name)
        );
        let head_sha = retry_with_backoff(
            || async {
                self.client
                    .get(&ref_url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<GitRef>()
                    .await
                    .context("failed to fetch base branch ref")
                    .map(|r| r.object.sha)
            },
            3,
        )
        .await?;

        Ok(head_sha)
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

    pub async fn fetch_file_at_ref(&self, path: &str, sha: &str) -> anyhow::Result<String> {
        let url = format!(
            "{}/repos/{}/{}/contents/{}?ref={}",
            self.cfg.api_url,
            self.cfg.owner,
            self.cfg.repo,
            url_encode(path),
            url_encode(sha)
        );
        self.client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github.raw")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await
            .context("failed to fetch file content")?
            .error_for_status()
            .context("file content API error")?
            .text()
            .await
            .context("failed to read file content")
    }

    pub async fn user_can_review(&self, login: &str) -> anyhow::Result<bool> {
        let url = format!(
            "{}/repos/{}/{}/collaborators/{}/permission",
            self.cfg.api_url,
            self.cfg.owner,
            self.cfg.repo,
            url_encode(login)
        );
        let permission: CollaboratorPermission = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await
            .context("failed to check commenter permission")?
            .error_for_status()
            .context("failed to read commenter permission")?
            .json()
            .await
            .context("failed to parse commenter permission")?;
        Ok(matches!(
            permission.permission.as_str(),
            "admin" | "maintain" | "write"
        ))
    }

    async fn current_login(&self) -> anyhow::Result<String> {
        let url = format!("{}/user", self.cfg.api_url);
        Ok(self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await
            .context("failed to fetch authenticated GitHub user")?
            .error_for_status()?
            .json::<AuthenticatedUser>()
            .await
            .context("failed to parse authenticated GitHub user")?
            .login)
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

    pub async fn list_review_comments(&self) -> anyhow::Result<Vec<ReviewComment>> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/comments?per_page=100",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        retry_with_backoff(
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
                    .json::<Vec<ReviewComment>>()
                    .await
                    .context("failed to list review comments")
            },
            3,
        )
        .await
    }

    pub async fn create_review_comment(
        &self,
        head_sha: &str,
        path: &str,
        line: Option<u32>,
        body: &str,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/comments",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let (line, subject_type) = line.map_or((None, Some("file")), |l| (Some(l), None));
        let payload = CreateReviewComment {
            body,
            commit_id: head_sha,
            path,
            line,
            subject_type,
        };
        retry_with_backoff(
            || async {
                let resp = self
                    .client
                    .post(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .json(&payload)
                    .send()
                    .await
                    .context("failed to send create review comment request")?;
                let status = resp.status();
                if !status.is_success() {
                    let detail = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "(unreadable body)".to_string());
                    anyhow::bail!("failed to create review comment ({status}): {detail}");
                }
                Ok(())
            },
            3,
        )
        .await
    }

    pub async fn update_review_comment(&self, id: u64, body: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/comments/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, id
        );
        retry_with_backoff(
            || async {
                let resp = self
                    .client
                    .patch(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .json(&ReviewCommentBody { body })
                    .send()
                    .await
                    .context("failed to send update review comment request")?;
                let status = resp.status();
                if !status.is_success() {
                    let detail = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "(unreadable body)".to_string());
                    anyhow::bail!("failed to update review comment ({status}): {detail}");
                }
                Ok(())
            },
            3,
        )
        .await
    }

    pub async fn delete_review_comment(&self, id: u64) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/comments/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, id
        );
        retry_with_backoff(
            || async {
                let resp = self
                    .client
                    .delete(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await
                    .context("failed to send delete review comment request")?;
                let status = resp.status();
                if !status.is_success() {
                    let detail = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "(unreadable body)".to_string());
                    anyhow::bail!("failed to delete review comment ({status}): {detail}");
                }
                Ok(())
            },
            3,
        )
        .await
    }

    /// Reconcile inline review comments so the PR matches the desired set.
    ///
    /// Existing Cururu comments are matched by (path, line); those still
    /// desired are updated in place, new ones are created, and stale ones are
    /// deleted. Falls back to the file-level comment (`subject_type = "file"`)
    /// for findings without a valid line anchor.
    pub async fn reconcile_review_comments(
        &self,
        head_sha: &str,
        desired: &[ReviewCommentDraft],
    ) -> anyhow::Result<()> {
        let existing = self.list_review_comments().await?;
        let current_login = self.current_login().await?;
        let cururu_existing: Vec<ReviewComment> = existing
            .into_iter()
            .filter(|c| {
                let is_cururu_user = c
                    .user
                    .as_ref()
                    .is_some_and(|user| user.login == current_login);
                is_cururu_user
                    && c.body
                        .as_deref()
                        .unwrap_or_default()
                        .contains(output::finding_marker())
            })
            .collect();

        // Map desired comments by (path, line) key so multiple findings on the
        // same line merge into one comment.
        let desired_map = merge_desired_by_anchor(desired);

        for comment in &cururu_existing {
            let key = (comment.path.clone(), comment.line);
            if let Some(new_body) = desired_map.get(&key) {
                if comment.body.as_deref().is_some_and(|b| b != new_body) {
                    self.update_review_comment(comment.id, new_body).await?;
                }
            } else {
                self.delete_review_comment(comment.id).await?;
            }
        }

        let existing_keys: std::collections::HashSet<(String, Option<u32>)> = cururu_existing
            .iter()
            .map(|c| (c.path.clone(), c.line))
            .collect();

        for (key, body) in &desired_map {
            if !existing_keys.contains(key) {
                self.create_review_comment(head_sha, &key.0, key.1, body)
                    .await?;
            }
        }

        Ok(())
    }

    pub async fn upsert_summary_comment(&self, body: &str) -> anyhow::Result<()> {
        if let Some(id) = self.find_existing_summary_comment().await? {
            self.update_issue_comment(id, body).await
        } else {
            self.create_issue_comment(body).await
        }
    }

    pub async fn summary_has_head(&self, head_sha: &str) -> anyhow::Result<bool> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments?per_page=100",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let comments: Vec<IssueComment> = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await
            .context("failed to list PR comments")?
            .error_for_status()?
            .json()
            .await
            .context("failed to parse PR comments")?;
        let current_login = self.current_login().await?;
        Ok(comments.iter().any(|comment| {
            comment
                .user
                .as_ref()
                .is_some_and(|user| user.login == current_login)
                && comment.body.as_deref().is_some_and(|body| {
                    body.contains(output::marker())
                        && body.contains(&format!("<!-- cururu:state:v1 head={head_sha} -->"))
                })
        }))
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
        let current_login = self.current_login().await?;

        Ok(comments
            .into_iter()
            .find(|c| {
                let is_cururu_user = c
                    .user
                    .as_ref()
                    .is_some_and(|user| user.login == current_login);
                is_cururu_user
                    && c.body
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
                let resp = self
                    .client
                    .post(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .json(&CreateIssueComment { body })
                    .send()
                    .await
                    .context("failed to send create comment request")?;
                let status = resp.status();
                if !status.is_success() {
                    let detail = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "(unreadable body)".to_string());
                    anyhow::bail!(
                        "failed to create GitHub PR summary comment ({status}): {detail}"
                    );
                }
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

/// Group desired comments by (path, line) so findings on the same anchor merge
/// into a single comment body.
fn merge_desired_by_anchor(
    desired: &[ReviewCommentDraft],
) -> std::collections::HashMap<(String, Option<u32>), String> {
    let mut map: std::collections::HashMap<(String, Option<u32>), String> =
        std::collections::HashMap::new();
    for draft in desired {
        let entry = map.entry((draft.path.clone(), draft.line)).or_default();
        if !entry.is_empty() {
            entry.push_str("\n\n---\n\n");
        }
        entry.push_str(&draft.body);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_desired_groups_same_anchor() {
        let drafts = vec![
            ReviewCommentDraft {
                path: "a.rs".into(),
                line: Some(1),
                body: "first".into(),
            },
            ReviewCommentDraft {
                path: "a.rs".into(),
                line: Some(1),
                body: "second".into(),
            },
            ReviewCommentDraft {
                path: "b.rs".into(),
                line: None,
                body: "file-level".into(),
            },
        ];
        let map = merge_desired_by_anchor(&drafts);
        assert_eq!(map.len(), 2);
        let merged = map.get(&("a.rs".to_string(), Some(1))).unwrap();
        assert!(merged.contains("first"));
        assert!(merged.contains("second"));
        assert!(merged.contains("---"));
        assert_eq!(map.get(&("b.rs".to_string(), None)).unwrap(), "file-level");
    }

    #[test]
    fn merge_desired_distinct_anchors_stay_separate() {
        let drafts = vec![
            ReviewCommentDraft {
                path: "a.rs".into(),
                line: Some(1),
                body: "x".into(),
            },
            ReviewCommentDraft {
                path: "a.rs".into(),
                line: Some(2),
                body: "y".into(),
            },
        ];
        let map = merge_desired_by_anchor(&drafts);
        assert_eq!(map.len(), 2);
    }
}
