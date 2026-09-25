mod db;
mod github_app;

use crate::{
    agent, build_inline_drafts,
    config::{AppConfig, CommentMode, GitHubConfig},
    github::GitHubClient,
    output, quality, review,
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
use db::{DeliveryStore, QueuedDelivery};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::{sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tracing::{info, warn};

type HmacSha256 = Hmac<Sha256>;
pub const GITHUB_WEBHOOK_PATH: &str = "/v1/webhooks/github";

#[derive(Clone)]
struct AppState {
    store: DeliveryStore,
    auth: github_app::GitHubAppAuth,
    webhook_secret: Arc<Vec<u8>>,
    app_slug: Arc<String>,
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
        .layer(DefaultBodyLimit::max(1_000_000))
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
            let (config, github) = app_config_for_event(state, &payload, None)
                .await?
                .context("PR webhook configuration was not loaded")?;
            run_pull_request_review(state, &config, &github, action == "opened", false).await
        }
        "issue_comment" => process_issue_comment(state, &payload).await,
        "pull_request_review_comment" => process_review_comment(state, &payload).await,
        _ => Ok(()),
    }
}

async fn github_context(
    state: &AppState,
    payload: &Value,
) -> anyhow::Result<(AppConfig, GitHubClient, u64)> {
    let installation_id = payload["installation"]["id"]
        .as_u64()
        .context("webhook is missing installation.id")?;
    let repository = string_at(payload, &["repository", "full_name"])
        .context("webhook is missing repository.full_name")?;
    let (owner, repo) = repository
        .split_once('/')
        .context("repository.full_name must be owner/repo")?;
    let owner = owner.to_string();
    let repo = repo.to_string();
    let pr_number = payload["pull_request"]["number"]
        .as_u64()
        .or_else(|| payload["issue"]["number"].as_u64())
        .context("webhook is missing pull request number")?;
    let token = state.auth.installation_token(installation_id).await?;
    let mut config = AppConfig::from_local_env()?;
    if config.llm.api_key.is_empty() {
        bail!("LLM_API_KEY is required by the Cururu GitHub App worker");
    }
    config.github = GitHubConfig {
        token,
        repository,
        owner,
        repo,
        pr_number,
        api_url: std::env::var("GITHUB_API_URL")
            .unwrap_or_else(|_| "https://api.github.com".to_string()),
        server_url: std::env::var("GITHUB_SERVER_URL")
            .unwrap_or_else(|_| "https://github.com".to_string()),
    };
    let github = GitHubClient::new(&config.github)?;
    Ok((config, github, installation_id))
}

async fn app_config_for_event(
    state: &AppState,
    payload: &Value,
    authorized_login: Option<&str>,
) -> anyhow::Result<Option<(AppConfig, GitHubClient)>> {
    let (mut config, github, consumer_installation) = github_context(state, payload).await?;
    if let Some(login) = authorized_login
        && !github.user_can_review(login).await?
    {
        return Ok(None);
    }
    let base_sha = github.fetch_base_sha().await?;
    let shared_token = if let Some(local_config) = github.fetch_config_toml(&base_sha).await? {
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
    super::merge_repository_config_with_shared_token(&mut config, &github, shared_token.as_deref())
        .await?;
    Ok(Some((config, github)))
}

async fn run_pull_request_review(
    state: &AppState,
    config: &AppConfig,
    github: &GitHubClient,
    request_reviewer: bool,
    force: bool,
) -> anyhow::Result<()> {
    let head_sha = github.fetch_head_sha().await?;
    if !force && github.summary_has_head(&head_sha).await? {
        return Ok(());
    }
    if request_reviewer {
        let login = format!("{}[bot]", state.app_slug);
        match github.request_app_reviewer(&login).await {
            Ok(true) => info!(%login, "GitHub accepted Cururu as a requested reviewer"),
            Ok(false) => {
                info!(%login, "GitHub does not accept this App bot as a requested reviewer");
            }
            Err(error) => {
                warn!(%error, "could not request Cururu as a reviewer; continuing with formal review");
            }
        }
    }

    let result = review::run_review(config, github).await?;
    let report = quality::evaluate(&result.review, config.review.policy.fail_on);
    match config.review.comment_mode {
        CommentMode::Inline => {
            let head = github.fetch_head_sha().await?;
            github
                .reconcile_review_comments(&head, &build_inline_drafts(&result))
                .await?;
        }
        CommentMode::Summary => {}
    }
    let bot_login = format!("{}[bot]", state.app_slug);
    let marker = format!("<!-- cururu:formal-review:v1 head={head_sha} -->");
    if !github.formal_review_exists(&marker, &bot_login).await? {
        github
            .submit_formal_review(&format!(
                "{marker}\n\nCururu has completed an automated review. See the Cururu summary and inline findings in the conversation. This review is advisory and does not replace human review."
            ))
            .await?;
    }
    let summary = output::render_summary_comment(&result);
    github.upsert_summary_comment(&summary).await?;
    info!(
        findings = result.review.findings.len(),
        gate_passed = report.passed,
        "Cururu App review completed"
    );
    Ok(())
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
    let command = matches!(trimmed, "/cururu review" | "/cururu review --full");
    if !command && !mentions_cururu(&body, &state.app_slug) {
        return Ok(());
    }
    let Some((mut config, github)) = app_config_for_event(state, payload, Some(&login)).await?
    else {
        return Ok(());
    };
    if command {
        if trimmed == "/cururu review --full" {
            config.review.policy.incremental = false;
        }
        return run_pull_request_review(state, &config, &github, false, true).await;
    }
    let number = payload["issue"]["number"]
        .as_u64()
        .context("issue comment has no PR number")?;
    answer_mention(&github, &config, &body, number, None, Some(&login)).await
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
    let Some((config, github)) = app_config_for_event(state, payload, Some(&login)).await? else {
        return Ok(());
    };
    let parent_id = payload["comment"]["in_reply_to_id"].as_u64();
    let is_reply_to_cururu = if let Some(parent_id) = parent_id {
        github
            .list_review_comments()
            .await?
            .into_iter()
            .find(|comment| comment.id == parent_id)
            .is_some_and(|comment| {
                GitHubClient::comment_is_cururu(&comment, Some(&format!("{}[bot]", state.app_slug)))
            })
    } else {
        false
    };
    if !is_reply_to_cururu && !mentions_cururu(&body, &state.app_slug) {
        return Ok(());
    }
    let number = payload["pull_request"]["number"]
        .as_u64()
        .context("review comment has no PR number")?;
    let path = string_at(payload, &["comment", "path"]).unwrap_or_default();
    let line = payload["comment"]["line"].as_u64();
    let question = format!(
        "Review thread at {path}:{}\n{body}",
        line.map_or_else(|| "file".into(), |n| n.to_string())
    );
    answer_mention(&github, &config, &question, number, parent_id, None).await
}

async fn answer_mention(
    github: &GitHubClient,
    config: &AppConfig,
    body: &str,
    pr_number: u64,
    reply_to: Option<u64>,
    mention_login: Option<&str>,
) -> anyhow::Result<()> {
    let diff = github.fetch_pr_diff().await?;
    let base_sha = github.fetch_base_sha().await?;
    let context_files = crate::context::fetch_context(
        &config.context,
        &config.github.api_url,
        &config.github.token,
        &config.github.owner,
        &config.github.repo,
        &base_sha,
    )
    .await?;
    let context = format!(
        "Pull request #{pr_number}\n\nUntrusted PR diff:\n{diff}\n\nTrusted base-commit context:\n{}",
        context_files.render()
    );
    let answer = agent::answer_question(
        &config.llm,
        &config.review.tone,
        &config.review.technical_level,
        body,
        &context,
    )
    .await?;
    if answer.is_empty() {
        return Ok(());
    }
    if let Some(comment_id) = reply_to {
        github.reply_review_comment(comment_id, &answer).await
    } else {
        let response =
            mention_login.map_or_else(|| answer.clone(), |login| format!("@{login} {answer}"));
        github.create_issue_comment(&response).await
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
