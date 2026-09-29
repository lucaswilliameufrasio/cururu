use std::fmt::Write;

use crate::{config::ContextConfig, scm::ScmProvider};
use anyhow::Context;
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

pub async fn fetch_context(
    config: &ContextConfig,
    source_control: &dyn ScmProvider,
    base_sha: &str,
) -> anyhow::Result<ContextStore> {
    let tree_paths = source_control
        .list_repository_paths_at_revision(base_sha)
        .await
        .context("failed to list repository paths for context matching")?;

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
                let Ok(content) = source_control.fetch_file_at_ref(p, base_sha).await else {
                    warn!("failed to fetch a context file from the SCM provider");
                    continue;
                };

                let remaining = config.max_bytes.saturating_sub(total_bytes);
                if content.len() > remaining {
                    truncated.push((*p).clone());
                    files.push(ContextFile {
                        label: label.to_string(),
                        path: (*p).clone(),
                        content: truncate_utf8(&content, remaining),
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
