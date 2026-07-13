use anyhow::{Context, bail};
use clap::ValueEnum;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::env;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub github: GitHubConfig,
    pub llm: LlmConfig,
    pub review: ReviewConfig,
}

#[derive(Debug, Clone)]
pub struct GitHubConfig {
    pub token: String,
    pub repository: String,
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub api_url: String,
    pub server_url: String,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LlmProvider {
    OpenAiCompatible,
    Rig,
}

#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub max_diff_bytes: usize,
    pub chunk_bytes: usize,
    pub summary_only: bool,
    pub fail_on_findings: bool,
    pub ignore: GlobSet,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let repository = env_required("GITHUB_REPOSITORY")?;
        let (owner, repo) = repository
            .split_once('/')
            .context("GITHUB_REPOSITORY must be owner/repo")?;
        let owner = owner.to_string();
        let repo = repo.to_string();

        let pr_number = env::var("PR_NUMBER")
            .or_else(|_| {
                env::var("GITHUB_REF_NAME")
                    .map(|v| v.split('/').next().unwrap_or_default().to_string())
            })
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .context("set PR_NUMBER, e.g. ${{ github.event.pull_request.number }}")?;

        let ignore = build_globs(&env::var("CURURU_IGNORE").unwrap_or_else(|_| {
            "**/Cargo.lock,**/package-lock.json,**/pnpm-lock.yaml,**/yarn.lock,**/dist/**,**/build/**".to_string()
        }))?;

        let provider = match env::var("CURURU_PROVIDER")
            .unwrap_or_else(|_| "openai-compatible".to_string())
            .as_str()
        {
            "openai-compatible" | "openai" | "openrouter" | "groq" => LlmProvider::OpenAiCompatible,
            "rig" => LlmProvider::Rig,
            other => bail!("unsupported CURURU_PROVIDER={other}"),
        };

        Ok(Self {
            github: GitHubConfig {
                token: env_required("GITHUB_TOKEN")?,
                repository,
                owner,
                repo,
                pr_number,
                api_url: env::var("GITHUB_API_URL")
                    .unwrap_or_else(|_| "https://api.github.com".to_string()),
                server_url: env::var("GITHUB_SERVER_URL")
                    .unwrap_or_else(|_| "https://github.com".to_string()),
            },
            llm: LlmConfig {
                provider,
                base_url: env::var("LLM_BASE_URL")
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
                api_key: env_required("LLM_API_KEY")?,
                model: env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-5-mini".to_string()),
                temperature: env_parse("LLM_TEMPERATURE", 0.1)?,
                max_output_tokens: env_parse("LLM_MAX_OUTPUT_TOKENS", 4000)?,
            },
            review: ReviewConfig {
                max_diff_bytes: env_parse("CURURU_MAX_DIFF_BYTES", 180_000)?,
                chunk_bytes: env_parse("CURURU_CHUNK_BYTES", 45_000)?,
                summary_only: env_bool("CURURU_SUMMARY_ONLY", true),
                fail_on_findings: env_bool("CURURU_FAIL_ON_FINDINGS", false),
                ignore,
            },
        })
    }
}

fn env_required(name: &str) -> anyhow::Result<String> {
    env::var(name).with_context(|| format!("missing env var {name}"))
}

fn env_parse<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse::<T>()
            .map_err(|err| anyhow::anyhow!("invalid {name}: {err}")),
        Err(_) => Ok(default),
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn build_globs(csv: &str) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for raw in csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        builder.add(Glob::new(raw)?);
    }
    Ok(builder.build()?)
}
