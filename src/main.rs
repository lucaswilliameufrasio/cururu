mod agent;
mod commands;
mod config;
mod context;
mod diff;
mod github;
mod output;
mod provider;
mod quality;
mod retry;
mod review;

use anyhow::Context;
use clap::{Parser, Subcommand};
use config::{AppConfig, CommentMode};
use std::io::Write;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug, Parser)]
#[command(name = "cururu", version, about = "Rust GitHub Actions PR review bot")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Review the current PR and post a GitHub summary comment.
    Review,
    /// Review the PR and print the JSON result without posting to GitHub.
    DryRun,
    /// Fetch and print the PR diff.
    PrintDiff,
    /// Print the merged configuration.
    PrintConfig,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let issue_comment_command =
        if std::env::var("GITHUB_EVENT_NAME").as_deref() == Ok("issue_comment") {
            commands::parse_issue_comment(
                &std::env::var("GITHUB_EVENT_PATH").context("missing GITHUB_EVENT_PATH")?,
            )?
        } else {
            None
        };
    if std::env::var("GITHUB_EVENT_NAME").as_deref() == Ok("issue_comment")
        && issue_comment_command.is_none()
    {
        return Ok(());
    }
    let mut config = AppConfig::from_env().context("failed to load configuration")?;
    let github = github::GitHubClient::new(&config.github)?;
    if let Some(command) = &issue_comment_command
        && !github.user_can_review(&command.login).await?
    {
        return Ok(());
    }
    // Load and merge .cururu.toml from the base commit if present
    match cli.command {
        Command::PrintConfig => {
            if let Ok(base_sha) = github.fetch_base_sha().await {
                if let Ok(Some(toml_raw)) = github.fetch_config_toml(&base_sha).await {
                    config.merge_toml_str(&toml_raw)?;
                    println!("Merged configuration from .cururu.toml:");
                } else {
                    println!("No .cururu.toml found at base commit {base_sha}");
                }
            } else {
                println!("Could not fetch base SHA (are you on a PR?)");
            }
            print_redacted_config(&config);
            return Ok(());
        }
        Command::Review | Command::DryRun => {
            if let Ok(base_sha) = github.fetch_base_sha().await
                && let Ok(Some(toml_raw)) = github.fetch_config_toml(&base_sha).await
            {
                config.merge_toml_str(&toml_raw)?;
            }
        }
        Command::PrintDiff => {}
    }

    if issue_comment_command
        .as_ref()
        .is_some_and(|command| command.full)
    {
        config.review.policy.incremental = false;
    }

    match cli.command {
        Command::PrintDiff => {
            let diff = github.fetch_pr_diff().await?;
            println!("{diff}");
        }
        Command::PrintConfig => {}
        Command::DryRun => {
            let result = review::run_review(&config, &github).await?;
            let report = quality::evaluate(&result.review, config.review.policy.fail_on);
            write_action_outputs(&report)?;
            println!("{}", serde_json::to_string_pretty(&result.review)?);
        }
        Command::Review => {
            if config.review.policy.incremental {
                let head_sha = github.fetch_head_sha().await?;
                if github.summary_has_head(&head_sha).await? {
                    println!("Cururu: review already exists for head {head_sha}");
                    return Ok(());
                }
            }
            let result = review::run_review(&config, &github).await?;
            let report = quality::evaluate(&result.review, config.review.policy.fail_on);
            write_action_outputs(&report)?;

            match config.review.comment_mode {
                CommentMode::Inline => {
                    let head_sha = github.fetch_head_sha().await?;
                    let drafts = build_inline_drafts(&result);
                    github.reconcile_review_comments(&head_sha, &drafts).await?;
                    // Keep a compact summary in the PR conversation as well.
                    let body = output::render_summary_comment(&result);
                    github.upsert_summary_comment(&body).await?;
                }
                CommentMode::Summary => {
                    let body = output::render_summary_comment(&result);
                    github.upsert_summary_comment(&body).await?;
                }
            }

            println!("{}", serde_json::to_string_pretty(&result.review)?);
            if !report.passed {
                anyhow::bail!(
                    "quality gate failed: {} finding(s) at or above configured threshold",
                    report.findings_count
                );
            }
        }
    }

    Ok(())
}

fn write_action_outputs(report: &quality::QualityReport) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("GITHUB_OUTPUT") else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .context("failed to open GITHUB_OUTPUT")?;
    writeln!(file, "quality_gate={}", report.status)?;
    writeln!(file, "quality_gate_passed={}", report.passed)?;
    writeln!(file, "findings_count={}", report.findings_count)?;
    writeln!(file, "critical_count={}", report.critical_count)?;
    writeln!(file, "high_count={}", report.high_count)?;
    writeln!(file, "medium_count={}", report.medium_count)?;
    writeln!(file, "low_count={}", report.low_count)?;
    writeln!(file, "highest_severity={}", report.highest_severity)?;
    Ok(())
}

fn print_redacted_config(config: &AppConfig) {
    println!("provider: {:?}", config.llm.provider);
    println!("base_url: {}", config.llm.base_url);
    println!("model: {}", config.llm.model);
    println!("temperature: {}", config.llm.temperature);
    println!("max_output_tokens: {}", config.llm.max_output_tokens);
    println!("repository: {}", config.github.repository);
    println!("pr_number: {}", config.github.pr_number);
    println!("review: {:#?}", config.review);
    println!("context: {:#?}", config.context);
    println!("summary: {:#?}", config.summary);
    println!("secrets: [redacted]");
}

/// Build review comment drafts from findings. Findings with a valid diff line
/// are anchored inline; others fall back to a file-level comment.
fn build_inline_drafts(result: &review::ReviewOutput) -> Vec<github::ReviewCommentDraft> {
    result
        .review
        .findings
        .iter()
        .map(|f| {
            let line = f
                .line
                .filter(|line| diff::is_valid_anchor(&result.changed_files, &f.path, *line));
            github::ReviewCommentDraft {
                path: f.path.clone(),
                line,
                body: output::render_inline_finding(f),
            }
        })
        .collect()
}
