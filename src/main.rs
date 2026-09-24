mod agent;
mod analysis;
mod app;
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
#[command(
    name = "cururu",
    version,
    about = "Self-service code review for GitHub pull requests"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add a starter configuration and GitHub Actions workflow to this repository.
    Init,
    /// Run the self-hosted GitHub App webhook server.
    Serve,
    /// Create a consistent online backup of the configured SQLite database.
    BackupSqlite {
        /// Destination path for the backup copy.
        destination: std::path::PathBuf,
    },
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
    if matches!(cli.command, Command::Init) {
        initialize_repository()?;
        return Ok(());
    }
    match &cli.command {
        Command::Serve => {
            app::serve().await?;
            return Ok(());
        }
        Command::BackupSqlite { destination } => {
            app::backup_sqlite(destination).await?;
            return Ok(());
        }
        _ => {}
    }
    if matches!(cli.command, Command::PrintConfig) {
        let mut config =
            AppConfig::from_local_env().context("failed to load local configuration defaults")?;
        match std::fs::read_to_string(".cururu.toml") {
            Ok(local_raw) => {
                if let Some(shared) = AppConfig::shared_base_from_toml(&local_raw)? {
                    let shared_token = std::env::var("CURURU_SHARED_CONFIG_TOKEN")
                        .ok()
                        .filter(|token| !token.is_empty())
                        .unwrap_or_else(|| config.github.token.clone());
                    if shared_token.is_empty() {
                        anyhow::bail!(
                            "GITHUB_TOKEN or CURURU_SHARED_CONFIG_TOKEN is required to inspect a private or remote shared base"
                        );
                    }
                    let mut shared_github_config = config.github.clone();
                    shared_github_config.token = shared_token;
                    let shared_github = github::GitHubClient::new(&shared_github_config)?;
                    let shared_raw = shared_github
                        .fetch_repository_file_at_ref(
                            &shared.repository,
                            &shared.path,
                            &shared.commit,
                        )
                        .await?;
                    if AppConfig::shared_base_from_toml(&shared_raw)?.is_some() {
                        anyhow::bail!("shared configuration cannot extend another base");
                    }
                    config
                        .merge_toml_str(&shared_raw)
                        .context("invalid shared base configuration")?;
                    config.merge_toml_str(&AppConfig::compose_toml(&shared_raw, &local_raw)?)?;
                    println!(
                        "Merged shared configuration from {}/{} at {}.",
                        shared.repository, shared.path, shared.commit
                    );
                } else {
                    config.merge_toml_str(&local_raw)?;
                    println!("Loaded local configuration from .cururu.toml.");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                println!("No .cururu.toml found in the current directory.");
            }
            Err(error) => return Err(error).context("failed to read .cururu.toml"),
        }
        print_redacted_config(&config);
        return Ok(());
    }
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
    // Load trusted configuration only for commands that use review settings.
    if matches!(
        cli.command,
        Command::PrintConfig | Command::Review | Command::DryRun
    ) {
        let loaded = merge_repository_config(&mut config, &github).await?;
        if matches!(cli.command, Command::PrintConfig) {
            if loaded {
                println!("Merged configuration from trusted base commit:");
            } else {
                println!("No .cururu.toml found at the trusted base commit.");
            }
            print_redacted_config(&config);
            return Ok(());
        }
    }

    if issue_comment_command
        .as_ref()
        .is_some_and(|command| command.full)
    {
        config.review.policy.incremental = false;
    }

    match cli.command {
        Command::Serve | Command::BackupSqlite { .. } => {
            unreachable!("handled before loading review configuration")
        }
        Command::Init => unreachable!("handled before loading runtime configuration"),
        Command::PrintDiff => {
            let diff = github.fetch_pr_diff().await?;
            println!("{diff}");
        }
        Command::PrintConfig => {}
        Command::DryRun => {
            let result = review::run_review(&config, &github).await?;
            let report = quality::evaluate(&result.review, config.review.policy.fail_on);
            write_action_outputs(&report)?;
            write_analysis_outputs(&result.analysis)?;
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
            write_analysis_outputs(&result.analysis)?;

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

async fn merge_repository_config(
    config: &mut AppConfig,
    github: &github::GitHubClient,
) -> anyhow::Result<bool> {
    let shared_token = std::env::var("CURURU_SHARED_CONFIG_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    merge_repository_config_with_shared_token(config, github, shared_token.as_deref()).await
}

async fn merge_repository_config_with_shared_token(
    config: &mut AppConfig,
    github: &github::GitHubClient,
    shared_token: Option<&str>,
) -> anyhow::Result<bool> {
    let base_sha = github
        .fetch_base_sha()
        .await
        .context("failed to resolve trusted PR base")?;
    let Some(local_raw) = github.fetch_config_toml(&base_sha).await? else {
        return Ok(false);
    };

    let mut validation = config.clone();
    validation.merge_toml_str(&local_raw)?;
    if let Some(shared) = AppConfig::shared_base_from_toml(&local_raw)? {
        let mut shared_github_config = config.github.clone();
        if let Some(token) = shared_token {
            shared_github_config.token = token.to_string();
        }
        let shared_github = github::GitHubClient::new(&shared_github_config)?;
        let shared_raw = shared_github
            .fetch_repository_file_at_ref(&shared.repository, &shared.path, &shared.commit)
            .await
            .with_context(|| {
                format!(
                    "failed to load shared config {}/{} at {} (verify GitHub installation access)",
                    shared.repository, shared.path, shared.commit
                )
            })?;
        if AppConfig::shared_base_from_toml(&shared_raw)?.is_some() {
            anyhow::bail!(
                "shared configuration cannot extend another base (recursive references are disabled)"
            );
        }
        validation
            .merge_toml_str(&shared_raw)
            .context("invalid shared base configuration")?;
        let effective = AppConfig::compose_toml(&shared_raw, &local_raw)?;
        config.merge_toml_str(&effective)?;
    } else {
        config.merge_toml_str(&local_raw)?;
    }
    Ok(true)
}

fn initialize_repository() -> anyhow::Result<()> {
    initialize_repository_at(std::path::Path::new("."))
}

fn initialize_repository_at(root: &std::path::Path) -> anyhow::Result<()> {
    use std::{fs::OpenOptions, io::Write};

    const CONFIG_PATH: &str = ".cururu.toml";
    const WORKFLOW_PATH: &str = ".github/workflows/cururu-review.yml";
    const CONFIG_TEMPLATE: &str = include_str!("../.cururu.toml");
    const WORKFLOW_TEMPLATE: &str = include_str!("../docs/examples/cururu-review.yml");

    for path in [CONFIG_PATH, WORKFLOW_PATH] {
        if std::fs::symlink_metadata(root.join(path)).is_ok() {
            anyhow::bail!(
                "{path} already exists; move it or merge it manually before running `cururu init`"
            );
        }
    }

    std::fs::create_dir_all(root.join(".github/workflows"))
        .context("failed to create .github/workflows")?;
    let mut config = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(CONFIG_PATH))
        .with_context(|| format!("failed to create {CONFIG_PATH} without overwriting"))?;
    config
        .write_all(CONFIG_TEMPLATE.as_bytes())
        .with_context(|| format!("failed to write {CONFIG_PATH}"))?;
    let mut workflow = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(WORKFLOW_PATH))
    {
        Ok(file) => file,
        Err(error) => {
            let _ = std::fs::remove_file(root.join(CONFIG_PATH));
            return Err(error)
                .with_context(|| format!("failed to create {WORKFLOW_PATH} without overwriting"));
        }
    };
    if let Err(error) = workflow.write_all(WORKFLOW_TEMPLATE.as_bytes()) {
        let _ = std::fs::remove_file(root.join(CONFIG_PATH));
        let _ = std::fs::remove_file(root.join(WORKFLOW_PATH));
        return Err(error).with_context(|| format!("failed to write {WORKFLOW_PATH}"));
    }

    println!("Created {CONFIG_PATH} and {WORKFLOW_PATH}.");
    println!("Add LLM_API_KEY as a repository secret, then review the generated files.");
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

fn write_analysis_outputs(report: &analysis::AnalysisReport) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("GITHUB_OUTPUT") else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .context("failed to open GITHUB_OUTPUT")?;
    writeln!(file, "analysis_status={}", report.status)?;
    writeln!(file, "analysis_tools_total={}", report.tools.len())?;
    writeln!(
        file,
        "analysis_tools_failed={}",
        report
            .tools
            .iter()
            .filter(|tool| tool.status == "failed")
            .count()
    )?;
    writeln!(
        file,
        "analysis_tools_not_run={}",
        report
            .tools
            .iter()
            .filter(|tool| tool.status == "not_run")
            .count()
    )?;
    writeln!(file, "analysis_findings_count={}", report.findings.len())?;
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
    println!("review.max_diff_bytes: {}", config.review.max_diff_bytes);
    println!("review.chunk_bytes: {}", config.review.chunk_bytes);
    println!("review.language: {}", config.review.language);
    println!("review.tone: {}", config.review.tone);
    println!("review.technical_level: {}", config.review.technical_level);
    println!(
        "review.suggestion_detail: {}",
        config.review.suggestion_detail
    );
    println!("review.comment_mode: {:?}", config.review.comment_mode);
    println!("policy: {:#?}", config.review.policy);
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

#[cfg(test)]
mod cli_tests {
    use super::initialize_repository_at;

    #[test]
    fn init_creates_starter_files_and_refuses_to_overwrite() {
        let root = tempfile::tempdir().unwrap();
        initialize_repository_at(root.path()).unwrap();

        let config_path = root.path().join(".cururu.toml");
        let workflow_path = root.path().join(".github/workflows/cururu-review.yml");
        assert!(config_path.is_file());
        assert!(workflow_path.is_file());

        std::fs::write(&config_path, "user config").unwrap();
        let error = initialize_repository_at(root.path()).unwrap_err();
        assert!(error.to_string().contains("already exists"));
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), "user config");
    }
}
