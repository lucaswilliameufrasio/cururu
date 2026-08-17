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
            Self::OpenAI => "gpt-5.6-luna",
            Self::OpenRouter => "openai/gpt-5.6-luna",
            Self::Groq => "openai/gpt-oss-120b",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentMode {
    Inline,
    Summary,
}

impl CommentMode {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "inline" => Some(Self::Inline),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub max_diff_bytes: usize,
    pub chunk_bytes: usize,
    pub ignore: GlobSet,
    pub language: String,
    pub comment_mode: CommentMode,
    pub policy: ReviewPolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewPolicy {
    pub profile: String,
    pub minimum_confidence: f32,
    pub max_findings: usize,
    pub fail_on: FailOn,
    pub allowed_severities: Vec<Severity>,
    pub suggested_changes: bool,
    pub incremental: bool,
    pub synthesis: bool,
    pub focus: Vec<String>,
}

impl Default for ReviewPolicy {
    fn default() -> Self {
        Self {
            profile: "balanced".into(),
            minimum_confidence: 0.65,
            max_findings: 30,
            fail_on: FailOn::Off,
            allowed_severities: Severity::all(),
            suggested_changes: false,
            incremental: false,
            synthesis: false,
            focus: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    Off,
    Critical,
    High,
    Medium,
    Low,
}

impl FailOn {
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(Self::Off),
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    pub const fn rank(self) -> u8 {
        match self {
            Self::Off => u8::MAX,
            Self::Critical => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    pub const fn rank(self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
        }
    }

    pub fn all() -> Vec<Self> {
        vec![Self::Critical, Self::High, Self::Medium, Self::Low]
    }
}

fn validate_confidence(value: f32) -> anyhow::Result<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        bail!("policy.minimum_confidence must be finite and between 0 and 1")
    }
}

fn profile_defaults(name: &str) -> anyhow::Result<ReviewPolicy> {
    let mut policy = ReviewPolicy {
        profile: name.to_string(),
        ..ReviewPolicy::default()
    };
    match name.trim().to_ascii_lowercase().as_str() {
        "balanced" => {}
        "strict" => {
            policy.minimum_confidence = 0.8;
            policy.max_findings = 50;
            policy.fail_on = FailOn::High;
        }
        "security" => {
            policy.minimum_confidence = 0.75;
            policy.max_findings = 40;
            policy.fail_on = FailOn::High;
            policy.allowed_severities = vec![Severity::Critical, Severity::High, Severity::Medium];
            policy.focus = vec!["security".into()];
        }
        "minimal" => {
            policy.minimum_confidence = 0.8;
            policy.max_findings = 10;
            policy.allowed_severities = vec![Severity::Critical, Severity::High];
        }
        _ => bail!("invalid review.profile: {name}"),
    }
    Ok(policy)
}

#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub conventions: Vec<String>,
    pub specifications: Vec<String>,
    pub skills: Vec<String>,
    pub additional: Vec<String>,
    pub max_bytes: usize,
    pub auto: AutoContextConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoContextConfig {
    pub enabled: bool,
    pub max_bytes: usize,
    pub max_files: usize,
    pub per_file_bytes: usize,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for AutoContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bytes: 50_000,
            max_files: 20,
            per_file_bytes: 12_000,
            include: vec!["src/**".into(), "tests/**".into()],
            exclude: vec!["**/generated/**".into(), "**/*.min.js".into()],
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            conventions: Vec::new(),
            specifications: Vec::new(),
            skills: Vec::new(),
            additional: Vec::new(),
            max_bytes: 100_000,
            auto: AutoContextConfig::default(),
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
            .unwrap_or(LlmProvider::OpenRouter);

        let base_url =
            env_optional("LLM_BASE_URL").unwrap_or_else(|| provider.default_base_url().to_string());

        let model =
            env_optional("LLM_MODEL").unwrap_or_else(|| provider.default_model().to_string());

        let ignore_globs = env::var("CURURU_IGNORE").unwrap_or_else(|_| {
            "**/Cargo.lock,**/package-lock.json,**/pnpm-lock.yaml,**/yarn.lock,**/dist/**,**/build/**".to_string()
        });

        let mut policy = env_optional("CURURU_PROFILE").map_or_else(
            || Ok(ReviewPolicy::default()),
            |value| profile_defaults(&value),
        )?;
        if let Some(value) = env_optional("CURURU_FAIL_ON") {
            policy.fail_on = FailOn::from_name(&value)
                .with_context(|| format!("invalid CURURU_FAIL_ON: {value}"))?;
        }

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
                ignore: build_globs(&ignore_globs)?,
                language: env_optional("CURURU_LANGUAGE").unwrap_or_else(|| "pt-BR".into()),
                comment_mode: CommentMode::Inline,
                policy,
            },
            context: ContextConfig::default(),
            summary: SummaryConfig::default(),
        })
    }

    #[allow(clippy::too_many_lines)]
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
                if env_optional("LLM_MODEL").is_none() {
                    self.llm.model = p.default_model().to_string();
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
            if env_optional("LLM_TEMPERATURE").is_none()
                && let Some(temperature) = tp.temperature
            {
                self.llm.temperature = temperature;
            }
            if env_optional("LLM_MAX_OUTPUT_TOKENS").is_none()
                && let Some(max_output_tokens) = tp.max_output_tokens
            {
                self.llm.max_output_tokens = max_output_tokens;
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
            if env_optional("CURURU_LANGUAGE").is_none()
                && let Some(lang) = tr.language
            {
                self.review.language = lang;
            }
            if let Some(mode) = tr.comment_mode
                && let Some(m) = CommentMode::from_name(&mode)
            {
                self.review.comment_mode = m;
            }

            if env_optional("CURURU_PROFILE").is_none()
                && let Some(profile) = tr.profile
            {
                self.review.policy = profile_defaults(&profile)?;
            }
        }

        if let Some(tp) = parsed.policy {
            if let Some(v) = tp.minimum_confidence {
                validate_confidence(v)?;
                self.review.policy.minimum_confidence = v;
            }
            if let Some(v) = tp.max_findings {
                self.review.policy.max_findings = v;
            }
            if env_optional("CURURU_FAIL_ON").is_none()
                && let Some(v) = tp.fail_on
            {
                self.review.policy.fail_on = FailOn::from_name(&v)
                    .with_context(|| format!("invalid policy.fail_on: {v}"))?;
            }
            if let Some(values) = tp.allowed_severities {
                let mut parsed = Vec::with_capacity(values.len());
                for value in values {
                    parsed.push(
                        Severity::from_name(&value).with_context(|| {
                            format!("invalid policy.allowed_severities: {value}")
                        })?,
                    );
                }
                self.review.policy.allowed_severities = parsed;
            }
            if let Some(v) = tp.suggested_changes {
                self.review.policy.suggested_changes = v;
            }
            if let Some(v) = tp.incremental {
                self.review.policy.incremental = v;
            }
            if let Some(v) = tp.synthesis {
                self.review.policy.synthesis = v;
            }
            if let Some(v) = tp.focus {
                self.review.policy.focus = v;
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
            if let Some(auto) = tc.auto {
                if let Some(v) = auto.enabled {
                    self.context.auto.enabled = v;
                }
                if let Some(v) = auto.max_bytes {
                    self.context.auto.max_bytes = v;
                }
                if let Some(v) = auto.max_files {
                    self.context.auto.max_files = v;
                }
                if let Some(v) = auto.per_file_bytes {
                    self.context.auto.per_file_bytes = v;
                }
                if let Some(v) = auto.include {
                    self.context.auto.include = v;
                }
                if let Some(v) = auto.exclude {
                    self.context.auto.exclude = v;
                }
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
    policy: Option<PolicyToml>,
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
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ReviewToml {
    #[serde(default)]
    max_diff_bytes: Option<usize>,
    #[serde(default)]
    chunk_bytes: Option<usize>,
    #[serde(default)]
    ignore: Option<Vec<String>>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    comment_mode: Option<String>,
    #[serde(default)]
    profile: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PolicyToml {
    #[serde(default)]
    minimum_confidence: Option<f32>,
    #[serde(default)]
    max_findings: Option<usize>,
    #[serde(default)]
    fail_on: Option<String>,
    #[serde(default)]
    allowed_severities: Option<Vec<String>>,
    #[serde(default)]
    suggested_changes: Option<bool>,
    #[serde(default)]
    incremental: Option<bool>,
    #[serde(default)]
    synthesis: Option<bool>,
    #[serde(default)]
    focus: Option<Vec<String>>,
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
    #[serde(default)]
    auto: Option<AutoContextToml>,
}

#[derive(Debug, Deserialize)]
struct AutoContextToml {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    max_files: Option<usize>,
    #[serde(default)]
    per_file_bytes: Option<usize>,
    #[serde(default)]
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
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

fn build_globs(csv: &str) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for raw in csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        builder.add(Glob::new(raw)?);
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn base_config() -> AppConfig {
        AppConfig {
            github: GitHubConfig {
                token: "test-token".into(),
                repository: "owner/repo".into(),
                owner: "owner".into(),
                repo: "repo".into(),
                pr_number: 42,
                api_url: "https://api.github.com".into(),
                server_url: "https://github.com".into(),
            },
            llm: LlmConfig {
                provider: LlmProvider::OpenRouter,
                base_url: "https://openrouter.ai/api/v1".into(),
                api_key: "sk-test".into(),
                model: "openai/gpt-5.6-luna".into(),
                temperature: 0.1,
                max_output_tokens: 4000,
            },
            review: ReviewConfig {
                max_diff_bytes: 180_000,
                chunk_bytes: 45_000,
                ignore: GlobSetBuilder::new().build().unwrap(),
                language: "pt-BR".into(),
                comment_mode: CommentMode::Inline,
                policy: ReviewPolicy::default(),
            },
            context: ContextConfig::default(),
            summary: SummaryConfig::default(),
        }
    }

    #[test]
    fn provider_from_name() {
        assert_eq!(LlmProvider::from_name("openai"), Some(LlmProvider::OpenAI));
        assert_eq!(LlmProvider::from_name("OpenAI"), Some(LlmProvider::OpenAI));
        assert_eq!(LlmProvider::from_name("OPENAI"), Some(LlmProvider::OpenAI));
        assert_eq!(
            LlmProvider::from_name("openrouter"),
            Some(LlmProvider::OpenRouter)
        );
        assert_eq!(
            LlmProvider::from_name("OpenRouter"),
            Some(LlmProvider::OpenRouter)
        );
        assert_eq!(LlmProvider::from_name("groq"), Some(LlmProvider::Groq));
        assert_eq!(LlmProvider::from_name("GROQ"), Some(LlmProvider::Groq));
        assert_eq!(LlmProvider::from_name("invalid"), None);
        assert_eq!(LlmProvider::from_name(""), None);
    }

    #[test]
    fn provider_defaults() {
        assert_eq!(
            LlmProvider::OpenAI.default_base_url(),
            "https://api.openai.com/v1"
        );
        assert_eq!(LlmProvider::OpenAI.default_model(), "gpt-5.6-luna");
        assert_eq!(
            LlmProvider::OpenRouter.default_base_url(),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            LlmProvider::OpenRouter.default_model(),
            "openai/gpt-5.6-luna"
        );
        assert_eq!(
            LlmProvider::Groq.default_base_url(),
            "https://api.groq.com/openai/v1"
        );
        assert_eq!(LlmProvider::Groq.default_model(), "openai/gpt-oss-120b");
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut cfg = base_config();
        let err = cfg.merge_toml_str("version = 2\n").unwrap_err();
        assert!(
            err.to_string().contains("unsupported"),
            "expected unsupported version error"
        );
    }

    #[test]
    fn accepts_minimal_toml() {
        let mut cfg = base_config();
        cfg.merge_toml_str("version = 1\n").unwrap();
        assert_eq!(cfg.llm.provider, LlmProvider::OpenRouter);
        assert!(!cfg.summary.show_cost);
    }

    #[test]
    fn overrides_provider() {
        let _guard = ENV_LOCK.lock().unwrap();
        let mut cfg = base_config();
        cfg.merge_toml_str("version = 1\n[provider]\nname = \"groq\"\nmodel = \"mixtral-8x7b\"\n")
            .unwrap();
        assert_eq!(cfg.llm.provider, LlmProvider::Groq);
        assert_eq!(cfg.llm.model, "mixtral-8x7b");
        assert_eq!(cfg.llm.base_url, "https://api.groq.com/openai/v1");
    }

    #[test]
    fn provider_change_updates_default_model() {
        let _guard = ENV_LOCK.lock().unwrap();
        let mut cfg = base_config();
        assert_eq!(cfg.llm.model, "openai/gpt-5.6-luna");
        cfg.merge_toml_str("version = 1\n[provider]\nname = \"groq\"\n")
            .unwrap();
        assert_eq!(cfg.llm.provider, LlmProvider::Groq);
        assert_eq!(cfg.llm.model, "openai/gpt-oss-120b");
        assert_eq!(cfg.llm.base_url, "https://api.groq.com/openai/v1");
    }

    #[test]
    fn toml_overrides_generation_parameters() {
        let mut cfg = base_config();
        cfg.merge_toml_str(
            "version = 1\n[provider]\ntemperature = 0.3\nmax_output_tokens = 8192\n",
        )
        .unwrap();
        assert!((cfg.llm.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(cfg.llm.max_output_tokens, 8192);
    }

    #[test]
    fn policy_profile_and_overrides_are_merged() {
        let mut cfg = base_config();
        cfg.merge_toml_str(
            "version = 1\n[review]\nprofile = \"security\"\n[policy]\nmax_findings = 7\nsuggested_changes = true\n",
        )
        .unwrap();
        assert_eq!(cfg.review.policy.profile, "security");
        assert!((cfg.review.policy.minimum_confidence - 0.75).abs() < f32::EPSILON);
        assert_eq!(cfg.review.policy.max_findings, 7);
        assert_eq!(cfg.review.policy.fail_on, FailOn::High);
        assert!(cfg.review.policy.suggested_changes);
    }

    #[test]
    fn invalid_policy_values_are_rejected() {
        let mut cfg = base_config();
        assert!(
            cfg.merge_toml_str("version = 1\n[policy]\nminimum_confidence = 2.0\n")
                .is_err()
        );
        assert!(
            cfg.merge_toml_str("version = 1\n[policy]\nfail_on = \"urgent\"\n")
                .is_err()
        );
    }

    #[test]
    fn env_overrides_toml_generation_parameters() {
        let _guard = ENV_LOCK.lock().unwrap();
        temp_env::with_var("LLM_TEMPERATURE", Some("0.7"), || {
            temp_env::with_var("LLM_MAX_OUTPUT_TOKENS", Some("2048"), || {
                let mut cfg = base_config();
                cfg.llm.temperature = 0.7;
                cfg.llm.max_output_tokens = 2048;
                cfg.merge_toml_str(
                    "version = 1\n[provider]\ntemperature = 0.3\nmax_output_tokens = 8192\n",
                )
                .unwrap();
                assert!((cfg.llm.temperature - 0.7).abs() < f32::EPSILON);
                assert_eq!(cfg.llm.max_output_tokens, 2048);
            });
        });
    }

    #[test]
    fn env_var_overrides_toml_provider() {
        let _guard = ENV_LOCK.lock().unwrap();
        temp_env::with_var("CURURU_PROVIDER", Some("openrouter"), || {
            temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
                temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                    temp_env::with_var("PR_NUMBER", Some("1"), || {
                        temp_env::with_var("LLM_API_KEY", Some("key"), || {
                            let mut cfg = AppConfig::from_env().unwrap();
                            cfg.merge_toml_str("version = 1\n[provider]\nname = \"groq\"\n")
                                .unwrap();
                            assert_eq!(cfg.llm.provider, LlmProvider::OpenRouter);
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn env_var_overrides_toml_base_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        temp_env::with_var("LLM_BASE_URL", Some("https://custom.example.com"), || {
            temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
                temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                    temp_env::with_var("PR_NUMBER", Some("1"), || {
                        temp_env::with_var("LLM_API_KEY", Some("key"), || {
                            let mut cfg = AppConfig::from_env().unwrap();
                            cfg.merge_toml_str(
                                "version = 1\n[provider]\nbase_url = \"https://ignored.com\"\n",
                            )
                            .unwrap();
                            assert_eq!(cfg.llm.base_url, "https://custom.example.com");
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn env_var_overrides_toml_model() {
        let _guard = ENV_LOCK.lock().unwrap();
        temp_env::with_var("LLM_MODEL", Some("custom-model"), || {
            temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
                temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                    temp_env::with_var("PR_NUMBER", Some("1"), || {
                        temp_env::with_var("LLM_API_KEY", Some("key"), || {
                            let mut cfg = AppConfig::from_env().unwrap();
                            cfg.merge_toml_str("version = 1\n[provider]\nmodel = \"ignored\"\n")
                                .unwrap();
                            assert_eq!(cfg.llm.model, "custom-model");
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn sets_review_config() {
        let mut cfg = base_config();
        cfg.merge_toml_str(
            "version = 1\n[review]\nmax_diff_bytes = 9999\nchunk_bytes = 1111\nignore = [\"*.lock\"]\n",
        )
        .unwrap();
        assert_eq!(cfg.review.max_diff_bytes, 9999);
        assert_eq!(cfg.review.chunk_bytes, 1111);
    }

    #[test]
    fn comment_mode_defaults_to_inline() {
        let cfg = base_config();
        assert_eq!(cfg.review.comment_mode, CommentMode::Inline);
    }

    #[test]
    fn toml_overrides_comment_mode() {
        let mut cfg = base_config();
        cfg.merge_toml_str("version = 1\n[review]\ncomment_mode = \"summary\"\n")
            .unwrap();
        assert_eq!(cfg.review.comment_mode, CommentMode::Summary);

        let mut cfg = base_config();
        cfg.merge_toml_str("version = 1\n[review]\ncomment_mode = \"inline\"\n")
            .unwrap();
        assert_eq!(cfg.review.comment_mode, CommentMode::Inline);
    }

    #[test]
    fn comment_mode_from_name() {
        assert_eq!(CommentMode::from_name("inline"), Some(CommentMode::Inline));
        assert_eq!(
            CommentMode::from_name("SUMMARY"),
            Some(CommentMode::Summary)
        );
        assert_eq!(
            CommentMode::from_name("  summary  "),
            Some(CommentMode::Summary)
        );
        assert_eq!(CommentMode::from_name("nope"), None);
    }

    #[test]
    fn sets_context_files() {
        let mut cfg = base_config();
        cfg.merge_toml_str(
            r#"
            version = 1
            [context]
            conventions = ["AGENTS.md"]
            specifications = ["docs/sdd/**/*.md"]
            skills = [".agents/skills/**/SKILL.md"]
            additional = ["docs/adr/**/*.md"]
            max_bytes = 50000
            "#,
        )
        .unwrap();
        assert_eq!(cfg.context.conventions, vec!["AGENTS.md"]);
        assert_eq!(cfg.context.max_bytes, 50000);
    }

    #[test]
    fn sets_summary_flags() {
        let mut cfg = base_config();
        cfg.merge_toml_str("version = 1\n[summary]\nshow_cost = true\nshow_usage = true\n")
            .unwrap();
        assert!(cfg.summary.show_cost);
        assert!(cfg.summary.show_usage);
    }

    #[test]
    fn partial_toml_does_not_reset_unset_fields() {
        let mut cfg = base_config();
        cfg.context.max_bytes = 777;
        cfg.merge_toml_str("version = 1\n[context]\nconventions = [\"CONVENTIONS.md\"]\n")
            .unwrap();
        assert_eq!(cfg.context.conventions, vec!["CONVENTIONS.md"]);
        assert_eq!(cfg.context.max_bytes, 777);
    }

    #[test]
    fn language_default_is_pt_br() {
        let cfg = base_config();
        assert_eq!(cfg.review.language, "pt-BR");
    }

    #[test]
    fn toml_overrides_language() {
        let mut cfg = base_config();
        cfg.merge_toml_str("version = 1\n[review]\nlanguage = \"en-US\"\n")
            .unwrap();
        assert_eq!(cfg.review.language, "en-US");
    }

    #[test]
    fn build_globs_empty() {
        let set = build_globs("").unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn build_globs_multiple() {
        let set = build_globs("*.rs,*.toml").unwrap();
        assert!(set.is_match("main.rs"));
        assert!(set.is_match("Cargo.toml"));
        assert!(!set.is_match("README.md"));
    }

    #[test]
    fn env_required_ok() {
        temp_env::with_var("TEST_ENV_REQUIRED", Some("value"), || {
            assert_eq!(env_required("TEST_ENV_REQUIRED").unwrap(), "value");
        });
    }

    #[test]
    fn env_required_missing() {
        temp_env::with_var("TEST_ENV_UNSET", None::<&str>, || {
            assert!(env_required("TEST_ENV_UNSET").is_err());
        });
    }

    #[test]
    fn env_required_empty() {
        temp_env::with_var("TEST_ENV_EMPTY", Some(""), || {
            assert!(env_required("TEST_ENV_EMPTY").is_err());
        });
    }

    #[test]
    fn env_optional_returns_none_for_empty() {
        temp_env::with_var("TEST_OPT_EMPTY", Some(""), || {
            assert_eq!(env_optional("TEST_OPT_EMPTY"), None);
        });
    }

    #[test]
    fn env_optional_returns_value() {
        temp_env::with_var("TEST_OPT_VAL", Some("hello"), || {
            assert_eq!(env_optional("TEST_OPT_VAL"), Some("hello".into()));
        });
    }

    #[test]
    fn env_parse_invalid_returns_error() {
        temp_env::with_var("TEST_PARSE", Some("not-a-number"), || {
            assert!(env_parse::<u32>("TEST_PARSE", 0).is_err());
        });
    }

    #[test]
    fn env_parse_valid() {
        temp_env::with_var("TEST_PARSE_VALID", Some("42"), || {
            assert_eq!(env_parse::<u32>("TEST_PARSE_VALID", 0).unwrap(), 42);
        });
    }

    #[test]
    fn env_parse_missing_uses_default() {
        temp_env::with_var("TEST_PARSE_MISSING", None::<&str>, || {
            assert_eq!(env_parse::<u32>("TEST_PARSE_MISSING", 99).unwrap(), 99);
        });
    }

    #[test]
    fn provider_name_changes_default_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        temp_env::with_var("CURURU_PROVIDER", Some("groq"), || {
            temp_env::with_var("LLM_BASE_URL", None::<&str>, || {
                temp_env::with_var("LLM_API_KEY", Some("key"), || {
                    temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
                        temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                            temp_env::with_var("PR_NUMBER", Some("1"), || {
                                let cfg = AppConfig::from_env().unwrap();
                                assert_eq!(cfg.llm.provider, LlmProvider::Groq);
                                assert_eq!(cfg.llm.base_url, "https://api.groq.com/openai/v1");
                                assert_eq!(cfg.llm.model, "openai/gpt-oss-120b");
                            });
                        });
                    });
                });
            });
        });
    }
}
