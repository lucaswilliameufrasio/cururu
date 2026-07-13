use std::fmt::Write;

use crate::config::ContextConfig;
use anyhow::Context;
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct ContextStore {
    pub files: Vec<ContextFile>,
    pub truncated: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextFile {
    pub label: String,
    pub path: String,
    pub content: String,
}

impl ContextStore {
    pub const fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn render(&self) -> String {
        if self.files.is_empty() {
            return String::new();
        }
        let mut out = String::from("\n---\n## Repository context\n\n");
        for file in &self.files {
            let _ = write!(
                out,
                "### {}: `{}`\n\n```\n{}\n```\n\n",
                file.label, file.path, file.content
            );
        }
        if !self.truncated.is_empty() {
            out.push_str("Truncated files: ");
            out.push_str(&self.truncated.join(", "));
            out.push('\n');
        }
        if !self.skipped.is_empty() {
            out.push_str("Files not found: ");
            out.push_str(&self.skipped.join(", "));
            out.push('\n');
        }
        out
    }
}

#[derive(Debug, Deserialize)]
struct GitTree {
    tree: Vec<TreeEntry>,
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

pub async fn fetch_context(
    config: &ContextConfig,
    api_url: &str,
    token: &str,
    owner: &str,
    repo: &str,
    base_sha: &str,
) -> anyhow::Result<ContextStore> {
    let client = reqwest::Client::builder()
        .user_agent("cururu/0.1")
        .build()?;

    let tree_paths = fetch_tree(&client, api_url, token, owner, repo, base_sha).await?;

    let mut files = Vec::new();
    let mut truncated = Vec::new();
    let mut skipped = Vec::new();
    let mut total_bytes = 0usize;

    let labeled_patterns = [
        ("Conventions", &config.conventions),
        ("Specifications", &config.specifications),
        ("Skills", &config.skills),
        ("Additional", &config.additional),
    ];

    for &(label, patterns) in &labeled_patterns {
        for pattern in patterns {
            let matched: Vec<&String> = tree_paths
                .iter()
                .filter(|p| match_path(p, pattern))
                .collect();

            if matched.is_empty() {
                skipped.push(pattern.clone());
                continue;
            }

            for p in &matched {
                if total_bytes >= config.max_bytes {
                    truncated.push((*p).clone());
                    continue;
                }
                let content =
                    match fetch_raw(&client, api_url, token, owner, repo, base_sha, p).await {
                        Ok(c) => c,
                        Err(e) => {
                            warn!(path = %p, error = %e, "failed to fetch context file");
                            continue;
                        }
                    };

                let remaining = config.max_bytes.saturating_sub(total_bytes);
                if content.len() > remaining {
                    truncated.push((*p).clone());
                    files.push(ContextFile {
                        label: label.to_string(),
                        path: (*p).clone(),
                        content: content[..remaining].to_string(),
                    });
                    total_bytes = config.max_bytes;
                } else {
                    total_bytes += content.len();
                    files.push(ContextFile {
                        label: label.to_string(),
                        path: (*p).clone(),
                        content,
                    });
                }
            }
        }
    }

    Ok(ContextStore {
        files,
        truncated,
        skipped,
    })
}

async fn fetch_tree(
    client: &reqwest::Client,
    api_url: &str,
    token: &str,
    owner: &str,
    repo: &str,
    sha: &str,
) -> anyhow::Result<Vec<String>> {
    let url = format!("{api_url}/repos/{owner}/{repo}/git/trees/{sha}?recursive=1");
    let resp: GitTree = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| "failed to fetch git tree")?
        .error_for_status()
        .with_context(|| "git tree API error")?
        .json()
        .await
        .with_context(|| "failed to parse git tree")?;

    if resp.truncated {
        warn!("git tree response was truncated; context file resolution may be incomplete");
    }

    Ok(resp
        .tree
        .into_iter()
        .filter(|e| e.kind == "blob")
        .map(|e| e.path)
        .collect())
}

async fn fetch_raw(
    client: &reqwest::Client,
    api_url: &str,
    token: &str,
    owner: &str,
    repo: &str,
    sha: &str,
    path: &str,
) -> anyhow::Result<String> {
    let url = format!("{api_url}/repos/{owner}/{repo}/contents/{path}?ref={sha}");
    let text = client
        .get(&url)
        .header("Accept", "application/vnd.github.raw")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| "failed to fetch file content")?
        .error_for_status()
        .with_context(|| "file content API error")?
        .text()
        .await
        .with_context(|| "failed to read file content")?;
    Ok(text)
}

fn match_path(path: &str, pattern: &str) -> bool {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        let g = globset::Glob::new(pattern).ok();
        let set = g.and_then(|g| {
            let mut b = globset::GlobSetBuilder::new();
            b.add(g);
            b.build().ok()
        });
        set.is_some_and(|s| s.is_match(path))
    } else {
        path == pattern
    }
}
