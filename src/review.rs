use crate::{agent, config::AppConfig, diff, github::GitHubClient};
use anyhow::Context;
use tracing::{info, warn};

const REVIEW_PROMPT: &str = include_str!("../prompts/review.md");

pub async fn run_review(
    config: &AppConfig,
    github: &GitHubClient,
) -> anyhow::Result<agent::ReviewResult> {
    let raw_diff = github
        .fetch_pr_diff()
        .await
        .context("failed to fetch PR diff")?;
    if raw_diff.len() > config.review.max_diff_bytes * 2 {
        warn!(
            bytes = raw_diff.len(),
            "very large diff; will truncate after filtering/chunking"
        );
    }

    let files = diff::filter_ignored(diff::parse_unified_diff(&raw_diff), &config.review.ignore);
    let chunks = diff::chunk_files(
        &files,
        config.review.chunk_bytes,
        config.review.max_diff_bytes,
    );
    info!(files = files.len(), chunks = chunks.len(), pr = %github.pr_url(), "reviewing PR diff");

    let agent = agent::build_agent(&config.llm, REVIEW_PROMPT.to_string())?;

    let mut results = Vec::new();
    for chunk in &chunks {
        results.push(agent.review_chunk(chunk).await?);
    }

    Ok(agent::merge_results(
        config.llm.model.clone(),
        files.len(),
        results,
    ))
}
