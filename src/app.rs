mod db;
mod github_app;

use crate::hosted_publication::{self, HostedCandidate};
use crate::scm::CandidateCheck;
use crate::{
    agent, build_inline_drafts,
    config::{AppConfig, CommentMode},
    github::GitHubClient,
    repository::{RepositoryIdentity, github_host_urls},
    review,
    scm::ScmProvider,
};
use anyhow::{Context, bail};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use db::{DeliveryStore, QueuedDelivery, StoredCandidate};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::{sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tracing::{info, warn};

type HmacSha256 = Hmac<Sha256>;
pub const GITHUB_WEBHOOK_PATH: &str = "/v1/webhooks/github";
const MAX_MENTION_DIFF_BYTES: usize = 80_000;
const MAX_WEBHOOK_BODY_BYTES: usize = 25_000_000;
const MENTION_RATE_WINDOW_SECS: i64 = 3_600;
const MAX_MENTIONS_PER_USER_PER_PR: i64 = 5;

#[derive(Clone)]
struct AppState {
    store: DeliveryStore,
    auth: github_app::GitHubAppAuth,
    webhook_secret: Arc<Vec<u8>>,
    app_slug: Arc<String>,
}

struct MentionRequest<'a> {
    body: &'a str,
    pr_number: u64,
    reply_to: Option<u64>,
    mention_login: Option<&'a str>,
    actor_login: &'a str,
}

pub async fn serve() -> anyhow::Result<()> {
    let app_id = required_env("GITHUB_APP_ID")?;
    let app_private_key = match std::env::var("GITHUB_APP_PRIVATE_KEY_PATH") {
        Ok(path) => std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read GitHub App key file at {path}"))?,
        Err(_) => required_env("GITHUB_APP_PRIVATE_KEY")?,
    };
    let app_slug = required_env("GITHUB_APP_SLUG")?;
    let webhook_secret = required_env("GITHUB_WEBHOOK_SECRET")?;
    let database_url = required_env("CURURU_DATABASE_URL")?;
    let api_url =
        std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".to_string());
    let store = DeliveryStore::connect(&database_url).await?;
    let auth = github_app::GitHubAppAuth::new(&app_id, &app_private_key, &api_url)?;
    let state = AppState {
        store,
        auth,
        webhook_secret: Arc::new(webhook_secret.into_bytes()),
        app_slug: Arc::new(app_slug.trim().to_ascii_lowercase()),
    };

    let worker_state = state.clone();
    tokio::spawn(async move { worker_loop(worker_state).await });

    let app = Router::new()
        .route("/health", get(health))
        .route(GITHUB_WEBHOOK_PATH, post(receive_webhook))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
        .with_state(state);
    let host = std::env::var("CURURU_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = std::env::var("PORT")
        .ok()
        .map(|value| value.parse::<u16>())
        .transpose()
        .context("PORT must be a valid TCP port")?
        .unwrap_or(8080);
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("failed to bind Cururu App to {host}:{port}"))?;
    info!(%host, port, "Cururu GitHub App webhook server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("Cururu webhook server stopped unexpectedly")
}

pub async fn backup_sqlite(destination: &std::path::Path) -> anyhow::Result<()> {
    let database_url = required_env("CURURU_DATABASE_URL")?;
    let store = DeliveryStore::connect(&database_url).await?;
    store.backup_sqlite(&destination.to_string_lossy()).await?;
    println!("SQLite backup created at {}", destination.display());
    Ok(())
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn receive_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if !verify_signature(&state.webhook_secret, &headers, &body) {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(delivery_id) = header_value(&headers, "x-github-delivery") else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(event_type) = header_value(&headers, "x-github-event") else {
        return StatusCode::BAD_REQUEST;
    };
    if delivery_id.len() > 128 || event_type.len() > 128 {
        return StatusCode::BAD_REQUEST;
    }
    if serde_json::from_slice::<Value>(&body).is_err() {
        return StatusCode::BAD_REQUEST;
    }
    let Ok(payload) = std::str::from_utf8(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    match state
        .store
        .enqueue(&delivery_id, &event_type, payload)
        .await
    {
        Ok(_) => StatusCode::ACCEPTED,
        Err(error) => {
            warn!(%error, "failed to enqueue signed GitHub webhook");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_owned)
}

fn verify_signature(secret: &[u8], headers: &HeaderMap, body: &[u8]) -> bool {
    let Some(signature) = header_value(headers, "x-hub-signature-256") else {
        return false;
    };
    let Some(hex_signature) = signature.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(signature) = hex::decode(hex_signature) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

async fn worker_loop(state: AppState) {
    let mut last_prune = tokio::time::Instant::now();
    loop {
        if last_prune.elapsed() >= Duration::from_hours(1) {
            if let Err(error) = state.store.prune_old().await {
                warn!(%error, "failed to prune expired webhook deliveries");
            }
            last_prune = tokio::time::Instant::now();
        }
        match state.store.claim_next().await {
            Ok(Some(delivery)) => {
                let result = process_delivery(&state, &delivery).await;
                match result {
                    Ok(()) => {
                        if let Err(error) = state.store.finish(&delivery.id).await {
                            warn!(delivery_id = %delivery.id, %error, "failed to mark webhook complete");
                        }
                    }
                    Err(error) => {
                        warn!(delivery_id = %delivery.id, event = %delivery.event, %error, "webhook processing failed");
                        if let Err(retry_error) = state.store.retry(&delivery).await {
                            warn!(delivery_id = %delivery.id, %retry_error, "failed to schedule webhook retry");
                        }
                    }
                }
            }
            Ok(None) => tokio::time::sleep(Duration::from_millis(500)).await,
            Err(error) => {
                warn!(%error, "webhook queue polling failed");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn process_delivery(state: &AppState, delivery: &QueuedDelivery) -> anyhow::Result<()> {
    let payload: Value =
        serde_json::from_str(&delivery.payload).context("invalid queued webhook JSON")?;
    match delivery.event.as_str() {
        "pull_request" => {
            let action = string_at(&payload, &["action"]).unwrap_or_default();
            if !should_review_pull_request(
                &action,
                payload["pull_request"]["draft"].as_bool() == Some(true),
            ) {
                return Ok(());
            }
            review::ensure_evaluator_mode_allowed(&AppConfig::from_local_env()?)?;
            let (config, github) = app_config_for_event(state, &payload, None, None)
                .await?
                .context("PR webhook configuration was not loaded")?;
            run_pull_request_review(
                &state.store,
                &state.app_slug,
                &config,
                &github,
                action == "opened",
                false,
            )
            .await
        }
        "issue_comment" => process_issue_comment(state, &payload).await,
        "check_run" => process_check_run(state, &payload).await,
        "pull_request_review_comment" => process_review_comment(state, &payload).await,
        _ => Ok(()),
    }
}

async fn github_context(
    state: &AppState,
    payload: &Value,
    pr_override: Option<u64>,
) -> anyhow::Result<(AppConfig, GitHubClient, u64)> {
    let installation_id = payload["installation"]["id"]
        .as_u64()
        .context("webhook is missing installation.id")?;
    let repository = string_at(payload, &["repository", "full_name"])
        .context("webhook is missing repository.full_name")?;
    let pr_number = pr_override
        .or_else(|| payload["pull_request"]["number"].as_u64())
        .or_else(|| payload["issue"]["number"].as_u64())
        .context("webhook is missing pull request number")?;
    let token = state.auth.installation_token(installation_id).await?;
    let mut config = AppConfig::from_local_env()?;
    let identity = RepositoryIdentity::parse(&repository)?;
    config.authorize_evaluator_for_repository(&identity)?;
    let (api_url, web_url) = github_host_urls(&identity);
    config.source_control = crate::config::ScmConfig {
        provider: "github".into(),
        token,
        repository,
        change_request_number: pr_number,
        api_url,
        web_url,
    };
    let github = GitHubClient::from_scm_config(&config.source_control)?;
    Ok((config, github, installation_id))
}

async fn app_config_for_event(
    state: &AppState,
    payload: &Value,
    authorized_login: Option<&str>,
    pr_override: Option<u64>,
) -> anyhow::Result<Option<(AppConfig, GitHubClient)>> {
    let (mut config, github, consumer_installation) =
        github_context(state, payload, pr_override).await?;
    if let Some(login) = authorized_login
        && !github.user_can_review(login).await?
    {
        return Ok(None);
    }
    require_llm_api_key(&config)?;
    merge_shared_repository_config(state, &mut config, &github, consumer_installation).await?;
    Ok(Some((config, github)))
}

fn require_llm_api_key(config: &AppConfig) -> anyhow::Result<()> {
    if config.llm.api_key.is_empty() {
        bail!("LLM_API_KEY is required by the Cururu GitHub App worker");
    }
    Ok(())
}

async fn merge_shared_repository_config(
    state: &AppState,
    config: &mut AppConfig,
    source_control: &dyn ScmProvider,
    consumer_installation: u64,
) -> anyhow::Result<()> {
    let base_sha = source_control.fetch_base_sha().await?;
    let shared_token =
        if let Some(local_config) = source_control.fetch_config_toml(&base_sha).await? {
            if let Some(shared) = AppConfig::shared_base_from_toml(&local_config)? {
                match state
                    .auth
                    .installation_for_repository(&shared.repository)
                    .await?
                {
                    Some(base_installation) if base_installation != consumer_installation => {
                        Some(state.auth.installation_token(base_installation).await?)
                    }
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
    super::merge_repository_config_with_shared_token(
        config,
        source_control,
        shared_token.as_deref(),
    )
    .await?;
    Ok(())
}

async fn run_pull_request_review(
    store: &DeliveryStore,
    app_slug: &str,
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    request_reviewer: bool,
    force: bool,
) -> anyhow::Result<()> {
    run_pull_request_review_with_evaluator(
        store,
        app_slug,
        config,
        source_control,
        request_reviewer,
        force,
        None,
    )
    .await
}

/// Review the PR and present a publication candidate for approval. Nothing is
/// commented on the PR here except the diff-limit notice.
async fn run_pull_request_review_with_evaluator(
    store: &DeliveryStore,
    app_slug: &str,
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    request_reviewer: bool,
    force: bool,
    evaluator: Option<&dyn crate::evaluation::ReviewEvaluator>,
) -> anyhow::Result<()> {
    review::ensure_evaluator_mode_allowed(config)?;
    let repository = config.source_control.repository.as_str();
    let pr_number = config.source_control.change_request_number;
    let head_sha = source_control.fetch_head_sha().await?;
    if !force
        && (source_control.has_summary_for_revision(&head_sha).await?
            || store
                .has_pending_candidate_for_head(repository, pr_number, &head_sha)
                .await?)
    {
        return Ok(());
    }
    if request_reviewer {
        request_cururu_as_reviewer(source_control, app_slug).await;
    }

    let mut result = match review::run_review_with_evaluator(config, source_control, evaluator)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            if review::publish_diff_limit_notice(config, source_control, &error).await? {
                info!("Cururu App review skipped because the diff exceeded the configured limit");
                return Ok(());
            }
            show_truncation_diagnostic(config, source_control, &error).await;
            return Err(error);
        }
    };
    review::ensure_review_head_unchanged(
        &result.head_sha,
        &source_control.fetch_head_sha().await?,
    )?;
    result.attach_pr_usage(source_control).await;
    let route = crate::auto_approval::decide(config, source_control, &mut result).await?;
    let inline = match config.review.comment_mode {
        CommentMode::Inline => build_inline_drafts(&result),
        CommentMode::Summary => Vec::new(),
    };
    if route == crate::auto_approval::Route::Approved {
        return publish_auto_approved(config, source_control, &result, inline).await;
    }
    let candidate = HostedCandidate::from_review(config, &result, inline, true, true);
    let digest = candidate.digest();
    if !candidate.approvable() {
        source_control
            .create_candidate_check(&CandidateCheck {
                head_sha: &candidate.head_sha,
                external_id: "not-approvable",
                title: "Candidate cannot be approved",
                summary: &candidate.check_summary(true),
                text: &candidate.approval_instructions(true),
                approval_action: false,
            })
            .await?;
        info!("Cururu App candidate exceeds the inspection limit; nothing was published");
        return Ok(());
    }
    if !store
        .store_candidate(
            repository,
            pr_number,
            &digest,
            &candidate.head_sha,
            &serde_json::to_string(&candidate)?,
        )
        .await?
    {
        info!("Cururu App candidate was already handled; nothing new to present");
        return Ok(());
    }
    let preview = candidate.preview();
    let check_id = source_control
        .create_candidate_check(&CandidateCheck {
            head_sha: &candidate.head_sha,
            external_id: &digest,
            title: "Awaiting publication approval",
            summary: &candidate.check_summary(true),
            text: &preview,
            approval_action: true,
        })
        .await?;
    store
        .set_candidate_check_run(repository, &digest, check_id)
        .await?;
    info!(
        findings = candidate.findings_count,
        gate_passed = candidate.quality_gate_passed,
        "Cururu App candidate awaits approval"
    );
    Ok(())
}

async fn request_cururu_as_reviewer(source_control: &dyn ScmProvider, app_slug: &str) {
    let login = format!("{app_slug}[bot]");
    match source_control.request_cururu_as_reviewer(&login).await {
        Ok(true) => info!("SCM provider accepted Cururu as a requested reviewer"),
        Ok(false) => info!("SCM provider does not accept Cururu as a requested reviewer"),
        Err(_) => warn!("could not request Cururu as a reviewer; continuing"),
    }
}

/// The repository's opt-in policy replaces the per-candidate approval; the
/// APPROVE review already submitted replaces the advisory formal review.
async fn publish_auto_approved(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    result: &review::ReviewOutput,
    inline: Vec<crate::scm::ReviewCommentDraft>,
) -> anyhow::Result<()> {
    let candidate = HostedCandidate::from_review(config, result, inline, true, false);
    hosted_publication::publish_candidate(&candidate, source_control, None)
        .await
        .context("the PR was auto-approved but publishing its review did not complete")?;
    info!("Cururu App auto-approved the PR and published its review");
    Ok(())
}

/// Publish an approved candidate after rechecking the approver, the candidate
/// state, the PR head and the trusted configuration. Every outcome is final:
/// a stale candidate is invalidated and a failed write is recorded as
/// uncertain, never retried automatically.
async fn publish_approved_candidate(
    state: &AppState,
    payload: &Value,
    stored: &StoredCandidate,
    approver: &str,
) -> anyhow::Result<()> {
    let Some((config, github)) =
        app_config_for_event(state, payload, Some(approver), Some(stored.pr_number)).await?
    else {
        info!("ignored approval by an account without write access");
        return Ok(());
    };
    let repository = config.source_control.repository.clone();
    let candidate: HostedCandidate =
        serde_json::from_str(&stored.candidate).context("stored candidate is invalid")?;
    anyhow::ensure!(
        candidate.digest() == stored.digest,
        "stored candidate does not match its digest"
    );
    let source_control: &dyn ScmProvider = &github;
    let finish_check = |conclusion: &'static str, title: &'static str, summary: String| {
        let check = stored.check_run_id;
        async move {
            if let Some(check) = check
                && let Err(error) = source_control
                    .finish_candidate_check(check, conclusion, title, &summary)
                    .await
            {
                warn!(%error, "could not update the Cururu candidate check");
            }
        }
    };
    if !state
        .store
        .claim_candidate(&repository, &stored.digest, approver)
        .await?
    {
        info!("ignored approval of a candidate that is not pending or has expired");
        // Re-read: a concurrent approval may have just claimed it.
        let current = state
            .store
            .find_candidate(&repository, &stored.digest)
            .await?;
        if current.is_some_and(|current| matches!(current.state.as_str(), "pending" | "superseded"))
        {
            finish_check(
                "neutral",
                "No longer approvable",
                "Nothing was published: this candidate was superseded or expired. Run `/cururu review` to prepare a new candidate.".into(),
            )
            .await;
        }
        return Ok(());
    }
    if let Err(error) =
        hosted_publication::ensure_candidate_current(&candidate, &config, source_control).await
    {
        state
            .store
            .finish_candidate(&repository, &stored.digest, "invalidated")
            .await?;
        finish_check(
            "neutral",
            "Approval invalidated",
            format!(
                "Nothing was published: {error}. Run `/cururu review` to prepare a new candidate."
            ),
        )
        .await;
        return Ok(());
    }
    let bot_login = format!("{}[bot]", state.app_slug);
    match hosted_publication::publish_candidate(&candidate, source_control, Some(&bot_login)).await
    {
        Ok(()) => {
            state
                .store
                .finish_candidate(&repository, &stored.digest, "published")
                .await?;
            finish_check(
                "success",
                "Published",
                format!("Published after approval by @{approver}."),
            )
            .await;
            info!("Cururu App published an approved candidate");
        }
        Err(error) => {
            state
                .store
                .finish_candidate(&repository, &stored.digest, "uncertain")
                .await?;
            warn!(%error, "approved publication failed; outcome may be partial");
            finish_check(
                "failure",
                "Publication incomplete",
                "Publication failed and may be partial. Inspect the PR before running `/cururu review` again; Cururu does not retry automatically.".into(),
            )
            .await;
        }
    }
    Ok(())
}

async fn process_check_run(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    if payload["action"].as_str() != Some("requested_action")
        || payload["requested_action"]["identifier"].as_str()
            != Some(hosted_publication::PUBLISH_ACTION_ID)
    {
        return Ok(());
    }
    let repository = string_at(payload, &["repository", "full_name"])
        .context("webhook is missing repository.full_name")?;
    let check_run_id = payload["check_run"]["id"]
        .as_u64()
        .context("webhook is missing check_run.id")?;
    let approver = string_at(payload, &["sender", "login"]).unwrap_or_default();
    if approver.is_empty() || approver.ends_with("[bot]") {
        return Ok(());
    }
    let Some(stored) = state
        .store
        .find_candidate_by_check_run(&repository, check_run_id)
        .await?
    else {
        return Ok(());
    };
    publish_approved_candidate(state, payload, &stored, &approver).await
}

/// Show an incomplete-model-response diagnostic for inspection without
/// commenting on the PR; it may contain model-written recommendations.
async fn show_truncation_diagnostic(
    config: &AppConfig,
    source_control: &dyn ScmProvider,
    error: &anyhow::Error,
) {
    let Some(failure) = error.downcast_ref::<agent::InvalidReviewOutput>() else {
        return;
    };
    let mut body = format!(
        "The model reported an incomplete response (finish reason: `{}`). No partial review was prepared.",
        failure.finish_reason
    );
    if config.review.recommendations
        && let Ok((recommendation, usage)) =
            agent::recommend_after_truncation(&config.llm, &failure.finish_reason).await
    {
        use std::fmt::Write as _;
        body.push_str("\n\n### Configuration recommendation\n\n");
        body.push_str(&recommendation);
        body.push_str("\n\n**Additional recommendation cost:** ");
        match usage.and_then(|u| u.cost) {
            Some(cost) => {
                let _ = write!(body, "provider-reported `${cost:.6}`.");
            }
            None => body.push_str("unavailable from the LLM provider; no estimate is shown."),
        }
    }
    let Ok(head_sha) = source_control.fetch_head_sha().await else {
        return;
    };
    if let Err(check_error) = source_control
        .create_candidate_check(&CandidateCheck {
            head_sha: &head_sha,
            external_id: "diagnostic",
            title: "Review could not complete",
            summary: "No review was prepared. See the diagnostic below.",
            text: &body,
            approval_action: false,
        })
        .await
    {
        warn!(error = %check_error, "could not show the truncation diagnostic");
    }
}

async fn process_issue_comment(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    if payload["action"].as_str() != Some("created") || payload["issue"]["pull_request"].is_null() {
        return Ok(());
    }
    let body = string_at(payload, &["comment", "body"]).unwrap_or_default();
    let login = string_at(payload, &["comment", "user", "login"]).unwrap_or_default();
    let kind = string_at(payload, &["comment", "user", "type"]).unwrap_or_default();
    if kind == "Bot" || login.ends_with("[bot]") {
        return Ok(());
    }
    let trimmed = body.trim();
    if let Some(digest) = hosted_publication::parse_approval_command(trimmed) {
        let repository = string_at(payload, &["repository", "full_name"])
            .context("webhook is missing repository.full_name")?;
        let number = payload["issue"]["number"]
            .as_u64()
            .context("issue comment has no PR number")?;
        if let Some(stored) = state.store.find_candidate(&repository, &digest).await?
            && stored.pr_number == number
        {
            return publish_approved_candidate(state, payload, &stored, &login).await;
        }
        return Ok(());
    }
    let command = matches!(trimmed, "/cururu review" | "/cururu review --full");
    if !command && !mentions_cururu(&body, &state.app_slug) {
        return Ok(());
    }
    let Some((mut config, github)) =
        app_config_for_event(state, payload, Some(&login), None).await?
    else {
        return Ok(());
    };
    if command {
        if trimmed == "/cururu review --full" {
            config.review.policy.incremental = false;
        }
        return run_pull_request_review(
            &state.store,
            &state.app_slug,
            &config,
            &github,
            false,
            true,
        )
        .await;
    }
    let number = payload["issue"]["number"]
        .as_u64()
        .context("issue comment has no PR number")?;
    answer_mention(
        state,
        &github,
        &config,
        MentionRequest {
            body: &body,
            pr_number: number,
            reply_to: None,
            mention_login: Some(&login),
            actor_login: &login,
        },
    )
    .await
}

async fn process_review_comment(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    if payload["action"].as_str() != Some("created") {
        return Ok(());
    }
    let body = string_at(payload, &["comment", "body"]).unwrap_or_default();
    let login = string_at(payload, &["comment", "user", "login"]).unwrap_or_default();
    let kind = string_at(payload, &["comment", "user", "type"]).unwrap_or_default();
    if kind == "Bot" || login.ends_with("[bot]") {
        return Ok(());
    }
    let mention = mentions_cururu(&body, &state.app_slug);
    let parent_id = payload["comment"]["in_reply_to_id"].as_u64();
    if !mention && parent_id.is_none() {
        return Ok(());
    }
    let (mut config, github, consumer_installation) = github_context(state, payload, None).await?;
    let source_control: &dyn ScmProvider = &github;
    let is_reply_to_cururu = if let Some(parent_id) = parent_id {
        source_control
            .list_review_comments()
            .await?
            .into_iter()
            .find(|comment| comment.id == parent_id)
            .is_some_and(|comment| {
                source_control
                    .is_cururu_review_comment(&comment, Some(&format!("{}[bot]", state.app_slug)))
            })
    } else {
        false
    };
    if !is_reply_to_cururu && !mention {
        return Ok(());
    }
    if !source_control.user_can_review(&login).await? {
        return Ok(());
    }
    require_llm_api_key(&config)?;
    merge_shared_repository_config(state, &mut config, source_control, consumer_installation)
        .await?;
    let number = payload["pull_request"]["number"]
        .as_u64()
        .context("review comment has no PR number")?;
    let path = string_at(payload, &["comment", "path"]).unwrap_or_default();
    let line = payload["comment"]["line"].as_u64();
    let question = format!(
        "Review thread at {path}:{}\n{body}",
        line.map_or_else(|| "file".into(), |n| n.to_string())
    );
    answer_mention(
        state,
        source_control,
        &config,
        MentionRequest {
            body: &question,
            pr_number: number,
            reply_to: parent_id,
            mention_login: None,
            actor_login: &login,
        },
    )
    .await
}

async fn answer_mention(
    state: &AppState,
    source_control: &dyn ScmProvider,
    config: &AppConfig,
    request: MentionRequest<'_>,
) -> anyhow::Result<()> {
    if !state
        .store
        .allow_mention(
            &config.source_control.repository,
            request.pr_number,
            request.actor_login,
            MENTION_RATE_WINDOW_SECS,
            MAX_MENTIONS_PER_USER_PER_PR,
        )
        .await?
    {
        info!("Cururu mention rate limit reached");
        return Ok(());
    }
    let (diff, diff_truncated) = source_control
        .fetch_diff_with_limit(MAX_MENTION_DIFF_BYTES)
        .await?;
    let base_sha = source_control.fetch_base_sha().await?;
    let context_files =
        crate::context::fetch_context(&config.context, source_control, &base_sha).await?;
    let completeness_note = if diff_truncated {
        "The diff excerpt is truncated; base the answer only on supplied evidence.\n\n"
    } else {
        ""
    };
    let context = format!(
        "Untrusted change-request diff (stream-limited to {MAX_MENTION_DIFF_BYTES} bytes):\n{completeness_note}{diff}\n\nTrusted base-revision context:\n{}",
        crate::context::render(&context_files)
    );
    let answer = agent::answer_conversation(
        &config.llm,
        &config.review.tone,
        &config.review.technical_level,
        request.body,
        &context,
    )
    .await?;
    if answer.is_empty() {
        return Ok(());
    }
    if let Some(comment_id) = request.reply_to {
        source_control
            .reply_review_comment(comment_id, &answer)
            .await
    } else {
        let response = request
            .mention_login
            .map_or_else(|| answer.clone(), |login| format!("@{login} {answer}"));
        source_control.create_issue_comment(&response).await
    }
}

fn mentions_cururu(body: &str, slug: &str) -> bool {
    let expected = [format!("@{slug}[bot]"), format!("@{slug}")];
    body.to_ascii_lowercase().split_whitespace().any(|word| {
        let word = word.trim_matches(|ch: char| ",;:!?()<>\"'".contains(ch));
        expected.iter().any(|mention| word == mention)
    })
}

fn should_review_pull_request(action: &str, draft: bool) -> bool {
    !draft
        && matches!(
            action,
            "opened" | "synchronize" | "reopened" | "ready_for_review"
        )
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut value = value;
    for key in path {
        value = value.get(key)?;
    }
    value.as_str().map(str::to_owned)
}

fn required_env(name: &str) -> anyhow::Result<String> {
    let value = std::env::var(name).with_context(|| format!("{name} is required"))?;
    if value.trim().is_empty() {
        bail!("{name} must not be empty");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synthetic_http_test_server() {
        if std::env::var_os("CURURU_RUN_SYNTHETIC_HTTP_TEST_SERVER").is_some() {
            super::serve().await.unwrap();
        }
    }

    #[test]
    fn signed_webhook_revalidates_config_after_http_evaluator() {
        let status = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/webhook_review_smoke.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .arg("--test-evaluator")
            .status()
            .unwrap();
        assert!(status.success());
    }

    struct ObserveScm {
        changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        writes: std::sync::atomic::AtomicUsize,
        checks: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ScmProvider for ObserveScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("head".into())
        }
        async fn fetch_base_sha(&self) -> anyhow::Result<String> {
            Ok("base".into())
        }
        async fn fetch_config_toml(&self, _: &str) -> anyhow::Result<Option<String>> {
            use std::sync::atomic::Ordering;
            Ok(Some(
                if self.changed.load(Ordering::SeqCst) {
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
        async fn reconcile_review_comments(
            &self,
            _: &str,
            _: &[crate::scm::ReviewCommentDraft],
        ) -> anyhow::Result<()> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn create_pending_formal_review(&self, _: &str, _: &str) -> anyhow::Result<u64> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(1)
        }
        async fn submit_formal_review(&self, _: u64) -> anyhow::Result<()> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn upsert_summary_comment(&self, _: &str) -> anyhow::Result<()> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn upsert_limit_notice(&self, _: &str) -> anyhow::Result<()> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn create_issue_comment(&self, _: &str) -> anyhow::Result<()> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn create_candidate_check(&self, check: &CandidateCheck<'_>) -> anyhow::Result<u64> {
            assert!(check.approval_action);
            assert_eq!(
                hosted_publication::preview_digest(check.text),
                check.external_id
            );
            Ok(self
                .checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst) as u64
                + 1)
        }
    }

    async fn temporary_store() -> (tempfile::TempDir, DeliveryStore) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("cururu.db").display()
        );
        let store = DeliveryStore::connect(&url).await.unwrap();
        (dir, store)
    }

    struct ObserveEvaluator {
        changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        mutate: bool,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::evaluation::ReviewEvaluator for ObserveEvaluator {
        async fn evaluate(
            &self,
            findings: &[crate::agent::ReviewFinding],
            _: &[crate::diff::ChangedFile],
        ) -> anyhow::Result<crate::evaluation::EvaluationReport> {
            assert_eq!(findings.len(), 1);
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.changed
                .store(self.mutate, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::evaluation::EvaluationReport {
                model: "fixture".into(),
                findings: vec![crate::evaluation::EvaluatedFinding {
                    finding: findings[0].clone(),
                    judgment: crate::evaluation::FindingJudgment {
                        defect_probability: 0.9,
                        severity: Some(crate::evaluation::SeverityJudgment::High),
                        severity_confidence: Some(0.9),
                    },
                    suppressed: false,
                    published: false,
                }],
                omitted_findings: 0,
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn hosted_observe_config_change_prevents_all_publication() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let review = serde_json::json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[{"severity":"high","path":"a","line":1,"title":"fixture","message":"issue","suggestion":"fix","confidence":0.99}]}).to_string();
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]})))
            .expect(2).mount(&server).await;
        for mutate in [false, true] {
            let (_dir, store) = temporary_store().await;
            let changed = Arc::new(AtomicBool::new(false));
            let scm = ObserveScm {
                changed: changed.clone(),
                writes: AtomicUsize::new(0),
                checks: AtomicUsize::new(0),
            };
            let evaluator = ObserveEvaluator {
                changed: changed.clone(),
                mutate,
                calls: AtomicUsize::new(0),
            };
            let mut config = AppConfig::from_local_env().unwrap();
            config.llm.provider = crate::config::LlmProvider::OpenAI;
            config.llm.base_url = server.uri();
            config.llm.api_key = "fixture".into();
            config.evaluator.mode = Some(crate::evaluation::EvaluationMode::Observe);
            config.evaluator.repository_allowed = true;
            config.source_control.repository = "owner/repo".into();
            config.source_control.change_request_number = 1;
            config.trusted_config_snapshot =
                Some(("base".into(), Some(review::trusted_config_hash("original"))));
            let result = run_pull_request_review_with_evaluator(
                &store,
                "fixture",
                &config,
                &scm,
                false,
                true,
                Some(&evaluator),
            )
            .await;
            assert_eq!(evaluator.calls.load(Ordering::SeqCst), 1);
            assert_eq!(scm.writes.load(Ordering::SeqCst), 0);
            if mutate {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("configuration changed")
                );
                assert_eq!(scm.checks.load(Ordering::SeqCst), 0);
                continue;
            }
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(scm.checks.load(Ordering::SeqCst), 1);
            let stored = store
                .find_candidate_by_check_run("owner/repo", 1)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.state, "pending");
            let candidate: HostedCandidate = serde_json::from_str(&stored.candidate).unwrap();
            assert_eq!(candidate.inline_comments.len(), 1);
            // A trusted configuration change after preparation invalidates it.
            let mut changed_config = config.clone();
            changed_config.trusted_config_snapshot =
                Some(("base".into(), Some(review::trusted_config_hash("changed"))));
            assert!(
                hosted_publication::ensure_candidate_current(&candidate, &changed_config, &scm)
                    .await
                    .is_err()
            );
            hosted_publication::ensure_candidate_current(&candidate, &config, &scm)
                .await
                .unwrap();
            hosted_publication::publish_candidate(&candidate, &scm, Some("fixture[bot]"))
                .await
                .unwrap();
            assert_eq!(scm.writes.load(Ordering::SeqCst), 4);
            // A duplicate event for the same head does not prepare again.
            run_pull_request_review_with_evaluator(
                &store,
                "fixture",
                &config,
                &scm,
                false,
                false,
                Some(&evaluator),
            )
            .await
            .unwrap();
            assert_eq!(scm.checks.load(Ordering::SeqCst), 1);
        }
    }

    struct BlockedScm;

    #[async_trait::async_trait]
    impl ScmProvider for BlockedScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            panic!("blocked webhook review must not access the SCM")
        }
        async fn fetch_diff_with_limit(&self, _: usize) -> anyhow::Result<(String, bool)> {
            panic!("blocked webhook review must not fetch a diff")
        }
        async fn request_cururu_as_reviewer(&self, _: &str) -> anyhow::Result<bool> {
            panic!("blocked webhook review must not request a reviewer")
        }
        async fn upsert_summary_comment(&self, _: &str) -> anyhow::Result<()> {
            panic!("blocked webhook review must not publish")
        }
    }

    #[tokio::test]
    async fn webhook_review_rejects_filter_before_any_side_effect() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let mut config = AppConfig::from_local_env().unwrap();
        config.evaluator.mode = Some(crate::evaluation::EvaluationMode::Filter);
        config.evaluator.repository_allowed = true;
        config.llm.base_url = server.uri();
        config.llm.api_key = "fixture".into();
        let (_dir, store) = temporary_store().await;
        for force in [false, true] {
            let error =
                run_pull_request_review(&store, "fixture", &config, &BlockedScm, true, force)
                    .await
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("evaluator filtering is disabled")
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn mention_matching_accepts_app_identity_and_slug() {
        assert!(mentions_cururu("Hey @cururu[bot], why?", "cururu"));
        assert!(mentions_cururu("@Cururu please explain", "cururu"));
        assert!(!mentions_cururu("@cururux please explain", "cururu"));
    }

    #[test]
    fn supported_pull_request_events_are_distinct_from_other_actions() {
        assert!(should_review_pull_request("opened", false));
        assert!(should_review_pull_request("synchronize", false));
        assert!(!should_review_pull_request("closed", false));
        assert!(!should_review_pull_request("opened", true));
    }

    #[test]
    fn webhook_endpoint_is_versioned_without_redundant_api_prefix() {
        assert_eq!(GITHUB_WEBHOOK_PATH, "/v1/webhooks/github");
    }

    #[test]
    fn webhook_signature_requires_exact_body_and_sha256_header() {
        let secret = b"test-secret";
        let body = b"{\"action\":\"opened\"}";
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();
        headers.insert("x-hub-signature-256", signature.parse().unwrap());
        assert!(verify_signature(secret, &headers, body));
        assert!(!verify_signature(secret, &headers, b"tampered"));
        assert!(!verify_signature(b"wrong-secret", &headers, body));
    }
}
