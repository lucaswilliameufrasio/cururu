use anyhow::{Context, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub github: GitHubConfig,
    pub llm: LlmConfig,
    pub review: ReviewConfig,
    pub context: ContextConfig,
    pub summary: SummaryConfig,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    OpenAI,
    OpenRouter,
    Groq,
}

impl LlmProvider {
    pub const fn default_base_url(&self) -> &str {
        match self {
            Self::OpenAI => "https://api.openai.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Groq => "https://api.groq.com/openai/v1",
        }
    }

    pub const fn default_model(&self) -> &str {
        match self {
            Self::OpenAI => "gpt-5-mini",
            Self::OpenRouter => "openai/gpt-5-mini",
            Self::Groq => "llama-3.3-70b-versatile",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAI),
            "openrouter" => Some(Self::OpenRouter),
            "groq" => Some(Self::Groq),
            _ => None,
        }
    }
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

#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub max_diff_bytes: usize,
    pub chunk_bytes: usize,
    #[allow(dead_code)]
    pub summary_only: bool,
    #[allow(dead_code)]
    pub fail_on_findings: bool,
    pub ignore: GlobSet,
}

#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub conventions: Vec<String>,
    pub specifications: Vec<String>,
    pub skills: Vec<String>,
    pub additional: Vec<String>,
    pub max_bytes: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            conventions: Vec::new(),
            specifications: Vec::new(),
            skills: Vec::new(),
            additional: Vec::new(),
            max_bytes: 100_000,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SummaryConfig {
    pub show_cost: bool,
    pub show_usage: bool,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let repository = env_required("GITHUB_REPOSITORY")?;
        let (owner, repo) = repository
            .split_once('/')
            .context("GITHUB_REPOSITORY must be owner/repo")?;
        let owner = owner.to_string();
        let repo = repo.to_string();

        let pr_number = env_optional("PR_NUMBER")
            .or_else(|| {
                env_optional("GITHUB_REF_NAME").and_then(|v| v.split('/').next().map(String::from))
            })
            .and_then(|v| v.parse::<u64>().ok())
            .context("set PR_NUMBER env var")?;

        let provider_name = env_optional("CURURU_PROVIDER");
        let provider = provider_name
            .as_deref()
            .and_then(LlmProvider::from_name)
            .unwrap_or(LlmProvider::OpenAI);

        let base_url =
            env_optional("LLM_BASE_URL").unwrap_or_else(|| provider.default_base_url().to_string());

        let model =
            env_optional("LLM_MODEL").unwrap_or_else(|| provider.default_model().to_string());

        let ignore_globs = env::var("CURURU_IGNORE").unwrap_or_else(|_| {
            "**/Cargo.lock,**/package-lock.json,**/pnpm-lock.yaml,**/yarn.lock,**/dist/**,**/build/**".to_string()
        });

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
                base_url,
                api_key: env_required("LLM_API_KEY")?,
                model,
                temperature: env_parse("LLM_TEMPERATURE", 0.1)?,
                max_output_tokens: env_parse("LLM_MAX_OUTPUT_TOKENS", 4000)?,
            },
            review: ReviewConfig {
                max_diff_bytes: env_parse("CURURU_MAX_DIFF_BYTES", 180_000)?,
                chunk_bytes: env_parse("CURURU_CHUNK_BYTES", 45_000)?,
                summary_only: env_bool("CURURU_SUMMARY_ONLY", true),
                fail_on_findings: env_bool("CURURU_FAIL_ON_FINDINGS", false),
                ignore: build_globs(&ignore_globs)?,
            },
            context: ContextConfig::default(),
            summary: SummaryConfig::default(),
        })
    }

    pub fn merge_toml_str(&mut self, raw: &str) -> anyhow::Result<()> {
        let de = toml::Deserializer::new(raw);
        let parsed: CururuToml =
            serde_path_to_error::deserialize(de).context("failed to parse .cururu.toml")?;

        if parsed.version != 1 {
            bail!(
                "unsupported .cururu.toml version {} (expected 1)",
                parsed.version
            );
        }

        // TOML provider fields apply only when the corresponding env var is NOT set
        if let Some(tp) = parsed.provider {
            if env_optional("CURURU_PROVIDER").is_none()
                && let Some(name) = tp.name
                && let Some(p) = LlmProvider::from_name(&name)
            {
                self.llm.provider = p;
                if env_optional("LLM_BASE_URL").is_none() {
                    self.llm.base_url = p.default_base_url().to_string();
                }
            }
            if env_optional("LLM_BASE_URL").is_none()
                && let Some(url) = tp.base_url
            {
                self.llm.base_url = url;
            }
            if env_optional("LLM_MODEL").is_none()
                && let Some(model) = tp.model
            {
                self.llm.model = model;
            }
        }

        if let Some(tr) = parsed.review {
            if let Some(v) = tr.max_diff_bytes {
                self.review.max_diff_bytes = v;
            }
            if let Some(v) = tr.chunk_bytes {
                self.review.chunk_bytes = v;
            }
            if let Some(patterns) = tr.ignore {
                self.review.ignore = build_globs(&patterns.join(","))?;
            }
        }

        if let Some(tc) = parsed.context {
            if let Some(v) = tc.conventions {
                self.context.conventions = v;
            }
            if let Some(v) = tc.specifications {
                self.context.specifications = v;
            }
            if let Some(v) = tc.skills {
                self.context.skills = v;
            }
            if let Some(v) = tc.additional {
                self.context.additional = v;
            }
            if let Some(v) = tc.max_bytes {
                self.context.max_bytes = v;
            }
        }

        if let Some(ts) = parsed.summary {
            if let Some(v) = ts.show_cost {
                self.summary.show_cost = v;
            }
            if let Some(v) = ts.show_usage {
                self.summary.show_usage = v;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct CururuToml {
    version: u32,
    #[serde(default)]
    provider: Option<ProviderToml>,
    #[serde(default)]
    review: Option<ReviewToml>,
    #[serde(default)]
    context: Option<ContextToml>,
    #[serde(default)]
    summary: Option<SummaryToml>,
}

#[derive(Debug, Deserialize)]
struct ProviderToml {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewToml {
    #[serde(default)]
    max_diff_bytes: Option<usize>,
    #[serde(default)]
    chunk_bytes: Option<usize>,
    #[serde(default)]
    ignore: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ContextToml {
    #[serde(default)]
    conventions: Option<Vec<String>>,
    #[serde(default)]
    specifications: Option<Vec<String>>,
    #[serde(default)]
    skills: Option<Vec<String>>,
    #[serde(default)]
    additional: Option<Vec<String>>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SummaryToml {
    #[serde(default)]
    show_cost: Option<bool>,
    #[serde(default)]
    show_usage: Option<bool>,
}

fn env_required(name: &str) -> anyhow::Result<String> {
    let val = env::var(name).map_err(|_| anyhow::anyhow!("missing env var {name}"))?;
    if val.is_empty() {
        anyhow::bail!("env var {name} is set but empty");
    }
    Ok(val)
}

fn env_optional(name: &str) -> Option<String> {
    let val = env::var(name).ok()?;
    if val.is_empty() { None } else { Some(val) }
}

fn env_parse<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    env_optional(name).map_or_else(
        || Ok(default),
        |value| {
            value
                .parse::<T>()
                .map_err(|err| anyhow::anyhow!("invalid {name}: {err}"))
        },
    )
}

fn env_bool(name: &str, default: bool) -> bool {
    env_optional(name).map_or(default, |v| {
        matches!(v.as_str(), "1" | "true" | "yes" | "on")
    })
}

fn build_globs(csv: &str) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for raw in csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        builder.add(Glob::new(raw)?);
    }
    Ok(builder.build()?)
}
