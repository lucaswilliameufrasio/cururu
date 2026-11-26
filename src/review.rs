use crate::{
    agent, analysis,
    config::AppConfig,
    context::{self, ContextFile, ContextStore},
    diff, evaluation, provider,
    scm::ScmProvider,
};
use anyhow::Context;
pub use cururu_core::{ensure_review_diff_limits, ensure_review_head_unchanged};
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
#[error(
    "change-request diff exceeds the configured review download limit; increase review.max_diff_bytes or split the change"
)]
pub struct DiffDownloadLimit {
    pub head_sha: String,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "filtered change-request diff exceeds review.max_diff_bytes; increase the limit or split the change"
)]
pub struct DiffTotalLimit {
    pub head_sha: String,
}

/// Publish a separate incomplete-review notice without replacing review history.
pub async fn publish_diff_limit_notice(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    error: &anyhow::Error,
) -> anyhow::Result<bool> {
    let head = if let Some(limit) = error.downcast_ref::<DiffDownloadLimit>() {
        &limit.head_sha
    } else if let Some(limit) = error.downcast_ref::<DiffTotalLimit>() {
        &limit.head_sha
    } else {
        return Ok(false);
    };
    ensure_review_head_unchanged(head, &source_control.fetch_head_sha().await?)?;
    let body = crate::output::render_diff_limit_notice(
        &config.review.language,
        config.review.max_diff_bytes,
        head,
    );
    source_control.upsert_limit_notice(&body).await?;
    Ok(true)
}

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
    /// Unique identifier of this review run.
    pub run_id: String,
    /// Usage of every recorded review run on the PR, including this one.
    pub pr_usage: Option<crate::review_usage::PrUsage>,
    /// Changed paths excluded from review by configuration.
    pub ignored_paths: Vec<String>,
    /// Binary files and submodule changes left out of the review.
    pub unreviewable_paths: Vec<String>,
    /// Files sent only partially because a diff line is longer than a chunk.
    pub truncated_paths: Vec<String>,
    /// Files in chunks whose coverage the model never reported consistently.
    pub unconfirmed_paths: Vec<String>,
    /// Findings dropped because the model placed them outside the diff.
    pub dropped_findings: usize,
    /// Auto-approval decision, when the policy is enabled.
    pub auto_approval: Option<crate::auto_approval::Decision>,
    /// Configured review language, used for Cururu's own explanations.
    pub language: String,
    /// Files this review covered with their patch hashes, recorded in the
    /// summary when `policy.incremental = "changed_files"`.
    pub reviewed_files: Option<crate::incremental::ReviewedFiles>,
    /// Set when only files changed since an earlier review were sent.
    pub incremental: Option<crate::incremental::Selection>,
}

impl ReviewOutput {
    /// Whether every reviewable file was sent in full in this run (not an
    /// incremental review), its coverage was confirmed and no finding was
    /// dropped. Paths excluded by configuration
    /// are a deliberate choice.
    #[must_use]
    pub const fn fully_reviewed(&self) -> bool {
        self.dropped_findings == 0
            && self.unreviewable_paths.is_empty()
            && self.truncated_paths.is_empty()
            && self.unconfirmed_paths.is_empty()
            && self.incremental.is_none()
    }

    pub fn usage_record(&self) -> crate::review_usage::UsageRecord {
        crate::review_usage::UsageRecord::new(&self.run_id, &self.head_sha, self.usage.as_ref())
    }

    /// Add the PR's accumulated usage. History failures are logged and shown
    /// as incomplete rather than blocking the review.
    pub async fn attach_pr_usage(&mut self, source_control: &dyn ScmProvider) {
        let history = match source_control.review_usage_history().await {
            Ok(history) => history,
            Err(error) => {
                tracing::warn!(%error, "could not read earlier Cururu review usage");
                crate::review_usage::UsageHistory::default()
            }
        };
        self.pr_usage = Some(crate::review_usage::pr_total(
            &history,
            &self.usage_record(),
        ));
    }
}

pub fn ensure_evaluator_mode_allowed(config: &AppConfig) -> anyhow::Result<()> {
    anyhow::ensure!(
        !matches!(
            config.evaluator.mode,
            Some(crate::evaluation::EvaluationMode::Filter)
        ),
        "evaluator filtering is disabled pending approved data, thresholds and rollout"
    );
    Ok(())
}

pub fn trusted_config_hash(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(raw.as_bytes()))
}

async fn ensure_trusted_config_unchanged(
    config: &AppConfig,
    scm: &dyn ScmProvider,
) -> anyhow::Result<()> {
    let Some((expected_base, expected_hash)) = &config.trusted_config_snapshot else {
        return Ok(());
    };
    let base = scm.fetch_base_sha().await?;
    anyhow::ensure!(
        &base == expected_base,
        "trusted repository configuration changed during review"
    );
    let current_hash = scm
        .fetch_config_toml(&base)
        .await?
        .as_deref()
        .map(trusted_config_hash);
    anyhow::ensure!(
        &current_hash == expected_hash,
        "trusted repository configuration changed during review"
    );
    if let Some(shared) = &config.shared_config_snapshot {
        let raw = shared
            .client
            .fetch_repository_file_at_ref(
                &shared.source.repository,
                &shared.source.path,
                &shared.commit,
            )
            .await
            .map_err(|_| anyhow::anyhow!("failed to revalidate trusted shared configuration"))?;
        anyhow::ensure!(
            trusted_config_hash(&raw) == shared.hash,
            "trusted shared configuration changed during review"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn run_review(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
) -> anyhow::Result<ReviewOutput> {
    run_review_with_evaluator(config, source_control, None).await
}

#[allow(clippy::too_many_lines)]
pub async fn run_review_with_evaluator(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    evaluator_override: Option<&dyn evaluation::ReviewEvaluator>,
) -> anyhow::Result<ReviewOutput> {
    ensure_evaluator_mode_allowed(config)?;
    if config.evaluator.mode.is_some() {
        anyhow::ensure!(
            config.evaluator.repository_allowed,
            "Jev evaluation is not authorized for this repository; add its identity to CURURU_EVALUATOR_ALLOWED_REPOSITORIES"
        );
    }
    ensure_trusted_config_unchanged(config, source_control).await?;
    let prepared = crate::review_input::prepare_pr(config, source_control).await?;
    crate::review_input::ensure_reviewable(&prepared)?;
    let head_sha = &prepared.revision;
    let files = &prepared.files;
    let current_patches = crate::review_input::patch_hashes(files);
    let selection = incremental_selection(config, source_control, &current_patches).await;
    let incremental_chunks;
    let chunks: &[cururu_core::DiffChunk] = match &selection {
        Some(selection) => {
            let changed: Vec<diff::ChangedFile> = files
                .iter()
                .filter(|file| selection.changed.contains(&file.path))
                .cloned()
                .collect();
            incremental_chunks = crate::review_input::build_chunks(config, &changed).0;
            info!(
                changed = selection.changed.len(),
                unchanged = selection.unchanged.len(),
                since = %selection.since_head,
                "incremental review: only files whose patch changed since the last review are sent"
            );
            &incremental_chunks
        }
        None => &prepared.chunks,
    };
    info!(
        files = files.len(),
        chunks = chunks.len(),
        "reviewing change-request diff"
    );

    let (context_store, prior_feedback, analysis_report) =
        collect_review_evidence(config, source_control, files, head_sha).await?;
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
    let system_prompt = format!(
        "{}{lang_instruction}",
        cururu_engine::DEFAULT_REVIEW_PROMPT.trim()
    );
    let agent = agent::build_review_agent(
        &config.llm,
        system_prompt,
        serde_json::json!({
            "repository_context": context_store,
            "prior_feedback": prior_feedback,
        }),
    )?;

    let checked = crate::review_input::review_chunks_checked(
        agent.as_ref(),
        &prepared,
        chunks,
        &prior_feedback,
    )
    .await?;
    let mut unconfirmed_paths: Vec<String> = Vec::new();
    for (item, chunk) in checked.iter().zip(chunks) {
        if !item.coverage_confirmed {
            for path in &chunk.files {
                if !unconfirmed_paths.contains(path) {
                    unconfirmed_paths.push(path.clone());
                }
            }
        }
    }
    let dropped_findings = checked.iter().map(|item| item.dropped_findings).sum();
    let mut chunk_results: Vec<cururu_engine::ChunkResult> =
        checked.into_iter().map(|item| item.result).collect();
    ensure_trusted_config_unchanged(config, source_control).await?;
    let current = crate::review_input::prepare_pr(config, source_control).await?;
    anyhow::ensure!(
        current.snapshot_sha256 == prepared.snapshot_sha256,
        "review snapshot changed during model call"
    );
    let (current_context, current_feedback, current_analysis) =
        collect_review_evidence(config, source_control, &current.files, &current.revision).await?;
    ensure_review_head_unchanged(head_sha, &source_control.fetch_head_sha().await?)?;
    // The reviewed code and its evidence must be what the model saw. Prior
    // feedback may change meanwhile (a person replies or resolves a thread):
    // that does not invalidate the paid review, but findings on threads
    // resolved meanwhile are dropped.
    anyhow::ensure!(
        serde_json::to_vec(&(&context_store, &analysis_report))?
            == serde_json::to_vec(&(&current_context, &current_analysis))?,
        "review context or analyzer evidence changed during model call"
    );
    let mut dropped_newly_resolved = 0;
    if serde_json::to_vec(&prior_feedback)? != serde_json::to_vec(&current_feedback)? {
        for result in &mut chunk_results {
            dropped_newly_resolved += crate::prior_feedback::drop_on_newly_resolved(
                &prior_feedback,
                &current_feedback,
                &mut result.review.findings,
            );
        }
        warn!(
            dropped = dropped_newly_resolved,
            "prior review feedback changed during the model call; publishing the review without findings on threads resolved meanwhile"
        );
    }
    for (chunk, result) in chunks.iter().zip(&mut chunk_results) {
        crate::review_input::validate_chunk_findings(&prepared, chunk, &result.review)?;
        for finding in &mut result.review.findings {
            finding.source = None;
            finding.rule = None;
        }
    }

    let model = config.llm.model.clone();
    let usage = provider::merge_usage(&chunk_results);
    let candidates = agent::collect_candidates(
        model.clone(),
        files.len(),
        chunk_results,
        &config.review.policy,
        analysis_report.findings.clone(),
    );
    let (mut review, mut evaluation_report) = if let Some(mode) = config.evaluator.mode {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_mins(2))
            .build()
            .context("failed to build TypeSafe evaluator client")?;
        let evaluator = evaluation::build_jev_evaluator(
            client,
            config.evaluator.api_key.clone(),
            config.evaluator.model.clone(),
        );
        let evaluator = evaluator_override.unwrap_or_else(|| evaluator.as_ref());
        let mut report = evaluator.evaluate(&candidates.findings, files).await?;
        let mut judged_candidates = candidates;
        let unevaluated = judged_candidates.findings.split_off(report.findings.len());
        let (judged_findings, judged_report) = evaluation::apply_judgments(
            judged_candidates.findings,
            report
                .findings
                .iter()
                .map(|item| item.judgment.clone())
                .collect(),
            mode,
        )?;
        report.findings = judged_report.findings;
        judged_candidates.findings = judged_findings;
        judged_candidates.findings.extend(unevaluated);
        (
            agent::apply_policy(judged_candidates, &config.review.policy),
            Some(report),
        )
    } else {
        (agent::apply_policy(candidates, &config.review.policy), None)
    };
    if config.evaluator.mode.is_some() {
        ensure_trusted_config_unchanged(config, source_control).await?;
        let current = crate::review_input::prepare_pr(config, source_control).await?;
        anyhow::ensure!(
            current.snapshot_sha256 == prepared.snapshot_sha256,
            "review snapshot changed during evaluation"
        );
        let (context, feedback, analysis) =
            collect_review_evidence(config, source_control, &current.files, &current.revision)
                .await?;
        ensure_review_head_unchanged(head_sha, &source_control.fetch_head_sha().await?)?;
        anyhow::ensure!(
            serde_json::to_vec(&(&context_store, &analysis_report))?
                == serde_json::to_vec(&(&context, &analysis))?,
            "review context or analyzer evidence changed during evaluation"
        );
        let dropped = crate::prior_feedback::drop_on_newly_resolved(
            &current_feedback,
            &feedback,
            &mut review.findings,
        );
        if dropped > 0 {
            warn!(
                dropped,
                "prior review feedback changed during evaluation; findings on threads resolved meanwhile were dropped"
            );
        }
    }
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

    let reviewed_files = config.review.policy.incremental.changed_files().then(|| {
        let mut not_covered = prepared.truncated_paths.clone();
        not_covered.extend(unconfirmed_paths.iter().cloned());
        crate::incremental::next_state(head_sha, &current_patches, &not_covered)
    });
    Ok(ReviewOutput {
        review,
        usage,
        context_files: context_paths,
        model,
        show_usage: config.summary.show_usage,
        show_cost: config.summary.show_cost,
        logo_url: config.summary.logo_url.clone(),
        changed_files: files.clone(),
        run_id: crate::review_usage::new_run_id(head_sha),
        head_sha: head_sha.clone(),
        analysis: analysis_report,
        evaluation: evaluation_report,
        pr_usage: None,
        ignored_paths: prepared.ignored_paths.clone(),
        unreviewable_paths: prepared.unreviewable_paths.clone(),
        truncated_paths: prepared.truncated_paths.clone(),
        unconfirmed_paths,
        dropped_findings,
        auto_approval: None,
        language: config.review.language.clone(),
        reviewed_files,
        incremental: selection,
    })
}

/// The files to review again under `policy.incremental = "changed_files"`,
/// or None for a complete review: the mode is off, auto-approval or
/// `fail_on` needs the whole diff reviewed, there is no recorded state, it
/// cannot be read, or every file changed.
async fn incremental_selection(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    current: &std::collections::BTreeMap<String, String>,
) -> Option<crate::incremental::Selection> {
    let policy = &config.review.policy;
    if !policy.incremental.changed_files() {
        return None;
    }
    if config.auto_approval.enabled || policy.fail_on != cururu_core::FailOn::Off {
        info!(
            "incremental review disabled for this run: auto-approval and fail_on need the whole diff reviewed"
        );
        return None;
    }
    let previous = match source_control.latest_reviewed_files().await {
        Ok(Some(previous)) => previous,
        Ok(None) => return None,
        Err(error) => {
            warn!(%error, "could not read the last review's files; reviewing the whole diff");
            return None;
        }
    };
    let selection = crate::incremental::select(&previous, current);
    (!selection.unchanged.is_empty()).then_some(selection)
}

async fn collect_review_evidence(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    files: &[diff::ChangedFile],
    head_sha: &str,
) -> anyhow::Result<(
    ContextStore,
    Vec<cururu_core::PriorReviewFeedback>,
    analysis::AnalysisReport,
)> {
    let mut context = fetch_repo_context(config, source_control).await?;
    if config.context.auto.enabled {
        append_auto_context(config, source_control, &mut context, files).await?;
    }
    let feedback = source_control
        .fetch_prior_review_feedback(&crate::prior_feedback::FeedbackOptions::for_review(
            &config.review,
            files,
        ))
        .await?;
    let analysis =
        analysis::load_evidence(&config.analysis, files, head_sha, source_control).await?;
    Ok((context, feedback, analysis))
}

pub async fn fetch_repo_context(
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

pub async fn append_auto_context(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    store: &mut ContextStore,
    files: &[diff::ChangedFile],
) -> anyhow::Result<()> {
    let base_sha = source_control.fetch_base_sha().await?;
    let auto = &config.context.auto;
    let selector = cururu_engine::auto_context::CandidateSelector::new(auto)?;
    let base_paths: std::collections::HashSet<String> = source_control
        .list_repository_paths_at_revision(&base_sha)
        .await
        .context("failed to list base revision for automatic context")?
        .into_iter()
        .collect();
    let mut total = store
        .files
        .iter()
        .map(|file| file.content.len())
        .sum::<usize>();

    for changed in files {
        if !selector.accepts(
            &changed.path,
            store.files.iter().map(|file| file.path.as_str()),
            store.files.len(),
            total,
        ) {
            continue;
        }

        if !base_paths.contains(&changed.path) {
            store.skipped.push(changed.path.clone());
            continue;
        }
        let content = source_control
            .fetch_file_at_ref(&changed.path, &base_sha)
            .await
            .map_err(|_| {
                anyhow::anyhow!("failed to fetch automatic base context file from the SCM provider")
            })?;
        let remaining = auto.max_bytes.saturating_sub(total);
        let limit = remaining.min(auto.per_file_bytes);
        if content.len() > limit {
            store.truncated.push(changed.path.clone());
        }
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
    use super::ensure_review_head_unchanged;
    use crate::scm::ScmProvider;
    use std::sync::Mutex;

    struct FixtureReviewScm;

    struct EvaluationScm(std::sync::Arc<std::sync::atomic::AtomicBool>);

    #[async_trait::async_trait]
    impl ScmProvider for EvaluationScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_base_sha(&self) -> anyhow::Result<String> {
            Ok("base".into())
        }
        async fn fetch_config_toml(&self, _: &str) -> anyhow::Result<Option<String>> {
            Ok(Some(
                if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                    "changed"
                } else {
                    "original"
                }
                .into(),
            ))
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
    }

    struct SyntheticEvaluator {
        changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        mutate: bool,
    }

    #[async_trait::async_trait]
    impl crate::evaluation::ReviewEvaluator for SyntheticEvaluator {
        async fn evaluate(
            &self,
            findings: &[crate::agent::ReviewFinding],
            _: &[crate::diff::ChangedFile],
        ) -> anyhow::Result<crate::evaluation::EvaluationReport> {
            assert!(findings.is_empty());
            self.changed
                .store(self.mutate, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::evaluation::EvaluationReport {
                model: "synthetic".into(),
                findings: vec![],
                omitted_findings: 0,
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn observe_rejects_configuration_changed_during_evaluation() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let review =
            serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]})
                .to_string();
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]})))
            .expect(2).mount(&server).await;
        for mutate in [false, true] {
            let changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let scm = EvaluationScm(changed.clone());
            let evaluator = SyntheticEvaluator { changed, mutate };
            let mut config = crate::config::AppConfig::from_local_env().unwrap();
            config.llm.provider = crate::config::LlmProvider::OpenAI;
            config.llm.base_url = server.uri();
            config.llm.api_key = "fixture".into();
            config.evaluator.mode = Some(crate::evaluation::EvaluationMode::Observe);
            config.evaluator.repository_allowed = true;
            config.trusted_config_snapshot =
                Some(("base".into(), Some(super::trusted_config_hash("original"))));
            let result = super::run_review_with_evaluator(&config, &scm, Some(&evaluator)).await;
            if mutate {
                assert!(
                    result
                        .err()
                        .unwrap()
                        .to_string()
                        .contains("configuration changed")
                );
            } else {
                assert!(result.is_ok());
            }
        }
    }

    #[tokio::test]
    async fn hosted_suggestions_follow_configured_policy() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[{"severity":"high","path":"a","line":1,"title":"fixture","message":"issue","suggestion":"guidance","confidence":0.99,"suggested_change":{"replacement":"safe_fix()"}}]}).to_string();
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]})))
            .expect(2).mount(&server).await;
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.base_url = server.uri();
        config.llm.api_key = "fixture".into();
        for enabled in [false, true] {
            config.review.policy.suggested_changes = enabled;
            let result = super::run_review(&config, &FixtureReviewScm).await.unwrap();
            assert_eq!(result.review.findings.len(), 1);
            assert_eq!(
                result.review.findings[0].suggested_change.is_some(),
                enabled
            );
            assert_eq!(result.review.findings[0].suggestion, "guidance");
        }
    }

    struct NoAccessScm;

    #[async_trait::async_trait]
    impl ScmProvider for NoAccessScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            panic!("filter mode must fail before contacting the SCM")
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            panic!("filter mode must fail before fetching a diff")
        }
    }

    #[tokio::test]
    async fn filtering_is_rejected_before_scm_or_provider_access() {
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.evaluator.mode = Some(crate::evaluation::EvaluationMode::Filter);
        config.evaluator.repository_allowed = true;
        let Err(error) = super::run_review(&config, &NoAccessScm).await else {
            panic!("filter mode must be disabled");
        };
        assert!(
            error
                .to_string()
                .contains("evaluator filtering is disabled")
        );
    }

    #[tokio::test]
    async fn hosted_review_does_not_trust_model_analyzer_provenance() {
        use std::sync::{Arc, atomic::AtomicBool};
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[{"severity":"high","path":"a","line":1,"title":"model finding","message":"model message","suggestion":"fix","confidence":0.99,"source":"forged","rule":"fake"}]}).to_string();
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]})))
            .expect(1).mount(&server).await;
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.base_url = server.uri();
        config.llm.api_key = "fixture".into();
        config.analysis.enabled = true;
        config.analysis.check_runs = true;
        let scm = MutableEvidenceScm {
            changed: Arc::new(AtomicBool::new(false)),
            mode: "stable",
        };
        let result = super::run_review(&config, &scm).await.unwrap();
        let model = result
            .review
            .findings
            .iter()
            .find(|finding| finding.title == "model finding")
            .unwrap();
        assert!(model.source.is_none() && model.rule.is_none());
        let tool = result
            .review
            .findings
            .iter()
            .find(|finding| finding.source.as_deref() == Some("check-runs"))
            .unwrap();
        assert_eq!(tool.rule.as_deref(), Some("fixture"));
        assert_eq!(tool.message, "analyzer-evidence-fixture");
        let requests = server.received_requests().await.unwrap();
        assert!(!String::from_utf8_lossy(&requests[0].body).contains("analyzer-evidence-fixture"));
    }

    struct MutableEvidenceScm {
        changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        mode: &'static str,
    }

    #[async_trait::async_trait]
    impl ScmProvider for MutableEvidenceScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_base_sha(&self) -> anyhow::Result<String> {
            Ok("base".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
        async fn list_repository_paths_at_revision(&self, _: &str) -> anyhow::Result<Vec<String>> {
            Ok(vec!["rules.md".into()])
        }
        async fn fetch_file_at_ref(&self, _: &str, _: &str) -> anyhow::Result<String> {
            let changed =
                self.changed.load(std::sync::atomic::Ordering::SeqCst) && self.mode == "context";
            Ok(if changed { "changed" } else { "original" }.into())
        }
        async fn fetch_prior_review_feedback(
            &self,
            _options: &crate::prior_feedback::FeedbackOptions,
        ) -> anyhow::Result<Vec<cururu_core::PriorReviewFeedback>> {
            // In mode "feedback", a person resolves the thread on a:1 while
            // the model call runs.
            let resolved =
                self.changed.load(std::sync::atomic::Ordering::SeqCst) && self.mode == "feedback";
            Ok(vec![cururu_core::PriorReviewFeedback {
                location: "a (line 1)".into(),
                comments: vec![cururu_core::PriorReviewComment {
                    author: "fixture".into(),
                    body: "untrusted-feedback-fixture".into(),
                }],
                resolved,
                summarized: false,
                decided: resolved.then(|| cururu_core::DecidedFinding {
                    path: "a".into(),
                    line: Some(1),
                    anchor: None,
                    title: "Earlier finding".into(),
                    code_changed: false,
                }),
            }])
        }
        async fn list_finding_annotations(
            &self,
            _: &str,
            _: &[String],
        ) -> anyhow::Result<Vec<cururu_core::FindingAnnotation>> {
            let changed =
                self.changed.load(std::sync::atomic::Ordering::SeqCst) && self.mode == "evidence";
            Ok(vec![cururu_core::FindingAnnotation {
                path: "a".into(),
                line: Some(1),
                severity: "warning".into(),
                title: Some("fixture".into()),
                message: if changed {
                    "changed"
                } else {
                    "analyzer-evidence-fixture"
                }
                .into(),
                details: None,
            }])
        }
    }

    #[tokio::test]
    async fn hosted_review_sends_context_and_feedback_only_as_untrusted_user_data() {
        use std::sync::{Arc, atomic::AtomicBool};
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for anthropic in [false, true] {
            let server = MockServer::start().await;
            let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]}).to_string();
            let response = if anthropic {
                serde_json::json!({"stop_reason":"end_turn","content":[{"type":"text","text":review}]})
            } else {
                serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]})
            };
            Mock::given(method("POST"))
                .and(path(if anthropic {
                    "/messages"
                } else {
                    "/chat/completions"
                }))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            let mut config = crate::config::AppConfig::from_local_env().unwrap();
            config.llm.provider = if anthropic {
                crate::config::LlmProvider::Anthropic
            } else {
                crate::config::LlmProvider::OpenAI
            };
            config.llm.base_url = server.uri();
            config.llm.api_key = "fixture".into();
            config.context.additional = vec!["rules.md".into()];
            let scm = MutableEvidenceScm {
                changed: Arc::new(AtomicBool::new(false)),
                mode: "stable",
            };
            assert!(super::run_review(&config, &scm).await.is_ok());
            let requests = server.received_requests().await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
            let system = if anthropic {
                body["system"].as_str().unwrap()
            } else {
                body["messages"][0]["content"].as_str().unwrap()
            };
            assert!(!system.contains("untrusted-feedback-fixture"));
            assert!(!system.contains("rules.md"));
            let user_messages = body["messages"].to_string();
            assert!(user_messages.contains("untrusted-feedback-fixture"));
            assert!(user_messages.contains("rules.md"));
            assert!(user_messages.contains("untrusted"));
        }
    }

    #[tokio::test]
    async fn hosted_review_rejects_context_or_analysis_changed_during_model_call() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for mode in ["stable", "context", "evidence"] {
            let server = MockServer::start().await;
            let changed = Arc::new(AtomicBool::new(false));
            let update = Arc::clone(&changed);
            Mock::given(method("POST")).and(path("/chat/completions"))
                .respond_with(move |_: &wiremock::Request| {
                    update.store(true, Ordering::SeqCst);
                    let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]}).to_string();
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]}))
                }).expect(1).mount(&server).await;
            let mut config = crate::config::AppConfig::from_local_env().unwrap();
            config.llm.provider = crate::config::LlmProvider::OpenAI;
            config.llm.base_url = server.uri();
            config.llm.api_key = "fixture".into();
            config.context.additional = vec!["rules.md".into()];
            config.analysis.enabled = true;
            config.analysis.check_runs = true;
            let result = super::run_review(&config, &MutableEvidenceScm { changed, mode }).await;
            if mode == "stable" {
                assert!(result.is_ok());
            } else {
                let Err(error) = result else {
                    panic!("changed evidence was accepted")
                };
                assert!(
                    error
                        .to_string()
                        .contains("context or analyzer evidence changed")
                );
            }
        }
    }

    #[tokio::test]
    async fn hosted_review_publishes_when_only_feedback_changed_during_model_call() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let changed = Arc::new(AtomicBool::new(false));
        let update = Arc::clone(&changed);
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                update.store(true, Ordering::SeqCst);
                let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[
                    {"severity":"high","path":"a","line":1,"title":"Repeated on the resolved thread","message":"m","suggestion":"s","confidence":0.9}
                ]}).to_string();
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]}))
            }).expect(1).mount(&server).await;
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.base_url = server.uri();
        config.llm.api_key = "fixture".into();
        let result = super::run_review(
            &config,
            &MutableEvidenceScm {
                changed,
                mode: "feedback",
            },
        )
        .await
        .expect("a feedback change alone must not discard the paid review");
        // The finding falls on the thread resolved during the call.
        assert!(
            result.review.findings.is_empty(),
            "{:?}",
            result.review.findings
        );
    }

    struct MultiChunkScm;

    #[async_trait::async_trait]
    impl ScmProvider for MultiChunkScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            use std::fmt::Write as _;
            let mut diff = String::new();
            for name in ["a", "b"] {
                write!(
                    diff,
                    "diff --git a/{name} b/{name}\n@@ -1 +1 @@\n-{}\n+{}\n",
                    "old".repeat(20),
                    "new".repeat(20)
                )?;
            }
            Ok((diff, false))
        }
    }

    /// Files `a` and `b` changed; the last review recorded `recorded`.
    struct IncrementalScm {
        recorded: Option<crate::incremental::ReviewedFiles>,
    }

    #[async_trait::async_trait]
    impl ScmProvider for IncrementalScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            MultiChunkScm.fetch_head_sha().await
        }
        async fn fetch_diff_with_limit(&self, limit: usize) -> anyhow::Result<(String, bool)> {
            MultiChunkScm.fetch_diff_with_limit(limit).await
        }
        async fn latest_reviewed_files(
            &self,
        ) -> anyhow::Result<Option<crate::incremental::ReviewedFiles>> {
            Ok(self.recorded.clone())
        }
    }

    #[tokio::test]
    async fn changed_files_mode_sends_only_files_whose_patch_changed() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(|_: &wiremock::Request| {
                let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]}).to_string();
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]}))
            }).mount(&server).await;
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.base_url = server.uri();
        config.llm.api_key = "fixture".into();
        config.review.policy.incremental = cururu_engine::review_policy::Incremental::ChangedFiles;
        let prepared = crate::review_input::prepare_pr(&config, &MultiChunkScm)
            .await
            .unwrap();
        let current = crate::review_input::patch_hashes(&prepared.files);
        // The last review saw `a` with this patch and `b` with another one.
        let mut recorded = crate::incremental::next_state("old-head", &current, &[]);
        recorded.patches.insert("b".into(), "0".repeat(32));

        let output = super::run_review(
            &config,
            &IncrementalScm {
                recorded: Some(recorded.clone()),
            },
        )
        .await
        .unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(body.contains("diff --git a/b b/b") && !body.contains("diff --git a/a b/a"));
        let selection = output.incremental.as_ref().unwrap();
        assert_eq!(
            (selection.changed.as_slice(), selection.unchanged.as_slice()),
            (&["b".to_owned()][..], &["a".to_owned()][..])
        );
        assert!(!output.fully_reviewed());
        // The next state covers both files at their current patches.
        let state = output.reviewed_files.unwrap();
        assert_eq!(state, crate::incremental::next_state("head", &current, &[]));

        // Nothing changed since the recorded review (a new head, the same
        // PR patches): no model call at all.
        let output = super::run_review(
            &config,
            &IncrementalScm {
                recorded: Some(crate::incremental::next_state("old-head", &current, &[])),
            },
        )
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert!(output.review.findings.is_empty());
        assert_eq!(output.incremental.unwrap().changed, Vec::<String>::new());

        // fail_on needs the whole diff: both files are sent.
        config.review.policy.fail_on = cururu_core::FailOn::High;
        let output = super::run_review(
            &config,
            &IncrementalScm {
                recorded: Some(recorded),
            },
        )
        .await
        .unwrap();
        assert!(output.incremental.is_none());
        let requests = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&requests[1].body);
        assert!(body.contains("diff --git a/a b/a") && body.contains("diff --git a/b b/b"));
    }

    #[tokio::test]
    async fn hosted_multichunk_review_rejects_second_chunk_failure() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for fail_second in [false, true] {
            let server = MockServer::start().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&calls);
            Mock::given(method("POST")).and(path("/chat/completions"))
                .respond_with(move |_: &wiremock::Request| {
                    let number = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]}).to_string();
                    let usage = if number == 1 { Some(serde_json::json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12})) } else { None };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":if fail_second && number >= 2 { "length" } else { "stop" }}], "usage":usage}))
                // A cut-off chunk is requested once more with a larger limit.
                }).expect(if fail_second { 3 } else { 2 }).mount(&server).await;
            let mut config = crate::config::AppConfig::from_local_env().unwrap();
            config.llm.provider = crate::config::LlmProvider::OpenAI;
            config.llm.base_url = server.uri();
            config.llm.api_key = "fixture".into();
            config.review.chunk_bytes = 250;
            let result = super::run_review(&config, &MultiChunkScm).await;
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if fail_second { 3 } else { 2 }
            );
            if fail_second {
                let Err(error) = result else {
                    panic!("partial review was accepted")
                };
                assert!(
                    error
                        .downcast_ref::<crate::agent::InvalidReviewOutput>()
                        .is_some()
                );
            } else {
                let output = result.unwrap();
                assert_eq!(output.review.files_reviewed, 2);
                assert!(
                    output.usage.is_none(),
                    "missing usage must not yield a partial total"
                );
            }
        }
    }

    struct ChangingHeadScm(std::sync::atomic::AtomicUsize);

    #[async_trait::async_trait]
    impl ScmProvider for ChangingHeadScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            let call = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(if call < 2 { "head" } else { "changed" }.into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
    }

    #[tokio::test]
    async fn hosted_review_rejects_head_changed_after_model_response() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let review =
            serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]})
                .to_string();
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]})))
            .expect(1).mount(&server).await;
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.base_url = server.uri();
        config.llm.api_key = "fixture".into();
        let scm = ChangingHeadScm(std::sync::atomic::AtomicUsize::new(0));
        assert!(super::run_review(&config, &scm).await.is_err());
        assert_eq!(scm.0.load(std::sync::atomic::Ordering::SeqCst), 4);
    }

    #[async_trait::async_trait]
    impl ScmProvider for FixtureReviewScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
                false,
            ))
        }
    }

    #[tokio::test]
    async fn hosted_review_never_publishes_unanchored_model_findings() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for (line, anchored) in [(1, true), (99, false)] {
            let server = MockServer::start().await;
            let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[{"severity":"high","path":"a","line":line,"title":"fixture","message":"fixture","suggestion":"fix","confidence":0.9}]}).to_string();
            Mock::given(method("POST")).and(path("/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]})))
                // An unanchored finding is requested once more, then dropped.
                .expect(if anchored { 1 } else { 2 }).mount(&server).await;
            let mut config = crate::config::AppConfig::from_local_env().unwrap();
            config.llm.provider = crate::config::LlmProvider::OpenAI;
            config.llm.base_url = server.uri();
            config.llm.api_key = "fixture".into();
            let output = super::run_review(&config, &FixtureReviewScm).await.unwrap();
            assert_eq!(output.review.findings.len(), usize::from(anchored));
            assert_eq!(output.dropped_findings, usize::from(!anchored));
        }
    }

    #[tokio::test]
    async fn inconsistent_coverage_is_retried_then_kept_as_unconfirmed() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let reply = |files_reviewed: usize| {
            let review = serde_json::json!({"model":"fixture","files_reviewed":files_reviewed,"summary":"ok","findings":[{"severity":"high","path":"a","line":1,"title":"fixture","message":"fixture","suggestion":"fix","confidence":0.9}]}).to_string();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]}))
        };
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.api_key = "fixture".into();

        // Wrong count, then correct: the retry confirms coverage.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(reply(0))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(reply(1))
            .mount(&server)
            .await;
        config.llm.base_url = server.uri();
        let output = super::run_review(&config, &FixtureReviewScm).await.unwrap();
        assert_eq!(output.unconfirmed_paths, Vec::<String>::new());
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        // Wrong twice: the findings are kept and the coverage is unconfirmed.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(reply(3))
            .expect(2)
            .mount(&server)
            .await;
        config.llm.base_url = server.uri();
        let output = super::run_review(&config, &FixtureReviewScm).await.unwrap();
        assert_eq!(output.unconfirmed_paths, ["a"]);
        assert_eq!(output.review.findings.len(), 1);
    }

    #[tokio::test]
    async fn cut_off_review_is_retried_once_with_a_larger_output_limit() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let review =
            serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]})
                .to_string();
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.api_key = "fixture".into();
        config.llm.max_output_tokens = 8000;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"choices":[{"message":{"content":"{\"model\":"},"finish_reason":"length"}]}),
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]}),
            ))
            .mount(&server)
            .await;
        config.llm.base_url = server.uri();
        super::run_review(&config, &FixtureReviewScm).await.unwrap();
        let limits: Vec<u64> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| {
                request.body_json::<serde_json::Value>().unwrap()["max_tokens"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(limits, [8000, 16000]);
    }

    #[tokio::test]
    async fn findings_outside_the_diff_are_dropped_after_a_retry_and_reported() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let reply = |severity: &str, line: u32| {
            let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[
                {"severity":severity,"path":"a","line":line,"title":"fixture","message":"fixture","suggestion":"fix","confidence":0.9},
                {"severity":"high","path":"a","line":1,"title":"kept","message":"fixture","suggestion":"fix","confidence":0.9}]}).to_string();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]}))
        };
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.llm.provider = crate::config::LlmProvider::OpenAI;
        config.llm.api_key = "fixture".into();

        // A line outside the diff twice: only that finding is dropped.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(reply("high", 999))
            .expect(2)
            .mount(&server)
            .await;
        config.llm.base_url = server.uri();
        let output = super::run_review(&config, &FixtureReviewScm).await.unwrap();
        assert_eq!(output.dropped_findings, 1);
        assert_eq!(output.review.findings.len(), 1);
        assert_eq!(output.review.findings[0].title, "kept");
        assert!(!output.fully_reviewed());

        // Any other invalid finding still fails the review.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(reply("<LEVEL>", 1))
            .expect(2)
            .mount(&server)
            .await;
        config.llm.base_url = server.uri();
        assert!(super::run_review(&config, &FixtureReviewScm).await.is_err());
    }

    struct UnreviewableScm(&'static str);

    #[async_trait::async_trait]
    impl ScmProvider for UnreviewableScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((self.0.into(), false))
        }
        async fn fetch_prior_review_feedback(
            &self,
            _options: &crate::prior_feedback::FeedbackOptions,
        ) -> anyhow::Result<Vec<crate::scm::PriorReviewFeedback>> {
            panic!("unreviewable input must fail before further collection")
        }
    }

    #[tokio::test]
    async fn hosted_review_rejects_empty_ignored_binary_and_submodule_before_calls() {
        let config = crate::config::AppConfig::from_local_env().unwrap();
        for (raw, expected) in [
            ("", "no changes available"),
            (
                "diff --git a/src/Cargo.lock b/src/Cargo.lock\n@@ -1 +1 @@\n-old\n+new\n",
                "no changes available",
            ),
            (
                "diff --git a/a.bin b/a.bin\nBinary files a/a.bin and b/a.bin differ\n",
                "binary files",
            ),
            (
                "diff --git a/sub b/sub\n@@ -1 +1 @@\n-Subproject commit old\n+Subproject commit new\n",
                "submodule",
            ),
            (
                "diff --git a/sub b/sub\ndeleted file mode 160000\n@@ -1 +0,0 @@\n-Subproject commit old\n",
                "submodule",
            ),
        ] {
            let Err(error) = super::run_review(&config, &UnreviewableScm(raw)).await else {
                panic!("unreviewable input was accepted");
            };
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    struct NoticeScm {
        head: &'static str,
        fail: bool,
        notices: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl ScmProvider for NoticeScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok(self.head.into())
        }

        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            Ok((String::new(), true))
        }

        async fn upsert_limit_notice(&self, body: &str) -> anyhow::Result<()> {
            anyhow::ensure!(!self.fail, "notice publication failed");
            self.notices.lock().unwrap().push(body.into());
            Ok(())
        }
    }

    #[tokio::test]
    async fn diff_limit_notice_is_localized_bound_to_head_and_fails_closed() {
        let mut config = crate::config::AppConfig::from_local_env().unwrap();
        config.review.language = "pt-BR".into();
        let scm = NoticeScm {
            head: "abc",
            fail: false,
            notices: Mutex::new(vec![]),
        };
        let error = anyhow::Error::new(super::DiffDownloadLimit {
            head_sha: "abc".into(),
        });
        assert!(
            super::publish_diff_limit_notice(&config, &scm, &error)
                .await
                .unwrap()
        );
        let body = scm.notices.lock().unwrap()[0].clone();
        assert!(body.contains("revisão não realizada"));
        assert!(body.contains("não é uma aprovação"));
        assert!(body.contains("`abc`"));

        let other = anyhow::anyhow!("network failure");
        assert!(
            !super::publish_diff_limit_notice(&config, &scm, &other)
                .await
                .unwrap()
        );
        let stale = anyhow::Error::new(super::DiffTotalLimit {
            head_sha: "old".into(),
        });
        assert!(
            super::publish_diff_limit_notice(&config, &scm, &stale)
                .await
                .is_err()
        );
        let failing = NoticeScm {
            head: "abc",
            fail: true,
            notices: Mutex::new(vec![]),
        };
        assert!(
            super::publish_diff_limit_notice(&config, &failing, &error)
                .await
                .is_err()
        );
        let english = crate::output::render_diff_limit_notice("en-US", 42, "abc");
        assert!(english.contains("not an approval"));
        assert!(english.contains("42 bytes"));
    }

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
        assert!(super::ensure_review_diff_limits(&files, 10, 100).is_ok());
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
}
