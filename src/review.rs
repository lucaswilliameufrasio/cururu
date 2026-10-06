use crate::{
    agent, analysis,
    config::AppConfig,
    context::{self, ContextFile, ContextStore},
    diff, evaluation, provider,
    scm::ScmProvider,
};
use anyhow::Context;
use tracing::info;

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
    pub evaluation: Option<evaluation::EvaluationReport>,
}

#[allow(clippy::too_many_lines)]
pub async fn run_review(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
) -> anyhow::Result<ReviewOutput> {
    let head_sha = source_control.fetch_head_sha().await?;
    let max_download_bytes = config.review.max_diff_bytes.saturating_mul(2);
    let (raw_diff, truncated) = source_control
        .fetch_diff_with_limit(max_download_bytes)
        .await
        .context("failed to fetch change-request diff")?;
    anyhow::ensure!(
        !truncated,
        "change-request diff exceeds the configured review download limit; increase review.max_diff_bytes or split the change"
    );
    ensure_review_head_unchanged(&head_sha, &source_control.fetch_head_sha().await?)?;

    let files = diff::filter_ignored(diff::parse_unified_diff(&raw_diff), &config.review.ignore);
    ensure_review_diff_limits(
        &files,
        config.review.chunk_bytes,
        config.review.max_diff_bytes,
    )?;
    let chunks = diff::chunk_files(
        &files,
        config.review.chunk_bytes,
        config.review.max_diff_bytes,
    );
    info!(
        files = files.len(),
        chunks = chunks.len(),
        "reviewing change-request diff"
    );

    let mut context_store = fetch_repo_context(config, source_control).await?;
    if config.context.auto.enabled {
        append_auto_context(config, source_control, &mut context_store, &files).await?;
    }
    let context_rendered = if context_store.is_empty() {
        String::new()
    } else {
        context_store.render()
    };
    let prior_feedback = source_control.fetch_prior_review_feedback().await?;
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
        analysis::load_evidence(&config.analysis, &files, &head_sha, source_control).await?;
    let candidates = agent::collect_candidates(
        model.clone(),
        files.len(),
        chunk_results,
        &config.review.policy,
        analysis_report.findings.clone(),
    );
    let (review, mut evaluation_report) = if let Some(mode) = config.evaluator.mode {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("failed to build TypeSafe evaluator client")?;
        let mut report = evaluation::evaluate_with_jev(
            &client,
            &config.evaluator.api_key,
            &config.evaluator.model,
            &candidates.findings,
            &files,
        )
        .await?;
        let (judged_findings, judged_report) = evaluation::apply_judgments(
            candidates.findings.clone(),
            report
                .findings
                .iter()
                .map(|item| item.judgment.clone())
                .collect(),
            mode,
        )?;
        report.findings = judged_report.findings;
        let mut judged_candidates = candidates;
        judged_candidates.findings = judged_findings;
        (
            agent::apply_policy(judged_candidates, &config.review.policy),
            Some(report),
        )
    } else {
        (agent::apply_policy(candidates, &config.review.policy), None)
    };
    if let Some(report) = &mut evaluation_report {
        evaluation::mark_published(report, &review.findings);
    }

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
        evaluation: evaluation_report,
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

pub fn ensure_review_head_unchanged(expected: &str, current: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        expected == current,
        "PR head changed during review; any already-published findings remain anchored to the analyzed revision, and a fresh review is required"
    );
    Ok(())
}

fn ensure_review_diff_limits(
    files: &[diff::ChangedFile],
    chunk_bytes: usize,
    max_diff_bytes: usize,
) -> anyhow::Result<()> {
    let total_bytes = files
        .iter()
        .map(|file| file.patch.len())
        .fold(0usize, usize::saturating_add);
    anyhow::ensure!(
        total_bytes <= max_diff_bytes,
        "filtered change-request diff exceeds review.max_diff_bytes; increase the limit or split the change"
    );
    anyhow::ensure!(
        files.iter().all(|file| file.patch.len() <= chunk_bytes),
        "a changed file exceeds review.chunk_bytes; increase the chunk size or split the change"
    );
    Ok(())
}

async fn fetch_repo_context(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
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

    let base_sha = source_control
        .fetch_base_sha()
        .await
        .context("failed to fetch base commit SHA for context resolution")?;

    context::fetch_context(&config.context, source_control, &base_sha)
        .await
        .context("failed to fetch repository context")
}

async fn append_auto_context(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    store: &mut ContextStore,
    files: &[diff::ChangedFile],
) -> anyhow::Result<()> {
    let base_sha = source_control.fetch_base_sha().await?;
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

        let Ok(content) = source_control
            .fetch_file_at_ref(&changed.path, &base_sha)
            .await
        else {
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
    use super::{build_review_system_prompt, ensure_review_head_unchanged};

    #[test]
    fn rejects_review_inputs_that_would_be_silently_truncated() {
        let files = vec![crate::diff::ChangedFile {
            path: "src/a.rs".into(),
            patch: "a patch longer than limits".into(),
            right_lines: vec![1],
        }];
        assert!(
            super::ensure_review_diff_limits(&files, 100, 10)
                .unwrap_err()
                .to_string()
                .contains("max_diff_bytes")
        );
        assert!(
            super::ensure_review_diff_limits(&files, 10, 100)
                .unwrap_err()
                .to_string()
                .contains("chunk_bytes")
        );
    }

    #[test]
    fn rejects_review_results_when_pull_request_head_changed() {
        assert!(ensure_review_head_unchanged("abc", "abc").is_ok());
        let error = ensure_review_head_unchanged("abc", "def").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("already-published findings remain anchored")
        );
    }

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
