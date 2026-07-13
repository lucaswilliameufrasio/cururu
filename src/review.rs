use crate::{agent, config::AppConfig, context, diff, github::GitHubClient, provider};
use anyhow::Context;
use tracing::{info, warn};

const REVIEW_PROMPT: &str = include_str!("../prompts/review.md");

pub struct ReviewOutput {
    pub review: agent::ReviewResult,
    pub usage: Option<provider::ProviderUsage>,
    pub context_files: Vec<String>,
    pub model: String,
    pub show_usage: bool,
    pub show_cost: bool,
}

pub async fn run_review(config: &AppConfig, github: &GitHubClient) -> anyhow::Result<ReviewOutput> {
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

    let context_store = fetch_repo_context(config, github).await?;
    let context_rendered = context_store.render();
    info!(
        files = context_store.files.len(),
        "loaded repository context"
    );

    let system_prompt = if context_store.is_empty() {
        REVIEW_PROMPT.to_string()
    } else {
        format!("{REVIEW_PROMPT}{context_rendered}")
    };

    let agent = agent::build_agent(&config.llm, system_prompt)?;

    let mut chunk_results = Vec::new();
    for chunk in &chunks {
        let result = agent.review_chunk(chunk).await?;
        chunk_results.push(result);
    }

    let model = config.llm.model.clone();
    let usage = provider::merge_usage(&chunk_results);
    let review = agent::merge_results(model.clone(), files.len(), chunk_results);

    let context_paths: Vec<String> = context_store.files.iter().map(|f| f.path.clone()).collect();

    if let Some(ref u) = usage {
        info!(
            prompt_tokens = u.prompt_tokens,
            completion_tokens = u.completion_tokens,
            total_tokens = u.total_tokens,
            cost = ?u.cost,
            "LLM usage"
        );
    }

    Ok(ReviewOutput {
        review,
        usage,
        context_files: context_paths,
        model,
        show_usage: config.summary.show_usage,
        show_cost: config.summary.show_cost,
    })
}

async fn fetch_repo_context(
    config: &AppConfig,
    github: &GitHubClient,
) -> anyhow::Result<context::ContextStore> {
    if config.context.conventions.is_empty()
        && config.context.specifications.is_empty()
        && config.context.skills.is_empty()
        && config.context.additional.is_empty()
    {
        return Ok(context::ContextStore {
            files: Vec::new(),
            truncated: Vec::new(),
            skipped: Vec::new(),
        });
    }

    let base_sha = github
        .fetch_base_sha()
        .await
        .context("failed to fetch base commit SHA for context resolution")?;

    context::fetch_context(
        &config.context,
        &config.github.api_url,
        &config.github.token,
        &config.github.owner,
        &config.github.repo,
        &base_sha,
    )
    .await
    .context("failed to fetch repository context")
}
