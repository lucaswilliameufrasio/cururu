use std::fmt::Write;

use crate::{config::ContextConfig, scm::ScmProvider};
use anyhow::Context;
use cururu_core::context_path_matches;
pub use cururu_core::{ContextFile, ContextStore};

pub fn render(store: &ContextStore) -> String {
    if store.files.is_empty() && store.truncated.is_empty() && store.skipped.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n---\n## Repository context\n\n");
    for file in &store.files {
        let _ = write!(
            out,
            "### {}: `{}`\n\n```\n{}\n```\n\n",
            file.label, file.path, file.content
        );
    }
    if !store.truncated.is_empty() {
        out.push_str("Truncated files: ");
        out.push_str(&store.truncated.join(", "));
        out.push('\n');
    }
    if !store.skipped.is_empty() {
        out.push_str("Files not found: ");
        out.push_str(&store.skipped.join(", "));
        out.push('\n');
    }
    out
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
                .filter(|p| context_path_matches(p, pattern))
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
                let content = source_control
                    .fetch_file_at_ref(p, base_sha)
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "failed to fetch configured context file from the SCM provider"
                        )
                    })?;

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

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    value
        .char_indices()
        .take_while(|(index, character)| index.saturating_add(character.len_utf8()) <= max_bytes)
        .map(|(_, character)| character)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ContextFile, ContextStore, render};

    #[test]
    fn truncation_never_exceeds_byte_limit_for_multibyte_text() {
        assert_eq!(super::truncate_utf8("xéz", 2), "x");
        assert_eq!(super::truncate_utf8("xéz", 3), "xé");
    }

    #[test]
    fn rendering_preserves_the_existing_prompt_context_format() {
        let store = ContextStore {
            files: vec![ContextFile {
                label: "Conventions".into(),
                path: "CONTRIBUTING.md".into(),
                content: "Check errors.".into(),
            }],
            truncated: vec!["large.md".into()],
            skipped: vec!["missing.md".into()],
        };

        assert_eq!(
            render(&store),
            "\n---\n## Repository context\n\n### Conventions: `CONTRIBUTING.md`\n\n```\nCheck errors.\n```\n\nTruncated files: large.md\nFiles not found: missing.md\n"
        );
    }

    #[test]
    fn rendering_empty_context_preserves_missing_and_truncation_diagnostics() {
        assert_eq!(
            render(&ContextStore {
                files: Vec::new(),
                truncated: vec!["large.md".into()],
                skipped: vec!["missing.md".into()],
            }),
            "\n---\n## Repository context\n\nTruncated files: large.md\nFiles not found: missing.md\n"
        );
    }

    struct FailedContextScm;

    #[async_trait::async_trait]
    impl crate::scm::ScmProvider for FailedContextScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((String::new(), false))
        }
        async fn list_repository_paths_at_revision(&self, _: &str) -> anyhow::Result<Vec<String>> {
            Ok(vec!["rules.md".into()])
        }
        async fn fetch_file_at_ref(&self, _: &str, _: &str) -> anyhow::Result<String> {
            anyhow::bail!("private fixture detail")
        }
    }

    #[tokio::test]
    async fn configured_context_read_failure_is_not_silently_omitted() {
        let mut config = crate::config::AppConfig::from_local_env().unwrap().context;
        config.additional = vec!["rules.md".into()];
        let error = super::fetch_context(&config, &FailedContextScm, "base")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to fetch configured context file"));
        assert!(!error.contains("private fixture detail"));
    }
}
