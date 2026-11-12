mod action_publication;
mod agent;
mod agent_publication;
mod analysis;
mod anthropic;
mod app;
mod auto_approval;
mod commands;
mod config;
mod context;
mod diagnostics;
mod diff;
mod evaluation;
mod github;
mod hosted_approval;
mod hosted_publication;
mod mcp;
mod mcp_install;
mod output;
mod provider;
mod quality;
mod repository;
mod retry;
mod review;
mod review_input;
mod review_usage;
mod scm;
mod setup;

use anyhow::Context;
use clap::{Parser, Subcommand};
use config::{AppConfig, CommentMode};
use std::io::Write;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug, Parser)]
#[command(
    name = "cururu",
    version,
    about = "Self-service code review for hosted change requests"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(serde::Serialize)]
struct DryRunOutput<'a> {
    #[serde(flatten)]
    review: &'a agent::ReviewResult,
    evaluation: &'a Option<evaluation::EvaluationReport>,
}

#[derive(serde::Serialize)]
struct AgentReviewReceipt<'a> {
    source: &'a str,
    revision: &'a str,
    diff_sha256: &'a str,
    snapshot_sha256: &'a str,
    scope: &'a str,
    ignored_paths: &'a [String],
    review: &'a agent::ReviewResult,
}

#[derive(serde::Serialize)]
struct ApiReviewReceipt<'a> {
    #[serde(flatten)]
    receipt: AgentReviewReceipt<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<ApiUsage>,
}

#[derive(serde::Serialize)]
struct ApiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    cached_tokens: u64,
    reasoning_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<f64>,
}

fn complete_api_usage(results: &[agent::ChunkResult]) -> Option<ApiUsage> {
    if results.is_empty() || results.iter().any(|result| result.usage.is_none()) {
        return None;
    }
    let mut total = ApiUsage {
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        cached_tokens: 0,
        reasoning_tokens: 0,
        cost: Some(0.0),
    };
    for result in results {
        let usage = result.usage.as_ref()?;
        total.prompt_tokens = total
            .prompt_tokens
            .checked_add(u64::from(usage.prompt_tokens))?;
        total.completion_tokens = total
            .completion_tokens
            .checked_add(u64::from(usage.completion_tokens))?;
        total.total_tokens = total
            .total_tokens
            .checked_add(u64::from(usage.total_tokens))?;
        total.cached_tokens = total
            .cached_tokens
            .checked_add(u64::from(usage.cached_tokens))?;
        total.reasoning_tokens = total
            .reasoning_tokens
            .checked_add(u64::from(usage.reasoning_tokens))?;
        total.cost = match (total.cost, usage.cost) {
            (Some(a), Some(b)) if b.is_finite() && b >= 0.0 && (a + b).is_finite() => Some(a + b),
            _ => None,
        };
    }
    Some(total)
}

#[cfg(test)]
mod api_usage_tests {
    use super::*;

    fn chunk(usage: Option<cururu_engine::ReviewUsage>) -> agent::ChunkResult {
        agent::ChunkResult {
            review: agent::ReviewResult {
                model: "fixture".into(),
                files_reviewed: 0,
                summary: String::new(),
                findings: vec![],
            },
            usage,
        }
    }

    #[test]
    fn usage_is_only_reported_when_all_chunks_report_it() {
        let usage = cururu_engine::ReviewUsage {
            prompt_tokens: 10,
            completion_tokens: 2,
            total_tokens: 12,
            cached_tokens: 3,
            reasoning_tokens: 0,
            cost: None,
        };
        assert!(complete_api_usage(&[chunk(Some(usage.clone())), chunk(None)]).is_none());
        let total = complete_api_usage(&[chunk(Some(usage.clone())), chunk(Some(usage))]).unwrap();
        assert_eq!(
            (total.prompt_tokens, total.total_tokens, total.cached_tokens),
            (20, 24, 6)
        );
        assert!(total.cost.is_none());
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add a starter configuration and GitHub Actions workflow to this repository.
    Init,
    /// Guide enabling an optional feature, previewing changes before writing.
    Setup {
        #[command(subcommand)]
        target: SetupTarget,
    },
    /// Show which Cururu features the local configuration enables.
    Status,
    /// Run the self-hosted GitHub App webhook server.
    Serve,
    /// Create a consistent online backup of the configured `SQLite` database.
    BackupSqlite {
        /// Destination path for the backup copy.
        destination: std::path::PathBuf,
    },
    /// Review the current change request and prepare a publication candidate
    /// for approval; nothing is commented on the change request.
    Review,
    /// Publish a GitHub Action candidate from a job approved through a
    /// protected GitHub Environment.
    PublishApprovedReview,
    /// Inspect a hosted review candidate and approve exactly that candidate.
    ApproveHostedReview {
        /// Pull request number (defaults to `PR_NUMBER`).
        #[arg(long)]
        pr: Option<u64>,
        /// Environment approved for GitHub Action candidates.
        #[arg(long, default_value = hosted_publication::DEFAULT_APPROVAL_ENVIRONMENT)]
        environment: String,
    },
    /// Review the change request and print JSON without publishing comments.
    DryRun,
    /// Fetch and print the change-request diff.
    PrintDiff,
    /// Print the merged configuration.
    PrintConfig,
    /// Prepare a bounded, read-only review input without invoking an agent.
    PrepareReview {
        /// Review tracked staged and unstaged changes against HEAD, not untracked files.
        #[arg(long, conflicts_with = "pr", required_unless_present = "pr")]
        local: bool,
        /// Review the configured hosted change request.
        #[arg(long, conflicts_with = "local", required_unless_present = "local")]
        pr: bool,
        /// Preview an explicitly selected local SARIF file.
        #[arg(long, requires = "analysis_manifest")]
        sarif: Option<std::path::PathBuf>,
        /// Manifest identifying the revision analyzed by the SARIF file.
        #[arg(long, requires = "sarif")]
        analysis_manifest: Option<std::path::PathBuf>,
    },
    /// Retired: use api-review or an agent's MCP integration instead.
    AgentReview {
        #[arg(long, conflicts_with = "pr", required_unless_present = "pr")]
        local: bool,
        #[arg(long, conflicts_with = "local", required_unless_present = "local")]
        pr: bool,
        /// Explicit HTTP loopback origin of a trusted `OpenCode` V2 server.
        #[arg(long)]
        opencode_server: String,
        /// Confirm sending the diff to the server and its selected provider.
        #[arg(long)]
        confirm_send: bool,
        /// Require the exact snapshot hash previously inspected with prepare-review.
        #[arg(long)]
        expected_snapshot: Option<String>,
        /// Explicit local SARIF evidence (requires --analysis-manifest).
        #[arg(long, requires = "analysis_manifest")]
        sarif: Option<std::path::PathBuf>,
        /// Explicit local manifest tied to the reviewed revision.
        #[arg(long, requires = "sarif")]
        analysis_manifest: Option<std::path::PathBuf>,
    },
    /// Review local changes or a hosted PR using the configured API, without publishing.
    ApiReview {
        /// Review the configured hosted PR instead of local tracked changes.
        #[arg(long)]
        pr: bool,
        /// Confirm sending the review input to the configured API provider.
        #[arg(long)]
        confirm_send: bool,
        /// Require the snapshot previously inspected with prepare-review.
        #[arg(long)]
        expected_snapshot: Option<String>,
        /// Explicit local SARIF evidence (requires --analysis-manifest).
        #[arg(long, requires = "analysis_manifest")]
        sarif: Option<std::path::PathBuf>,
        /// Manifest identifying the revision analyzed by the SARIF file.
        #[arg(long, requires = "sarif")]
        analysis_manifest: Option<std::path::PathBuf>,
    },
    /// Preview and explicitly confirm posting an agent review to its current PR.
    PublishAgentReview {
        /// JSON receipt produced by `api-review --pr` or prepared from MCP results.
        receipt: std::path::PathBuf,
        /// Explicit SARIF used to create this receipt.
        #[arg(long, requires = "analysis_manifest")]
        sarif: Option<std::path::PathBuf>,
        /// Explicit manifest used to create this receipt.
        #[arg(long, requires = "sarif")]
        analysis_manifest: Option<std::path::PathBuf>,
    },
    /// Serve read-only review preparation tools over MCP stdio.
    Mcp,
    /// Create a project MCP configuration only if its file does not exist.
    InstallMcp {
        #[arg(value_enum)]
        client: mcp_install::Client,
        /// Cururu executable visible to the client.
        #[arg(long, default_value = "cururu")]
        executable: String,
    },
    /// Check command fields in an MCP configuration without starting the client.
    CheckMcp {
        #[arg(value_enum)]
        client: mcp_install::Client,
        configuration: std::path::PathBuf,
        #[arg(long, default_value = "cururu")]
        executable: String,
    },
    /// Preview a merge without modifying the existing configuration.
    PreviewMcpMerge {
        #[arg(value_enum)]
        client: mcp_install::Client,
        configuration: std::path::PathBuf,
        #[arg(long, default_value = "cururu")]
        executable: String,
    },
    /// Add the server to an existing Codex TOML file without replacing it.
    UpdateCodexMcp {
        configuration: std::path::PathBuf,
        #[arg(long, default_value = "cururu")]
        executable: String,
    },
    /// Write a merged MCP configuration to a new file without replacing the source.
    WriteMcpMerge {
        #[arg(value_enum)]
        client: mcp_install::Client,
        configuration: std::path::PathBuf,
        destination: std::path::PathBuf,
        #[arg(long, default_value = "cururu")]
        executable: String,
    },
    /// Interactively update an existing JSONC configuration with a new backup.
    UpdateJsoncMcp {
        #[arg(value_enum)]
        client: mcp_install::Client,
        configuration: std::path::PathBuf,
        /// New backup file; an existing file is never overwritten.
        #[arg(long)]
        backup: std::path::PathBuf,
        #[arg(long, default_value = "cururu")]
        executable: String,
    },
}

#[derive(Debug, Subcommand)]
enum SetupTarget {
    /// Add an opt-in `[auto_approval]` policy to `.cururu.toml` and check requirements.
    AutoApproval {
        /// Append the section after an interactive confirmation (default: preview only).
        #[arg(long)]
        write: bool,
    },
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if !matches!(
        cli.command,
        Command::Mcp
            | Command::PrepareReview { .. }
            | Command::AgentReview { .. }
            | Command::ApiReview { .. }
            | Command::PublishAgentReview { .. }
            | Command::ApproveHostedReview { .. }
            | Command::InstallMcp { .. }
            | Command::CheckMcp { .. }
            | Command::PreviewMcpMerge { .. }
            | Command::UpdateCodexMcp { .. }
            | Command::WriteMcpMerge { .. }
            | Command::UpdateJsoncMcp { .. }
    ) {
        dotenvy::dotenv().ok();
    }
    fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    if matches!(cli.command, Command::Init) {
        initialize_repository()?;
        return Ok(());
    }
    match &cli.command {
        Command::Setup {
            target: SetupTarget::AutoApproval { write },
        } => return setup::auto_approval(std::path::Path::new(".cururu.toml"), *write, true).await,
        Command::Status => return setup::status(std::path::Path::new(".cururu.toml")),
        _ => {}
    }
    match &cli.command {
        Command::UpdateJsoncMcp {
            client,
            configuration,
            backup,
            executable,
        } => {
            #[cfg(not(unix))]
            {
                let _ = (client, configuration, backup, executable);
                anyhow::bail!("updating active JSONC is only supported on Unix");
            }
            #[cfg(unix)]
            {
                use std::io::IsTerminal as _;
                anyhow::ensure!(
                    std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
                    "update requires an interactive terminal"
                );
                let update =
                    mcp_install::ActiveJsoncUpdate::prepare(*client, configuration, executable)?;
                let entry = if matches!(client, mcp_install::Client::Opencode) {
                    serde_json::json!({"cururu":{"type":"local","command":[executable,"mcp"]}})
                } else {
                    serde_json::json!({"cururu":{"type":"stdio","command":executable,"args":["mcp"]}})
                };
                eprintln!(
                    "Will add this entry to {}:\n{}\nBackup: {}. Close clients and editors first; writers ignoring advisory locks can still race. Existing content is not printed because it may contain secrets.",
                    configuration.display(),
                    serde_json::to_string_pretty(&entry)?,
                    backup.display()
                );
                eprint!("Type UPDATE to proceed, or anything else to cancel: ");
                std::io::stderr().flush()?;
                let mut confirmation = String::new();
                std::io::stdin().read_line(&mut confirmation)?;
                anyhow::ensure!(confirmation.trim_end() == "UPDATE", "update cancelled");
                update.apply(backup)?;
                println!("Configuration updated; backup created.");
                return Ok(());
            }
        }
        Command::Serve => {
            app::serve().await?;
            return Ok(());
        }
        Command::BackupSqlite { destination } => {
            app::backup_sqlite(destination).await?;
            return Ok(());
        }
        Command::PrepareReview {
            local,
            sarif,
            analysis_manifest,
            ..
        } => {
            let mut prepared = prepare_review_input(*local).await?;
            if let (Some(sarif), Some(manifest)) = (sarif, analysis_manifest) {
                review_input::attach_local_analysis(&mut prepared, sarif, manifest)?;
            }
            println!("{}", serde_json::to_string_pretty(&prepared)?);
            return Ok(());
        }
        Command::ApiReview {
            pr,
            confirm_send,
            expected_snapshot,
            sarif,
            analysis_manifest,
        } => {
            anyhow::ensure!(
                *confirm_send,
                "sending source code requires --confirm-send; inspect prepare-review first"
            );
            let (config, mut prepared) = prepare_api_review(*pr).await?;
            if let (Some(sarif), Some(manifest)) = (sarif, analysis_manifest) {
                review_input::attach_local_analysis(&mut prepared, sarif, manifest)?;
            }
            config.require_llm_api_key()?;
            if let Some(expected) = expected_snapshot {
                anyhow::ensure!(
                    expected == &prepared.snapshot_sha256,
                    "review snapshot differs from the expected snapshot; nothing was sent"
                );
            }
            review_input::ensure_reviewable(&prepared)?;
            let evidence = serde_json::json!({"repository_context":prepared.repository_context, "prior_feedback":prepared.prior_feedback, "analyzer_evidence":prepared.analyzer_evidence, "suggested_changes":prepared.policy.suggested_changes});
            let evidence_text = serde_json::to_string(&evidence)?;
            anyhow::ensure!(
                evidence_text.len() <= 768 * 1024,
                "review evidence exceeds byte limit"
            );
            let agent: Box<dyn cururu_engine::ReviewAgent> = if matches!(
                config.llm.provider,
                config::LlmProvider::Anthropic
            ) {
                Box::new(anthropic::AnthropicAgent::new(
                    &config.llm,
                    prepared.review_instructions.into(),
                    Some(
                        serde_json::json!({"preferences":prepared.review_preferences, "evidence":evidence}),
                    ),
                )?)
            } else {
                Box::new(
                    cururu_engine::OpenAiCompatibleAgent::new(
                        cururu_engine::OpenAiCompatibleSettings {
                            base_url: config.llm.base_url.clone(),
                            api_key: config.llm.api_key.clone(),
                            model: config.llm.model.clone(),
                            temperature: config.llm.temperature,
                            max_output_tokens: config.llm.max_output_tokens,
                        },
                        prepared.review_instructions.into(),
                    )?
                    .with_review_preferences(
                        &cururu_engine::review_preferences::ReviewPreferences {
                            language: Some(config.review.language.clone()),
                            tone: Some(config.review.tone.clone()),
                            technical_level: Some(config.review.technical_level.clone()),
                            suggestion_detail: Some(config.review.suggestion_detail.clone()),
                        },
                    )?
                    .with_review_context(&cururu_engine::review_context::ReviewContext {
                        focus: config.review.policy.focus.clone(),
                        files: vec![],
                    })?
                    .with_host_review_evidence(&evidence_text)?,
                )
            };
            eprintln!(
                "Sending {} diff chunk(s) and configured context, prior feedback and analyzer evidence to the configured API. No review will be published.",
                prepared.chunks.len()
            );
            let results = cururu_engine::review_chunks(agent.as_ref(), &prepared.chunks).await?;
            let usage = complete_api_usage(&results);
            let reviews = results
                .into_iter()
                .zip(&prepared.chunks)
                .map(|(result, chunk)| review_input::ChunkReview {
                    chunk_index: chunk.index,
                    review: result.review,
                })
                .collect();
            let (_, mut current) = prepare_api_review(*pr).await?;
            if let (Some(sarif), Some(manifest)) = (sarif, analysis_manifest) {
                review_input::attach_local_analysis(&mut current, sarif, manifest)?;
            }
            let result =
                review_input::assemble_review(&current, &prepared.snapshot_sha256, reviews)?;
            let receipt = AgentReviewReceipt {
                source: current.source,
                revision: &current.revision,
                diff_sha256: &current.diff_sha256,
                snapshot_sha256: &current.snapshot_sha256,
                scope: current.scope,
                ignored_paths: &current.ignored_paths,
                review: &result,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&ApiReviewReceipt { receipt, usage })?
            );
            return Ok(());
        }
        Command::AgentReview { .. } => anyhow::bail!(
            "agent-review is retired and sends nothing. Use api-review with your configured API key, or invoke Cururu's MCP tools from your agent."
        ),
        Command::Mcp => {
            mcp::serve().await?;
            return Ok(());
        }
        Command::CheckMcp {
            client,
            configuration,
            executable,
        } => {
            use std::io::Read as _;
            let file = std::fs::File::open(configuration).context("cannot open configuration")?;
            anyhow::ensure!(
                file.metadata()?.is_file(),
                "configuration must be a regular file"
            );
            let mut input = String::new();
            file.take(1_048_577)
                .read_to_string(&mut input)
                .context("cannot read configuration as UTF-8")?;
            anyhow::ensure!(input.len() <= 1_048_576, "configuration exceeds size limit");
            mcp_install::check_entry(*client, &input, executable)?;
            println!(
                "Command fields match. This does not verify other settings, client connection, or trust."
            );
            return Ok(());
        }
        Command::InstallMcp { client, executable } => {
            let path = mcp_install::install(std::path::Path::new("."), *client, executable)?;
            println!(
                "Created {}. Check client trust, working directory and model disclosure before use.",
                path.display()
            );
            return Ok(());
        }
        Command::UpdateCodexMcp {
            configuration,
            executable,
        } => {
            mcp_install::update_codex(configuration, executable)?;
            println!(
                "Added the Cururu MCP server to {}. Review client trust and disclosure before use.",
                configuration.display()
            );
            return Ok(());
        }
        Command::WriteMcpMerge {
            client,
            configuration,
            destination,
            executable,
        } => {
            mcp_install::write_merge_proposal(*client, configuration, destination, executable)?;
            println!(
                "Created {}. The original is unchanged; review the proposal before applying it.",
                destination.display()
            );
            return Ok(());
        }
        Command::PreviewMcpMerge {
            client,
            configuration,
            executable,
        } => {
            let input = std::fs::read_to_string(configuration)
                .context("failed to read client configuration")?;
            let preview = mcp_install::merge_preview(*client, &input, executable)?;
            eprintln!("Preview only; original unchanged. Review before applying.");
            println!("{preview}");
            return Ok(());
        }
        Command::ApproveHostedReview { pr, environment } => {
            return hosted_approval::approve(*pr, environment).await;
        }
        Command::PublishAgentReview {
            receipt,
            sarif,
            analysis_manifest,
        } => {
            use std::io::IsTerminal;
            anyhow::ensure!(
                std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
                "publication requires an interactive terminal; no comment was published"
            );
            let receipt = agent_publication::read_receipt(receipt)?;
            let mut config = AppConfig::from_env()?;
            let scm = github::GitHubClient::from_scm_config(&config.source_control)?;
            merge_repository_config(&mut config, &scm).await?;
            let evidence =
                sarif
                    .as_ref()
                    .zip(analysis_manifest.as_ref())
                    .map(|(sarif, manifest)| agent_publication::LocalEvidence {
                        sarif: sarif.clone(),
                        manifest: manifest.clone(),
                    });
            let preview =
                agent_publication::preview(&config, &scm, &receipt, evidence.as_ref()).await?;
            eprintln!(
                "Target: {}\n\n{}\n",
                agent_publication::target(&config)?,
                preview.body
            );
            eprint!("Type PUBLISH to post this exact comment, or anything else to cancel: ");
            std::io::stderr().flush()?;
            let mut confirmation = String::new();
            std::io::stdin().read_line(&mut confirmation)?;
            anyhow::ensure!(
                confirmation.trim_end() == "PUBLISH",
                "publication cancelled"
            );
            let mut current_config = AppConfig::from_env()?;
            merge_repository_config(&mut current_config, &scm).await?;
            agent_publication::publish(&current_config, &scm, &preview).await?;
            println!("Comment submitted. Verify it in the PR conversation.");
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
                        .unwrap_or_else(|| config.source_control.token.clone());
                    if shared_token.is_empty() {
                        anyhow::bail!(
                            "GITHUB_TOKEN or CURURU_SHARED_CONFIG_TOKEN is required to inspect a private or remote shared base"
                        );
                    }
                    let mut shared_github_config = config.source_control.clone();
                    shared_github_config.token = shared_token;
                    let shared_github: Box<dyn scm::ScmProvider> = Box::new(
                        github::GitHubClient::from_scm_config(&shared_github_config)?,
                    );
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
                        "Merged shared configuration from the configured source-control base."
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
    if matches!(cli.command, Command::Review | Command::DryRun) {
        config.require_llm_api_key()?;
    }
    let source_control: Box<dyn scm::ScmProvider> = Box::new(
        github::GitHubClient::from_scm_config(&config.source_control)?,
    );
    if let Some(command) = &issue_comment_command
        && !source_control.user_can_review(&command.login).await?
    {
        return Ok(());
    }
    // Load trusted configuration only for commands that use review settings.
    if matches!(
        cli.command,
        Command::PrintConfig | Command::Review | Command::DryRun | Command::PublishApprovedReview
    ) {
        let loaded = merge_repository_config(&mut config, source_control.as_ref()).await?;
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
        Command::Init
        | Command::Setup { .. }
        | Command::Status
        | Command::PrepareReview { .. }
        | Command::AgentReview { .. }
        | Command::ApiReview { .. }
        | Command::PublishAgentReview { .. }
        | Command::ApproveHostedReview { .. }
        | Command::InstallMcp { .. }
        | Command::PreviewMcpMerge { .. }
        | Command::UpdateCodexMcp { .. }
        | Command::WriteMcpMerge { .. }
        | Command::CheckMcp { .. }
        | Command::UpdateJsoncMcp { .. }
        | Command::Mcp => {
            unreachable!("handled before loading runtime configuration")
        }
        Command::PrintDiff => {
            let (diff, truncated) = source_control.fetch_diff_with_limit(usize::MAX).await?;
            anyhow::ensure!(
                !truncated,
                "change-request diff exceeded the maximum supported output size"
            );
            println!("{diff}");
        }
        Command::PrintConfig => {}
        Command::DryRun => {
            let result = review::run_review(&config, source_control.as_ref()).await?;
            let report = quality::evaluate(&result.review, config.review.policy.fail_on);
            write_action_outputs(&report)?;
            write_analysis_outputs(&result.analysis)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&DryRunOutput {
                    review: &result.review,
                    evaluation: &result.evaluation,
                })?
            );
        }
        Command::Review => {
            let expected_head_sha = std::env::var("EXPECTED_HEAD_SHA")
                .ok()
                .filter(|sha| !sha.trim().is_empty());
            if let Some(expected) = &expected_head_sha {
                review::ensure_review_head_unchanged(
                    expected,
                    &source_control.fetch_head_sha().await?,
                )?;
            }
            if config.review.policy.incremental {
                let head_sha = source_control.fetch_head_sha().await?;
                if source_control.has_summary_for_revision(&head_sha).await? {
                    println!("Cururu: review already exists for the current revision");
                    return Ok(());
                }
            }
            let mut result = match review::run_review(&config, source_control.as_ref()).await {
                Ok(result) => result,
                Err(error) => {
                    if review::publish_diff_limit_notice(&config, source_control.as_ref(), &error)
                        .await?
                    {
                        if let Some(path) = std::env::var_os("GITHUB_OUTPUT") {
                            let mut file = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(path)?;
                            writeln!(file, "review_status=skipped_diff_limit")?;
                            writeln!(file, "quality_gate=not_run")?;
                            writeln!(file, "quality_gate_passed=false")?;
                        }
                        write_auto_approval_status(
                            &config,
                            if config.auto_approval.enabled {
                                "not_evaluated"
                            } else {
                                "disabled"
                            },
                            "[]",
                        )?;
                        println!(
                            "Cururu: review not performed because the diff exceeded the configured limit"
                        );
                        return Ok(());
                    }
                    if let Some(failure) = error.downcast_ref::<agent::InvalidReviewOutput>() {
                        report_incomplete_review(
                            &config,
                            source_control.as_ref(),
                            &failure.finish_reason,
                        )
                        .await?;
                    }
                    return Err(error);
                }
            };
            if let Some(expected) = &expected_head_sha {
                review::ensure_review_head_unchanged(expected, &result.head_sha)?;
            }
            let report = quality::evaluate(&result.review, config.review.policy.fail_on);
            write_action_outputs(&report)?;
            write_analysis_outputs(&result.analysis)?;
            result.attach_pr_usage(source_control.as_ref()).await;
            let route =
                auto_approval::decide(&config, source_control.as_ref(), &mut result).await?;
            let inline = matches!(config.review.comment_mode, CommentMode::Inline);
            let candidate = hosted_publication::HostedCandidate::from_review(
                &config,
                &result,
                if inline {
                    build_inline_drafts(&result)
                } else {
                    Vec::new()
                },
                inline,
                false,
            );
            if route == auto_approval::Route::Approved {
                // Publish right after approving: any failure from here on
                // revokes the approval. The opt-in policy replaces the
                // per-candidate publication approval.
                auto_approval::publish_approved(&candidate, source_control.as_ref(), &result)
                    .await?;
                write_auto_approval_outputs(&config, &result)?;
                append_github_output("review_status=completed\n")?;
                println!(
                    "Cururu: PR auto-approved and review published at {}",
                    result.head_sha
                );
            } else {
                write_auto_approval_outputs(&config, &result)?;
                review::ensure_review_head_unchanged(
                    &result.head_sha,
                    &source_control.fetch_head_sha().await?,
                )?;
                action_publication::prepare(&candidate, source_control.as_ref()).await?;
                println!(
                    "Cururu: candidate {} prepared; nothing was published. Approve the publication job to publish it.",
                    candidate.digest()
                );
            }
            if !report.passed {
                anyhow::bail!(
                    "quality gate failed: {} finding(s) at or above configured threshold",
                    report.findings_count
                );
            }
        }
        Command::PublishApprovedReview => {
            let github = github::GitHubClient::from_scm_config(&config.source_control)?;
            action_publication::publish_approved(&config, &github).await?;
        }
    }

    Ok(())
}

async fn prepare_api_review(pr: bool) -> anyhow::Result<(AppConfig, review_input::PreparedReview)> {
    if pr {
        let mut config = AppConfig::from_env()?;
        let scm = github::GitHubClient::from_scm_config(&config.source_control)?;
        merge_repository_config(&mut config, &scm).await?;
        let prepared = review_input::prepare_agent_pr(&config, &scm).await?;
        Ok((config, prepared))
    } else {
        let config = local_review_config()?;
        let prepared = review_input::prepare_local(&config, std::path::Path::new("."))?;
        Ok((config, prepared))
    }
}

async fn prepare_review_input(local: bool) -> anyhow::Result<review_input::PreparedReview> {
    if local {
        let config = local_review_config()?;
        review_input::prepare_local(&config, std::path::Path::new("."))
    } else {
        let mut config = AppConfig::from_env()?;
        let scm = github::GitHubClient::from_scm_config(&config.source_control)?;
        merge_repository_config(&mut config, &scm).await?;
        review_input::prepare_agent_pr(&config, &scm).await
    }
}

fn local_review_config() -> anyhow::Result<AppConfig> {
    let mut config = AppConfig::from_local_env()?;
    match std::fs::read_to_string(".cururu.toml") {
        Ok(raw) => {
            anyhow::ensure!(
                AppConfig::shared_base_from_toml(&raw)?.is_none(),
                "local review does not support shared configuration bases"
            );
            config.merge_toml_str(&raw)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to read local configuration"),
    }
    Ok(config)
}

async fn merge_repository_config(
    config: &mut AppConfig,
    source_control: &dyn scm::ScmProvider,
) -> anyhow::Result<bool> {
    let shared_token = std::env::var("CURURU_SHARED_CONFIG_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    merge_repository_config_with_shared_token(config, source_control, shared_token.as_deref()).await
}

async fn merge_repository_config_with_shared_token(
    config: &mut AppConfig,
    source_control: &dyn scm::ScmProvider,
    shared_token: Option<&str>,
) -> anyhow::Result<bool> {
    let base_sha = source_control
        .fetch_base_sha()
        .await
        .context("failed to resolve trusted PR base")?;
    let Some(local_raw) = source_control.fetch_config_toml(&base_sha).await? else {
        config.trusted_config_snapshot = Some((base_sha, None));
        config.shared_config_snapshot = None;
        return Ok(false);
    };

    let mut validation = config.clone();
    validation.merge_toml_str(&local_raw)?;
    if let Some(shared) = AppConfig::shared_base_from_toml(&local_raw)? {
        let mut shared_github_config = config.source_control.clone();
        if let Some(token) = shared_token {
            shared_github_config.token = token.to_string();
        }
        let shared_github: std::sync::Arc<dyn scm::ScmProvider> = std::sync::Arc::new(
            github::GitHubClient::from_scm_config(&shared_github_config)?,
        );
        let shared_raw = shared_github
            .fetch_repository_file_at_ref(&shared.repository, &shared.path, &shared.commit)
            .await
            .context("failed to load shared config; verify source-control access")?;
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
        config.shared_config_snapshot = Some(config::SharedConfigSnapshot {
            source: shared,
            hash: review::trusted_config_hash(&shared_raw),
            client: shared_github,
        });
    } else {
        config.merge_toml_str(&local_raw)?;
        config.shared_config_snapshot = None;
    }
    config.trusted_config_snapshot =
        Some((base_sha, Some(review::trusted_config_hash(&local_raw))));
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

/// Explain an incomplete model response in the job summary and the PR check,
/// with the setting to change; nothing is commented on the PR.
async fn report_incomplete_review(
    config: &AppConfig,
    source_control: &dyn scm::ScmProvider,
    finish_reason: &str,
) -> anyhow::Result<()> {
    let diagnostic = diagnostics::incomplete_review(config, finish_reason);
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .context("failed to open GITHUB_STEP_SUMMARY")?;
        writeln!(file, "## {}\n\n{}", diagnostic.title, diagnostic.body)?;
    }
    append_github_output("review_status=incomplete\n")?;
    if let Ok(head) = source_control.fetch_head_sha().await
        && let Err(error) = source_control
            .create_candidate_check(&scm::CandidateCheck {
                head_sha: &head,
                external_id: "diagnostic",
                title: diagnostic.title,
                summary: &diagnostic.body,
                text: &diagnostic.body,
                approval_action: false,
            })
            .await
    {
        tracing::warn!(%error, "could not report the incomplete review check");
    }
    Ok(())
}

fn append_github_output(lines: &str) -> anyhow::Result<()> {
    if let Some(path) = std::env::var_os("GITHUB_OUTPUT") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .context("failed to open GITHUB_OUTPUT")?;
        file.write_all(lines.as_bytes())?;
    }
    Ok(())
}

/// `auto_approval`, `auto_approval_reasons` and `pr_url` workflow outputs.
fn write_auto_approval_outputs(
    config: &AppConfig,
    result: &review::ReviewOutput,
) -> anyhow::Result<()> {
    result.auto_approval.as_ref().map_or_else(
        || write_auto_approval_status(config, "disabled", "[]"),
        |decision| {
            write_auto_approval_status(
                config,
                decision.status(),
                &auto_approval::reasons_json(decision, &result.language),
            )
        },
    )
}

/// Always write all three outputs so consuming workflows can rely on them.
fn write_auto_approval_status(
    config: &AppConfig,
    status: &str,
    reasons: &str,
) -> anyhow::Result<()> {
    append_github_output(&format!(
        "auto_approval={status}\nauto_approval_reasons={reasons}\npr_url={}/{}/pull/{}\n",
        config.source_control.web_url.trim_end_matches('/'),
        config.source_control.repository,
        config.source_control.change_request_number
    ))
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
    println!("evaluator.mode: {:?}", config.evaluator.mode);
    println!("evaluator.model: {}", config.evaluator.model);
    println!("temperature: {}", config.llm.temperature);
    println!("max_output_tokens: {}", config.llm.max_output_tokens);
    println!("repository: {}", config.source_control.repository);
    println!(
        "change_request_number: {}",
        config.source_control.change_request_number
    );
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
fn build_inline_drafts(result: &review::ReviewOutput) -> Vec<scm::ReviewCommentDraft> {
    result
        .review
        .findings
        .iter()
        .map(|f| {
            let line = f
                .line
                .filter(|line| diff::is_valid_anchor(&result.changed_files, &f.path, *line));
            scm::ReviewCommentDraft {
                path: f.path.clone(),
                line,
                body: output::render_inline_finding(f),
            }
        })
        .collect()
}

#[cfg(test)]
mod cli_tests {
    use super::{DryRunOutput, initialize_repository_at};

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

    #[test]
    fn dry_run_adds_evaluation_without_nesting_or_removing_review_fields() {
        let review = crate::agent::ReviewResult {
            model: "review-model".into(),
            files_reviewed: 2,
            summary: "summary".into(),
            findings: vec![],
        };
        let evaluation = None;
        let json = serde_json::to_value(DryRunOutput {
            review: &review,
            evaluation: &evaluation,
        })
        .unwrap();

        assert_eq!(json["model"], "review-model");
        assert_eq!(json["files_reviewed"], 2);
        assert!(json["findings"].is_array());
        assert!(json["evaluation"].is_null());
        assert!(json.get("review").is_none());
    }
}
