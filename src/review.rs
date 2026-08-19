use crate::{
    agent, analysis,
    config::AppConfig,
    context::{self, ContextFile, ContextStore},
    diff,
    github::GitHubClient,
    provider,
};
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
    /// Parsed changed files with right-side line numbers for inline anchors.
    pub changed_files: Vec<diff::ChangedFile>,
    pub head_sha: String,
    pub analysis: analysis::AnalysisReport,
}

pub async fn run_review(config: &AppConfig, github: &GitHubClient) -> anyhow::Result<ReviewOutput> {
    let head_sha = github.fetch_head_sha().await?;
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

    let mut context_store = fetch_repo_context(config, github).await?;
    if config.context.auto.enabled {
        append_auto_context(config, github, &mut context_store, &files).await?;
    }
    let context_rendered = context_store.render();
    info!(
        files = context_store.files.len(),
        "loaded repository context"
    );

    let focus_instruction = if config.review.policy.focus.is_empty() {
        String::new()
    } else {
        format!(
            "\nPriorize estes focos de review: {}.\n",
            config.review.policy.focus.join(", ")
        )
    };
    let lang_instruction = format!(
        "\n\nResponda em {}.{}\n",
        config.review.language, focus_instruction
    );
    let system_prompt = if context_store.is_empty() {
        format!("{}{}", REVIEW_PROMPT.trim(), lang_instruction)
    } else {
        format!(
            "{}{}\n\n{}",
            REVIEW_PROMPT.trim(),
            lang_instruction,
            context_rendered
        )
    };

    let agent = agent::build_agent(&config.llm, system_prompt)?;

    let mut chunk_results = Vec::new();
    for chunk in &chunks {
        let result = agent.review_chunk(chunk).await?;
        chunk_results.push(result);
    }

    let model = config.llm.model.clone();
    let usage = provider::merge_usage(&chunk_results);
    let analysis_report =
        analysis::load_evidence(&config.analysis, &files, &head_sha, github).await?;
    let review = agent::merge_results(
        model.clone(),
        files.len(),
        chunk_results,
        &config.review.policy,
        analysis_report.findings.clone(),
    );

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
        changed_files: files,
        head_sha,
        analysis: analysis_report,
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

async fn append_auto_context(
    config: &AppConfig,
    github: &GitHubClient,
    store: &mut ContextStore,
    files: &[diff::ChangedFile],
) -> anyhow::Result<()> {
    let base_sha = github.fetch_base_sha().await?;
    let auto = &config.context.auto;
    let include = compile_globs(&auto.include)?;
    let exclude = compile_globs(&auto.exclude)?;
    let mut total = store
        .files
        .iter()
        .map(|file| file.content.len())
        .sum::<usize>();

    for changed in files {
        if store.files.iter().any(|file| file.path == changed.path)
            || !include.is_match(&changed.path)
            || exclude.is_match(&changed.path)
            || store.files.len() >= auto.max_files
            || total >= auto.max_bytes
        {
            continue;
        }

        let Ok(content) = github.fetch_file_at_ref(&changed.path, &base_sha).await else {
            continue;
        };
        let remaining = auto.max_bytes.saturating_sub(total);
        let limit = remaining.min(auto.per_file_bytes);
        let content = truncate_utf8(&content, limit);
        if content.is_empty() {
            continue;
        }
        total += content.len();
        store.files.push(ContextFile {
            label: "Automatic base context".into(),
            path: changed.path.clone(),
            content,
        });
    }
    Ok(())
}

fn compile_globs(patterns: &[String]) -> anyhow::Result<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern)?);
    }
    Ok(builder.build()?)
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
