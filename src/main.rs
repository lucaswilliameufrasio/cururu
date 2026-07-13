mod agent;
mod config;
mod context;
mod diff;
mod github;
mod output;
mod provider;
mod retry;
mod review;

use anyhow::Context;
use clap::{Parser, Subcommand};
use config::AppConfig;
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
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let mut config = AppConfig::from_env().context("failed to load configuration")?;
    let github = github::GitHubClient::new(&config.github)?;

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
            println!("{config:#?}");
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

    match cli.command {
        Command::PrintDiff => {
            let diff = github.fetch_pr_diff().await?;
            println!("{diff}");
        }
        Command::PrintConfig => {}
        Command::DryRun => {
            let result = review::run_review(&config, &github).await?;
            println!("{}", serde_json::to_string_pretty(&result.review)?);
        }
        Command::Review => {
            let result = review::run_review(&config, &github).await?;
            let body = output::render_summary_comment(&result);
            github.upsert_summary_comment(&body).await?;
            println!("{}", serde_json::to_string_pretty(&result.review)?);
        }
    }

    Ok(())
}
