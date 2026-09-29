use crate::config::{GitHubConfig, ScmConfig};
use crate::output;
use crate::repository::{RepositoryIdentity, validate_github_adapter};
use crate::retry::retry_with_backoff;
use crate::scm::{
    FindingAnnotation, PriorReviewComment, PriorReviewFeedback, ReviewComment, ReviewCommentDraft,
    ScmIdentity, ScmProvider,
};
use anyhow::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
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
struct PullRequestFilePatch {
    filename: String,
    #[serde(default)]
    previous_filename: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    patch: Option<String>,
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
struct GitHubTree {
    tree: Vec<GitHubTreeEntry>,
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubTreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct CollaboratorPermission {
    permission: String,
}

#[derive(Debug, Deserialize)]
struct PullRequestReview {
    body: Option<String>,
    user: Option<ScmIdentity>,
}

#[derive(Debug, Deserialize)]
struct AuthenticatedUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct IssueComment {
    id: u64,
    body: Option<String>,
    user: Option<ScmIdentity>,
}

#[derive(Debug, Serialize)]
struct CreateIssueComment<'a> {
    body: &'a str,
}

#[derive(Debug, Serialize)]
struct ReviewCommentBody<'a> {
    body: &'a str,
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

#[derive(Debug, Serialize)]
struct PullRequestReviewBody<'a> {
    body: &'a str,
    commit_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct CreatedPullRequestReview {
    id: u64,
}

#[derive(Debug, Serialize)]
struct SubmitPullRequestReview<'a> {
    event: &'a str,
}

#[derive(Debug, Serialize)]
struct RequestReviewers<'a> {
    reviewers: [&'a str; 1],
}

#[derive(Debug, Deserialize)]
struct GitHubCheckAnnotation {
    pub path: String,
    #[serde(rename = "start_line")]
    pub start_line: Option<u32>,
    #[serde(rename = "end_line")]
    pub end_line: Option<u32>,
    #[serde(rename = "annotation_level")]
    pub annotation_level: String,
    pub message: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "raw_details")]
    #[serde(default)]
    pub raw_details: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CheckRunList {
    #[serde(default)]
    check_runs: Vec<CheckRun>,
}

#[derive(Debug, Deserialize)]
struct CheckRun {
    id: u64,
    name: String,
    #[serde(default)]
    conclusion: Option<String>,
}

impl GitHubClient {
    pub fn from_scm_config(config: &ScmConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(
            config.provider.eq_ignore_ascii_case("github"),
            "the configured source-control provider is not implemented by this adapter"
        );
        let identity = RepositoryIdentity::parse(&config.repository)?;
        validate_github_adapter(&identity, &config.provider)?;
        let (owner, repo) = identity.owner_and_repository()?;
        let adapter_config = GitHubConfig {
            token: config.token.clone(),
            repository: format!("{owner}/{repo}"),
            owner: owner.to_string(),
            repo: repo.to_string(),
            pr_number: config.change_request_number,
            api_url: config.api_url.clone(),
            server_url: config.web_url.clone(),
        };
        Self::new(&adapter_config)
    }

    pub fn new(cfg: &GitHubConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("cururu/0.1")
            .build()?;
        Ok(Self {
            client,
            cfg: cfg.clone(),
        })
    }

    pub async fn fetch_pr_diff_with_limit(
        &self,
        max_bytes: usize,
    ) -> anyhow::Result<(String, bool)> {
        if max_bytes == 0 {
            return Ok((String::new(), false));
        }
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let mut response = retry_with_backoff(
            || async {
                self.client
                    .get(&url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github.diff")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!("transport error while fetching pull request diff")
                    })
            },
            3,
        )
        .await?;

        if response.status() == reqwest::StatusCode::NOT_ACCEPTABLE {
            return self.fetch_pr_diff_from_files(max_bytes).await;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = read_response_prefix(&mut response, 4096).await;
            let body = String::from_utf8_lossy(&body);
            let detail = self.safe_provider_detail(&body);
            anyhow::bail!(
                "pull request diff request failed (HTTP {status}){}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }

        let read_limit = max_bytes.saturating_add(1);
        let mut diff = Vec::with_capacity(read_limit.min(64 * 1024));
        while diff.len() < read_limit {
            let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("failed while streaming pull request diff"))?
            else {
                break;
            };
            let copy_len = (read_limit - diff.len()).min(chunk.len());
            diff.extend_from_slice(&chunk[..copy_len]);
            if copy_len < chunk.len() {
                break;
            }
        }
        let truncated = diff.len() > max_bytes;
        diff.truncate(max_bytes);
        Ok((String::from_utf8_lossy(&diff).into_owned(), truncated))
    }

    async fn fetch_pr_diff_from_files(&self, max_bytes: usize) -> anyhow::Result<(String, bool)> {
        const PAGE_SIZE: usize = 100;
        const MAX_FILES: usize = 3_000;
        let mut result = String::new();
        let mut page = 1usize;
        let mut file_count = 0usize;
        let mut missing_patches = 0usize;
        let mut truncated = false;

        loop {
            let url = format!(
                "{}/repos/{}/{}/pulls/{}/files?per_page={PAGE_SIZE}&page={page}",
                self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
            );
            let mut response = retry_with_backoff(
                || async {
                    self.client
                        .get(&url)
                        .timeout(Duration::from_secs(30))
                        .header("Accept", "application/vnd.github+json")
                        .header("X-GitHub-Api-Version", "2026-03-10")
                        .bearer_auth(&self.cfg.token)
                        .send()
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!("transport error while fetching changed-file patches")
                        })
                },
                3,
            )
            .await?;

            if !response.status().is_success() {
                let status = response.status();
                let body = read_response_prefix(&mut response, 4096).await;
                let body = String::from_utf8_lossy(&body);
                let detail = self.safe_provider_detail(&body);
                anyhow::bail!(
                    "changed-file patch request failed (HTTP {status}){}",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                );
            }

            let files: Vec<PullRequestFilePatch> = response
                .json()
                .await
                .map_err(|_| anyhow::anyhow!("invalid changed-file patch response"))?;
            let page_len = files.len();
            for file in files {
                file_count += 1;
                if file_count > MAX_FILES {
                    anyhow::bail!("changed-file patch fallback exceeded the supported file count");
                }
                let Some(patch) = file.patch else {
                    missing_patches += 1;
                    continue;
                };
                let old_path = file.previous_filename.as_deref().unwrap_or(&file.filename);
                let mut file_diff = format!("diff --git a/{old_path} b/{}\n", file.filename);
                match file.status.as_deref() {
                    Some("added") => file_diff.push_str("--- /dev/null\n"),
                    _ => {
                        let _ = writeln!(file_diff, "--- a/{old_path}");
                    }
                }
                match file.status.as_deref() {
                    Some("removed") => file_diff.push_str("+++ /dev/null\n"),
                    _ => {
                        let _ = writeln!(file_diff, "+++ b/{}", file.filename);
                    }
                }
                file_diff.push_str(&patch);
                if !file_diff.ends_with('\n') {
                    file_diff.push('\n');
                }
                if result.len().saturating_add(file_diff.len()) > max_bytes {
                    truncated = true;
                    break;
                }
                result.push_str(&file_diff);
            }

            if truncated || page_len < PAGE_SIZE {
                break;
            }
            page += 1;
        }

        if missing_patches > 0 {
            anyhow::bail!(
                "the host omitted patches for {missing_patches} changed file(s); refusing to publish an incomplete review"
            );
        }
        Ok((result, truncated))
    }

    fn safe_provider_detail(&self, body: &str) -> String {
        let mut detail = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| body.to_string());
        let mut values = vec![
            self.cfg.token.as_str(),
            self.cfg.repository.as_str(),
            self.cfg.owner.as_str(),
            self.cfg.repo.as_str(),
            self.cfg.api_url.as_str(),
            self.cfg.server_url.as_str(),
        ];
        let encoded_repository = url_encode(&self.cfg.repository);
        values.push(&encoded_repository);
        let pull_number = self.cfg.pr_number.to_string();
        values.push(&pull_number);
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        for value in values.into_iter().filter(|value| !value.is_empty()) {
            if let Ok(pattern) = regex::RegexBuilder::new(&regex::escape(value))
                .case_insensitive(true)
                .build()
            {
                detail = pattern.replace_all(&detail, "[redacted]").into_owned();
            }
        }
        detail.chars().take(500).collect()
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

    pub async fn fetch_repository_file_at_ref(
        &self,
        repository: &str,
        path: &str,
        sha: &str,
    ) -> anyhow::Result<String> {
        let (owner, repo) = repository
            .split_once('/')
            .context("repository must be owner/repository")?;
        let encoded_path = path
            .split('/')
            .map(url_encode)
            .collect::<Vec<_>>()
            .join("/");
        let url = format!(
            "{}/repos/{owner}/{repo}/contents/{encoded_path}?ref={}",
            self.cfg.api_url,
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
            .context("failed to fetch shared config")?
            .error_for_status()
            .context("shared config API error; verify that the GitHub token can read the base repository")?
            .text()
            .await
            .context("failed to read shared config")
    }

    pub async fn list_repository_paths_at_revision(
        &self,
        sha: &str,
    ) -> anyhow::Result<Vec<String>> {
        let url = format!(
            "{}/repos/{}/{}/git/trees/{}?recursive=1",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, sha
        );
        let tree: GitHubTree = retry_with_backoff(
            || async {
                self.client
                    .get(&url)
                    .timeout(Duration::from_secs(30))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await
                    .context("failed to list repository tree")
            },
            3,
        )
        .await?;
        if tree.truncated {
            tracing::warn!(
                "source-control tree response was truncated; context matching may be incomplete"
            );
        }
        Ok(tree
            .tree
            .into_iter()
            .filter(|entry| entry.kind == "blob")
            .map(|entry| entry.path)
            .collect())
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

    /// Resolve the authenticated user's login, when the token permits it.
    ///
    /// GitHub Actions tokens (`GITHUB_TOKEN`) cannot call `GET /user` and return
    /// 403. In that case we fall back to `None` and identify Cururu's own
    /// comments by bot type combined with the exclusive Cururu marker, so we
    /// never touch another bot's comments.
    async fn current_login(&self) -> Option<String> {
        let url = format!("{}/user", self.cfg.api_url);
        let user = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json::<AuthenticatedUser>()
            .await
            .ok()?;
        Some(user.login)
    }

    /// Match a comment user against Cururu's own identity.
    ///
    /// Prefers the authenticated login when available; otherwise matches bots
    /// whose comments carry the exclusive Cururu marker.
    fn user_is_cururu(user: Option<&ScmIdentity>, own_login: Option<&str>) -> bool {
        let user_login = user.map(|u| u.login.as_str());
        let is_bot = user.is_some_and(|u| u.kind == "Bot");
        own_login.is_some_and(|own| user_login == Some(own)) || (own_login.is_none() && is_bot)
    }

    pub(crate) fn comment_is_cururu(comment: &ReviewComment, own_login: Option<&str>) -> bool {
        Self::user_is_cururu(comment.user.as_ref(), own_login)
            && comment
                .body
                .as_deref()
                .is_some_and(|b| b.contains(output::finding_marker()))
    }

    fn issue_comment_is_cururu(comment: &IssueComment, own_login: Option<&str>) -> bool {
        Self::user_is_cururu(comment.user.as_ref(), own_login)
            && comment
                .body
                .as_deref()
                .is_some_and(|b| b.contains(output::marker()))
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

    /// List check-run annotations for a commit SHA, optionally filtered to the
    /// configured check run names. Used to ingest analyzer evidence that is
    /// reported through GitHub Check Runs instead of a SARIF artifact.
    pub async fn list_check_annotations(
        &self,
        head_sha: &str,
        names: &[String],
    ) -> anyhow::Result<Vec<FindingAnnotation>> {
        let runs_url = format!(
            "{}/repos/{}/{}/commits/{}/check-runs?per_page=100",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, head_sha
        );
        let list: CheckRunList = retry_with_backoff(
            || async {
                self.client
                    .get(&runs_url)
                    .timeout(Duration::from_secs(15))
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2026-03-10")
                    .bearer_auth(&self.cfg.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<CheckRunList>()
                    .await
                    .context("failed to list check runs")
            },
            3,
        )
        .await?;

        let mut annotations = Vec::new();
        for run in list.check_runs {
            if !names.is_empty() && !names.iter().any(|n| n == &run.name) {
                continue;
            }
            if run.conclusion.as_deref() == Some("success") {
                continue;
            }
            let ann_url = format!(
                "{}/repos/{}/{}/check-runs/{}/annotations?per_page=100",
                self.cfg.api_url, self.cfg.owner, self.cfg.repo, run.id
            );
            let page: Vec<GitHubCheckAnnotation> = retry_with_backoff(
                || async {
                    self.client
                        .get(&ann_url)
                        .timeout(Duration::from_secs(15))
                        .header("Accept", "application/vnd.github+json")
                        .header("X-GitHub-Api-Version", "2026-03-10")
                        .bearer_auth(&self.cfg.token)
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<Vec<GitHubCheckAnnotation>>()
                        .await
                        .context("failed to list check annotations")
                },
                3,
            )
            .await?;
            annotations.extend(page.into_iter().map(|annotation| FindingAnnotation {
                path: annotation.path,
                line: annotation.start_line.or(annotation.end_line),
                severity: annotation.annotation_level,
                title: annotation.title,
                message: annotation.message,
                details: annotation.raw_details,
            }));
        }
        Ok(annotations)
    }

    pub async fn list_review_comments(&self) -> anyhow::Result<Vec<ReviewComment>> {
        const PAGE_SIZE: usize = 100;
        const MAX_PAGES: usize = 100;
        let mut comments = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = format!(
                "{}/repos/{}/{}/pulls/{}/comments?per_page={PAGE_SIZE}&page={page}",
                self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
            );
            let mut page_comments: Vec<ReviewComment> = retry_with_backoff(
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
            .await?;
            let is_last_page = page_comments.len() < PAGE_SIZE;
            comments.append(&mut page_comments);
            if is_last_page {
                return Ok(comments);
            }
        }
        anyhow::bail!("review comment history exceeds the supported pagination limit")
    }

    /// Collect bounded human and Cururu replies to prior findings. Historical
    /// comments are discussion data, not instructions to the reviewer.
    pub async fn fetch_prior_review_feedback(&self) -> anyhow::Result<Vec<PriorReviewFeedback>> {
        const MAX_THREADS: usize = 20;
        const MAX_REPLIES_PER_THREAD: usize = 10;
        const MAX_COMMENT_CHARS: usize = 2_000;

        let comments = self.list_review_comments().await?;
        let own_login = self.current_login().await;
        let mut roots: Vec<_> = comments
            .iter()
            .filter(|comment| {
                comment.in_reply_to_id.is_none()
                    && Self::comment_is_cururu(comment, own_login.as_deref())
            })
            .filter_map(|root| {
                let root_body = root.body.as_deref()?.trim();
                let replies: Vec<_> = comments
                    .iter()
                    .filter(|reply| reply.in_reply_to_id == Some(root.id))
                    .filter_map(|reply| {
                        let user = reply.user.as_ref()?;
                        let is_cururu = user.login.eq_ignore_ascii_case(&root.user.as_ref()?.login);
                        (user.kind != "Bot" || is_cururu).then(|| PriorReviewComment {
                            author: user.login.clone(),
                            body: truncate_chars(
                                reply.body.as_deref().unwrap_or_default(),
                                MAX_COMMENT_CHARS,
                            ),
                        })
                    })
                    .take(MAX_REPLIES_PER_THREAD)
                    .collect();
                (!replies.is_empty()).then(|| {
                    let line = root
                        .line
                        .map_or_else(|| "file".to_string(), |n| format!("line {n}"));
                    let mut discussion = Vec::with_capacity(replies.len() + 1);
                    discussion.push(PriorReviewComment {
                        author: root
                            .user
                            .as_ref()
                            .map_or_else(|| "Cururu".into(), |u| u.login.clone()),
                        body: truncate_chars(root_body, MAX_COMMENT_CHARS),
                    });
                    discussion.extend(replies);
                    (
                        root.id,
                        PriorReviewFeedback {
                            location: format!("{} ({line})", root.path),
                            comments: discussion,
                        },
                    )
                })
            })
            .collect();
        roots.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
        let mut feedback: Vec<_> = roots
            .into_iter()
            .take(MAX_THREADS)
            .map(|(_, feedback)| feedback)
            .collect();
        feedback.reverse();

        if let Some(issue_feedback) = self
            .fetch_issue_comment_feedback(own_login.as_deref())
            .await?
        {
            feedback.push(issue_feedback);
        }
        Ok(bound_review_feedback(feedback, 12_000))
    }

    async fn fetch_issue_comment_feedback(
        &self,
        own_login: Option<&str>,
    ) -> anyhow::Result<Option<PriorReviewFeedback>> {
        const MAX_COMMENTS: usize = 10;
        const MAX_COMMENT_CHARS: usize = 2_000;

        let comments = self.list_issue_comments().await?;
        let Some(summary) = comments.iter().find(|comment| {
            Self::issue_comment_is_cururu(comment, own_login)
                && comment
                    .body
                    .as_deref()
                    .is_some_and(|body| body.contains(output::marker()))
        }) else {
            return Ok(None);
        };
        let summary_login = summary.user.as_ref().map(|user| user.login.as_str());
        let mut replies: Vec<_> = comments
            .iter()
            .filter(|comment| comment.id > summary.id)
            .filter_map(|comment| {
                let user = comment.user.as_ref()?;
                let is_cururu =
                    summary_login.is_some_and(|login| user.login.eq_ignore_ascii_case(login));
                (user.kind != "Bot" || is_cururu).then(|| {
                    (
                        comment.id,
                        PriorReviewComment {
                            author: user.login.clone(),
                            body: truncate_chars(
                                comment.body.as_deref().unwrap_or_default(),
                                MAX_COMMENT_CHARS,
                            ),
                        },
                    )
                })
            })
            .collect();
        replies.sort_by_key(|(id, _)| *id);
        let replies: Vec<_> = replies
            .into_iter()
            .rev()
            .take(MAX_COMMENTS)
            .map(|(_, comment)| comment)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if replies.is_empty() {
            return Ok(None);
        }
        Ok(Some(PriorReviewFeedback {
            location: "PR conversation after the previous Cururu summary".into(),
            comments: replies,
        }))
    }

    async fn list_issue_comments(&self) -> anyhow::Result<Vec<IssueComment>> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments?per_page=100&direction=desc",
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
                    .json::<Vec<IssueComment>>()
                    .await
                    .context("failed to list PR discussion comments")
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

    pub async fn create_pending_formal_review(
        &self,
        head_sha: &str,
        body: &str,
    ) -> anyhow::Result<u64> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/reviews",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let review: CreatedPullRequestReview = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .json(&PullRequestReviewBody {
                body,
                commit_id: head_sha,
                event: None,
            })
            .send()
            .await
            .context("failed to create pending Cururu review")?
            .error_for_status()
            .context("GitHub rejected the pending pull request review")?
            .json()
            .await
            .context("invalid pending pull request review response")?;
        Ok(review.id)
    }

    pub async fn submit_formal_review(&self, review_id: u64) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/reviews/{review_id}/events",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let result: anyhow::Result<()> = async {
            self.client
                .post(&url)
                .timeout(Duration::from_secs(15))
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2026-03-10")
                .bearer_auth(&self.cfg.token)
                .json(&SubmitPullRequestReview { event: "COMMENT" })
                .send()
                .await
                .context("failed to submit pending Cururu review")?
                .error_for_status()
                .context("GitHub rejected the formal pull request review")?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            if let Err(cleanup_error) = self.delete_pending_formal_review(review_id).await {
                return Err(error.context(format!(
                    "failed to clean up pending review after submission error: {cleanup_error:#}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    pub async fn delete_pending_formal_review(&self, review_id: u64) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/reviews/{review_id}",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        self.client
            .delete(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await
            .context("failed to discard pending Cururu review")?
            .error_for_status()
            .context("GitHub rejected pending review deletion")?;
        Ok(())
    }

    pub async fn formal_review_exists(
        &self,
        marker: &str,
        bot_login: &str,
    ) -> anyhow::Result<bool> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/reviews?per_page=100",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let reviews: Vec<PullRequestReview> = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .send()
            .await
            .context("failed to list pull request reviews")?
            .error_for_status()
            .context("GitHub rejected pull request review listing")?
            .json()
            .await
            .context("invalid pull request review list")?;
        Ok(reviews.iter().any(|review| {
            review
                .body
                .as_deref()
                .is_some_and(|body| body.contains(marker))
                && review
                    .user
                    .as_ref()
                    .is_some_and(|user| user.login.eq_ignore_ascii_case(bot_login))
        }))
    }

    /// Attempt to request the Cururu App bot as a reviewer. GitHub documents
    /// this endpoint for user and team logins; an App bot may be rejected with
    /// 422, which is reported as `false` so the formal review can still be posted.
    pub async fn request_app_reviewer(&self, login: &str) -> anyhow::Result<bool> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/requested_reviewers",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .json(&RequestReviewers { reviewers: [login] })
            .send()
            .await
            .context("failed to request Cururu as reviewer")?;
        if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            return Ok(false);
        }
        response
            .error_for_status()
            .context("GitHub rejected the Cururu reviewer request")?;
        Ok(true)
    }

    pub async fn reply_review_comment(&self, id: u64, body: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/comments/{id}/replies",
            self.cfg.api_url, self.cfg.owner, self.cfg.repo, self.cfg.pr_number
        );
        self.client
            .post(&url)
            .timeout(Duration::from_secs(15))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .bearer_auth(&self.cfg.token)
            .json(&ReviewCommentBody { body })
            .send()
            .await
            .context("failed to reply to pull request review comment")?
            .error_for_status()
            .context("GitHub rejected review-comment reply")?;
        Ok(())
    }

    /// Publish new inline findings while preserving all previously published
    /// comments as review history. Duplicate findings are identified by path
    /// and normalized body, independent of their current diff line.
    pub async fn reconcile_review_comments(
        &self,
        head_sha: &str,
        desired: &[ReviewCommentDraft],
    ) -> anyhow::Result<()> {
        let existing = self.list_review_comments().await?;
        let own_login = self.current_login().await;
        let cururu_existing: Vec<ReviewComment> = existing
            .into_iter()
            .filter(|c| Self::comment_is_cururu(c, own_login.as_deref()))
            .collect();

        // Map desired comments by (path, line) so multiple findings on the
        // same line merge into one comment.
        let desired_map = merge_desired_by_anchor(desired);

        let existing_findings: std::collections::HashSet<(String, String)> = cururu_existing
            .iter()
            .filter_map(|comment| {
                comment
                    .body
                    .as_deref()
                    .map(|body| (comment.path.clone(), normalize_finding_content(body)))
            })
            .collect();

        for (key, body) in &desired_map {
            if !existing_findings.contains(&(key.0.clone(), normalize_finding_content(body))) {
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
        let own_login = self.current_login().await;
        Ok(comments.iter().any(|comment| {
            Self::issue_comment_is_cururu(comment, own_login.as_deref())
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
        let own_login = self.current_login().await;

        Ok(comments
            .into_iter()
            .find(|c| Self::issue_comment_is_cururu(c, own_login.as_deref()))
            .map(|c| c.id))
    }

    pub async fn create_issue_comment(&self, body: &str) -> anyhow::Result<()> {
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
}

#[async_trait]
impl ScmProvider for GitHubClient {
    async fn fetch_head_sha(&self) -> anyhow::Result<String> {
        Self::fetch_head_sha(self).await
    }

    async fn fetch_diff_with_limit(&self, max_bytes: usize) -> anyhow::Result<(String, bool)> {
        Self::fetch_pr_diff_with_limit(self, max_bytes).await
    }

    async fn fetch_base_sha(&self) -> anyhow::Result<String> {
        Self::fetch_base_sha(self).await
    }

    async fn fetch_file_at_ref(&self, path: &str, revision: &str) -> anyhow::Result<String> {
        Self::fetch_file_at_ref(self, path, revision).await
    }

    async fn list_repository_paths_at_revision(
        &self,
        revision: &str,
    ) -> anyhow::Result<Vec<String>> {
        Self::list_repository_paths_at_revision(self, revision).await
    }

    async fn fetch_prior_review_feedback(&self) -> anyhow::Result<Vec<PriorReviewFeedback>> {
        Self::fetch_prior_review_feedback(self).await
    }

    async fn list_finding_annotations(
        &self,
        revision: &str,
        source_names: &[String],
    ) -> anyhow::Result<Vec<FindingAnnotation>> {
        Self::list_check_annotations(self, revision, source_names).await
    }

    async fn list_review_comments(&self) -> anyhow::Result<Vec<ReviewComment>> {
        Self::list_review_comments(self).await
    }

    async fn reconcile_review_comments(
        &self,
        revision: &str,
        desired: &[ReviewCommentDraft],
    ) -> anyhow::Result<()> {
        Self::reconcile_review_comments(self, revision, desired).await
    }

    async fn has_summary_for_revision(&self, revision: &str) -> anyhow::Result<bool> {
        Self::summary_has_head(self, revision).await
    }

    async fn upsert_summary_comment(&self, body: &str) -> anyhow::Result<()> {
        Self::upsert_summary_comment(self, body).await
    }

    async fn create_issue_comment(&self, body: &str) -> anyhow::Result<()> {
        Self::create_issue_comment(self, body).await
    }

    async fn reply_review_comment(&self, comment_id: u64, body: &str) -> anyhow::Result<()> {
        Self::reply_review_comment(self, comment_id, body).await
    }

    async fn user_can_review(&self, login: &str) -> anyhow::Result<bool> {
        Self::user_can_review(self, login).await
    }

    async fn request_cururu_as_reviewer(&self, login: &str) -> anyhow::Result<bool> {
        Self::request_app_reviewer(self, login).await
    }

    async fn formal_review_exists(&self, marker: &str, login: &str) -> anyhow::Result<bool> {
        Self::formal_review_exists(self, marker, login).await
    }

    async fn create_pending_formal_review(
        &self,
        revision: &str,
        body: &str,
    ) -> anyhow::Result<u64> {
        Self::create_pending_formal_review(self, revision, body).await
    }

    async fn delete_pending_formal_review(&self, review_id: u64) -> anyhow::Result<()> {
        Self::delete_pending_formal_review(self, review_id).await
    }

    async fn submit_formal_review(&self, review_id: u64) -> anyhow::Result<()> {
        Self::submit_formal_review(self, review_id).await
    }

    async fn fetch_config_toml(&self, revision: &str) -> anyhow::Result<Option<String>> {
        Self::fetch_config_toml(self, revision).await
    }

    async fn fetch_repository_file_at_ref(
        &self,
        repository: &str,
        path: &str,
        revision: &str,
    ) -> anyhow::Result<String> {
        Self::fetch_repository_file_at_ref(self, repository, path, revision).await
    }

    fn is_cururu_review_comment(&self, comment: &ReviewComment, login: Option<&str>) -> bool {
        Self::comment_is_cururu(comment, login)
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

async fn read_response_prefix(response: &mut reqwest::Response, max_bytes: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(max_bytes.min(1024));
    while body.len() < max_bytes {
        let Ok(Some(chunk)) = response.chunk().await else {
            break;
        };
        let remaining = max_bytes - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    body
}

fn bound_review_feedback(
    feedback: Vec<PriorReviewFeedback>,
    max_bytes: usize,
) -> Vec<PriorReviewFeedback> {
    let mut remaining = max_bytes;
    let mut bounded = Vec::new();
    for mut thread in feedback.into_iter().rev() {
        let mut comments = Vec::new();
        for mut comment in thread.comments.drain(..).rev() {
            if remaining == 0 {
                break;
            }
            if comment.body.len() > remaining {
                comment.body = truncate_utf8(&comment.body, remaining);
            }
            remaining = remaining.saturating_sub(comment.body.len());
            comments.push(comment);
        }
        if !comments.is_empty() {
            comments.reverse();
            thread.comments = comments;
            thread.location = truncate_chars(&thread.location, 512);
            remaining = remaining.saturating_sub(thread.location.len());
            bounded.push(thread);
        }
        if remaining == 0 {
            break;
        }
    }
    bounded.reverse();
    bounded
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    value
        .char_indices()
        .take_while(|(index, _)| *index < max_bytes)
        .map(|(_, character)| character)
        .collect()
}

fn normalize_finding_content(body: &str) -> String {
    body.replace(output::finding_marker(), "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
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
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    #[tokio::test]
    async fn bounded_pr_diff_stops_reading_at_the_byte_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1"))
            .and(header("accept", "application/vnd.github.diff"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0123456789".repeat(100)))
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        let (diff, truncated) = client.fetch_pr_diff_with_limit(32).await.unwrap();
        assert!(truncated);
        assert_eq!(diff, "0123456789".repeat(3) + "01");
    }

    #[tokio::test]
    async fn diff_http_errors_include_safe_detail_without_repository_identifiers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/private-owner/private-repo/pulls/81"))
            .and(header("accept", "application/vnd.github.diff"))
            .respond_with(ResponseTemplate::new(406).set_body_json(serde_json::json!({
                "message": "Could not generate diff for private-owner/private-repo pull 81"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/private-owner/private-repo/pulls/81/files"))
            .respond_with(ResponseTemplate::new(406).set_body_json(serde_json::json!({
                "message": "Could not generate diff for private-owner/private-repo pull 81"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "secret-token".into(),
            repository: "private-owner/private-repo".into(),
            owner: "private-owner".into(),
            repo: "private-repo".into(),
            pr_number: 81,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        let error = client
            .fetch_pr_diff_with_limit(4096)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("HTTP 406"));
        assert!(error.contains("Could not generate diff"));
        assert!(!error.contains("private-owner"));
        assert!(!error.contains("private-repo"));
        assert!(!error.contains("secret-token"));
        server.verify().await;
    }

    #[tokio::test]
    async fn falls_back_to_paginated_file_patches_when_host_rejects_full_diff() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1"))
            .and(header("accept", "application/vnd.github.diff"))
            .respond_with(ResponseTemplate::new(406).set_body_json(serde_json::json!({
                "message": "Diff representation unavailable"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/files"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .and(header("accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "filename": "src/lib.rs",
                    "status": "modified",
                    "patch": "@@ -1 +1 @@\n-old()\n+new()"
                }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        let (diff, truncated) = client.fetch_pr_diff_with_limit(4096).await.unwrap();

        assert!(!truncated);
        assert!(diff.contains("diff --git a/src/lib.rs b/src/lib.rs"));
        assert!(diff.contains("@@ -1 +1 @@\n-old()\n+new()"));
        server.verify().await;
    }

    #[tokio::test]
    async fn refuses_file_patch_fallback_when_host_omits_any_patch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1"))
            .respond_with(ResponseTemplate::new(406))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"filename":"assets/image.bin","status":"modified"}
            ])))
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        let error = client
            .fetch_pr_diff_with_limit(4096)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("host omitted patches for 1 changed file"));
    }

    #[tokio::test]
    async fn empty_review_reconciliation_preserves_previous_cururu_inline_comments() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 7,
                    "body": "<!-- cururu:finding --> Old inline finding.",
                    "user": {"login": "cururu[bot]", "type": "Bot"},
                    "path": "src/lib.rs",
                    "line": 12,
                    "subject_type": "line"
                }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/owner/repo/pulls/comments/7"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/owner/repo/pulls/comments/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        client
            .reconcile_review_comments("head-sha", &[])
            .await
            .unwrap();
        server.verify().await;
    }

    #[tokio::test]
    async fn review_reconciliation_does_not_repeat_same_finding_after_line_moves() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 7,
                    "body": "<!-- cururu:finding -->\n\n**HIGH**: Unsafe input\n\nThe value reaches a shell.",
                    "user": {"login": "cururu[bot]", "type": "Bot"},
                    "path": "src/lib.rs",
                    "line": 12,
                    "subject_type": "line"
                }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        client
            .reconcile_review_comments(
                "new-head",
                &[ReviewCommentDraft {
                    path: "src/lib.rs".into(),
                    line: Some(20),
                    body: "<!-- cururu:finding --> \n **high**:\n Unsafe Input   The value reaches a shell.".into(),
                }],
            )
            .await
            .unwrap();
        server.verify().await;
    }

    #[tokio::test]
    async fn review_finding_deduplication_checks_comment_pages_beyond_the_first() {
        let server = MockServer::start().await;
        let first_page: Vec<_> = (1..=100)
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "body": "human discussion",
                    "user": {"login": "reviewer", "type": "User"},
                    "path": "src/lib.rs",
                    "line": 1,
                    "subject_type": "line"
                })
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(first_page))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 101,
                    "body": "<!-- cururu:finding -->\n\n**HIGH**: Unsafe input\n\nThe value reaches a shell.",
                    "user": {"login": "cururu[bot]", "type": "Bot"},
                    "path": "src/lib.rs",
                    "line": 12,
                    "subject_type": "line"
                }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        client
            .reconcile_review_comments(
                "new-head",
                &[ReviewCommentDraft {
                    path: "src/lib.rs".into(),
                    line: Some(30),
                    body: "<!-- cururu:finding -->\n\n**HIGH**: Unsafe input\n\nThe value reaches a shell.".into(),
                }],
            )
            .await
            .unwrap();
        server.verify().await;
    }

    #[tokio::test]
    async fn changed_finding_is_added_without_editing_or_deleting_its_history() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 7,
                    "body": "<!-- cururu:finding -->\n\n**HIGH**: Unsafe input\n\nOld explanation.",
                    "user": {"login": "cururu[bot]", "type": "Bot"},
                    "path": "src/lib.rs",
                    "line": 12,
                    "subject_type": "line"
                }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .and(body_json(serde_json::json!({
                "body": "<!-- cururu:finding -->\n\n**HIGH**: Unsafe input\n\nNew explanation.",
                "commit_id": "new-head",
                "path": "src/lib.rs",
                "line": 12
            })))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/owner/repo/pulls/comments/7"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/owner/repo/pulls/comments/7"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        client
            .reconcile_review_comments(
                "new-head",
                &[ReviewCommentDraft {
                    path: "src/lib.rs".into(),
                    line: Some(12),
                    body: "<!-- cururu:finding -->\n\n**HIGH**: Unsafe input\n\nNew explanation."
                        .into(),
                }],
            )
            .await
            .unwrap();
        server.verify().await;
    }

    #[tokio::test]
    async fn fetches_shared_config_from_another_repository_with_token() {
        let server = MockServer::start().await;
        let commit = "0123456789abcdef0123456789abcdef01234567";
        Mock::given(method("GET"))
            .and(path(
                "/repos/acme/standards/contents/configs/cururu/base.toml",
            ))
            .and(query_param("ref", commit))
            .and(header("authorization", "Bearer installation-token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("version = 1\n"))
            .mount(&server)
            .await;

        let client = GitHubClient::new(&GitHubConfig {
            token: "installation-token".into(),
            repository: "consumer/repo".into(),
            owner: "consumer".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();
        let body = client
            .fetch_repository_file_at_ref("acme/standards", "configs/cururu/base.toml", commit)
            .await
            .unwrap();
        assert_eq!(body, "version = 1\n");
    }

    #[tokio::test]
    async fn unsupported_app_bot_reviewer_request_falls_back_without_failing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/1/requested_reviewers"))
            .respond_with(ResponseTemplate::new(422))
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();
        assert!(!client.request_app_reviewer("cururu[bot]").await.unwrap());
    }

    #[tokio::test]
    async fn formal_review_records_bot_identity_marker() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"body":"<!-- cururu:formal-review:v1 head=abc -->", "user":{"login":"cururu[bot]","type":"Bot"}}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/1/reviews"))
            .and(body_json(serde_json::json!({
                "body": "Cururu review.",
                "commit_id": "abc"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 42})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/1/reviews/42/events"))
            .and(body_json(serde_json::json!({"event": "COMMENT"})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();
        assert!(
            client
                .formal_review_exists("<!-- cururu:formal-review:v1 head=abc -->", "cururu[bot]")
                .await
                .unwrap()
        );
        let review_id = client
            .create_pending_formal_review("abc", "Cururu review.")
            .await
            .unwrap();
        client.submit_formal_review(review_id).await.unwrap();
    }

    #[tokio::test]
    async fn failed_formal_review_submission_discards_the_pending_review() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/repo/pulls/1/reviews/42/events"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/repos/owner/repo/pulls/1/reviews/42"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        let error = client.submit_formal_review(42).await.unwrap_err();
        assert!(error.to_string().contains("GitHub rejected"));
        server.verify().await;
    }

    #[tokio::test]
    async fn prior_review_feedback_uses_github_thread_replies() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 10,
                    "body": "<!-- cururu:finding --> Possible duplicate write.",
                    "user": {"login": "cururu[bot]", "type": "Bot"},
                    "path": "src/store.rs",
                    "line": 42,
                    "subject_type": "line"
                },
                {
                    "id": 11,
                    "in_reply_to_id": 10,
                    "body": "This operation is intentionally idempotent.",
                    "user": {"login": "maintainer", "type": "User"},
                    "path": "src/store.rs",
                    "line": 42,
                    "subject_type": "line"
                }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/repo/issues/1/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(Vec::<serde_json::Value>::new()))
            .mount(&server)
            .await;

        let client = GitHubClient::new(&GitHubConfig {
            token: "token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 1,
            api_url: server.uri(),
            server_url: "https://github.com".into(),
        })
        .unwrap();

        let feedback = client.fetch_prior_review_feedback().await.unwrap();
        let serialized = serde_json::to_string(&feedback).unwrap();
        assert!(serialized.contains("Possible duplicate write"));
        assert!(serialized.contains("intentionally idempotent"));
    }

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

    fn comment(body: &str, login: &str, kind: &str) -> ReviewComment {
        ReviewComment {
            id: 1,
            in_reply_to_id: None,
            body: Some(body.into()),
            user: Some(ScmIdentity {
                login: login.into(),
                kind: kind.into(),
            }),
            path: "a.rs".into(),
            line: None,
            subject_type: None,
        }
    }

    fn finding_comment(login: &str, kind: &str) -> ReviewComment {
        comment(&format!("{} body", output::finding_marker()), login, kind)
    }

    #[test]
    fn cururu_comment_matches_by_login_when_available() {
        let c = finding_comment("cururu[bot]", "Bot");
        assert!(GitHubClient::comment_is_cururu(&c, Some("cururu[bot]")));
    }

    #[test]
    fn cururu_comment_does_not_match_other_login() {
        let c = finding_comment("other[bot]", "Bot");
        assert!(!GitHubClient::comment_is_cururu(&c, Some("cururu[bot]")));
    }

    #[test]
    fn falls_back_to_bot_with_marker_when_login_unavailable() {
        let c = finding_comment("github-actions[bot]", "Bot");
        assert!(GitHubClient::comment_is_cururu(&c, None));
    }

    #[test]
    fn does_not_touch_other_bot_without_marker() {
        // Another bot without the exclusive Cururu marker is not ours.
        let c = comment("no marker here", "other[bot]", "Bot");
        assert!(!GitHubClient::comment_is_cururu(&c, None));
    }

    #[test]
    fn ignores_human_comments_when_login_unavailable() {
        let c = finding_comment("alice", "User");
        assert!(!GitHubClient::comment_is_cururu(&c, None));
    }
}
