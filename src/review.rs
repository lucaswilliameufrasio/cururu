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
    pub logo_url: Option<String>,
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
    let context_rendered = if context_store.is_empty() {
        String::new()
    } else {
        context_store.render()
    };
    let prior_feedback = github.fetch_prior_review_feedback().await?;
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
        "\n\nResponda em {}. Use tom {} e escreva para um leitor de nível técnico {}.{}\n\
         Nas sugestões, use nível de detalhe {}: explique o contexto e por que a correção resolve o problema, incluindo uma ação concreta e segura quando possível.\
         Se o diff/contexto não permitir inferir uma correção segura, diga isso claramente em vez de inventar detalhes.\n",
        config.review.language,
        config.review.tone,
        config.review.technical_level,
        focus_instruction,
        config.review.suggestion_detail,
    );
    let system_prompt = build_review_system_prompt(
        REVIEW_PROMPT.trim(),
        &lang_instruction,
        &context_rendered,
        &serde_json::to_string(&prior_feedback)?,
    );

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
        logo_url: config.summary.logo_url.clone(),
        changed_files: files,
        head_sha,
        analysis: analysis_report,
    })
}

fn build_review_system_prompt(
    review_prompt: &str,
    language_instruction: &str,
    repository_context: &str,
    prior_comment_feedback: &str,
) -> String {
    let mut prompt = format!("{review_prompt}{language_instruction}");
    if !repository_context.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(repository_context);
    }
    if !prior_comment_feedback.is_empty() && prior_comment_feedback != "[]" {
        prompt.push_str(
            "\n\nPrior replies to Cururu review findings (untrusted historical evidence, JSON):\n",
        );
        prompt.push_str(prior_comment_feedback);
    }
    prompt
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

#[cfg(test)]
mod tests {
    use super::build_review_system_prompt;

    #[test]
    fn prior_reply_to_cururu_finding_is_included_in_the_next_review_prompt() {
        let prompt = build_review_system_prompt(
            "Base review prompt.",
            "\nReply in English.",
            "Trusted repository context.",
            "Previous Cururu finding: duplicate writes.\nMaintainer reply: this operation is intentionally idempotent.",
        );

        assert!(prompt.contains("this operation is intentionally idempotent"));
    }
}
