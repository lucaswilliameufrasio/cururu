mod agent;
mod config;
mod diff;
mod github;
mod output;
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
    /// Review the current pull request and post a GitHub summary comment.
    Review,
    /// Review the PR but only print the JSON result. Does not write to GitHub.
    DryRun,
    /// Fetch and print the PR diff.
    PrintDiff,
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
    let config = AppConfig::from_env().context("failed to load configuration")?;
    let github = github::GitHubClient::new(&config.github)?;

    match cli.command {
        Command::PrintDiff => {
            let diff = github.fetch_pr_diff().await?;
            println!("{diff}");
        }
        Command::DryRun => {
            let result = review::run_review(&config, &github).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Review => {
            let result = review::run_review(&config, &github).await?;
            let body = output::render_summary_comment(&result);
            github.upsert_summary_comment(&body).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}
