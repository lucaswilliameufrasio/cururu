mod analysis;
mod context;
mod env;
mod github;
mod provider;
mod review;
mod schema;
mod summary;

#[cfg(test)]
mod tests;

pub use analysis::AnalysisConfig;
pub use context::ContextConfig;
pub use github::GitHubConfig;
pub use provider::{LlmConfig, LlmProvider};
pub use review::{CommentMode, FailOn, ReviewConfig, ReviewPolicy, Severity};
pub use summary::SummaryConfig;

use anyhow::{Context, bail};
use schema::CururuToml;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub github: GitHubConfig,
    pub llm: LlmConfig,
    pub review: ReviewConfig,
    pub context: ContextConfig,
    pub summary: SummaryConfig,
    pub analysis: AnalysisConfig,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let repository = env::env_required("GITHUB_REPOSITORY")?;
        let (owner, repo) = repository
            .split_once('/')
            .context("GITHUB_REPOSITORY must be owner/repo")?;
        let owner = owner.to_string();
        let repo = repo.to_string();

        let pr_number = env::env_optional("PR_NUMBER")
            .or_else(|| {
                env::env_optional("GITHUB_REF_NAME")
                    .and_then(|v| v.split('/').next().map(String::from))
            })
            .and_then(|v| v.parse::<u64>().ok())
            .context("set PR_NUMBER env var")?;

        let provider_name = env::env_optional("CURURU_PROVIDER");
        let provider = provider_name
            .as_deref()
            .and_then(LlmProvider::from_name)
            .unwrap_or(LlmProvider::OpenRouter);

        let base_url = env::env_optional("LLM_BASE_URL")
            .unwrap_or_else(|| provider.default_base_url().to_string());

        let model =
            env::env_optional("LLM_MODEL").unwrap_or_else(|| provider.default_model().to_string());

        let ignore_globs = std::env::var("CURURU_IGNORE").unwrap_or_else(|_| {
            "**/Cargo.lock,**/package-lock.json,**/pnpm-lock.yaml,**/yarn.lock,**/dist/**,**/build/**".to_string()
        });

        let mut policy = env::env_optional("CURURU_PROFILE").map_or_else(
            || Ok(ReviewPolicy::default()),
            |value| review::profile_defaults(&value),
        )?;
        if let Some(value) = env::env_optional("CURURU_FAIL_ON") {
            policy.fail_on = FailOn::from_name(&value)
                .with_context(|| format!("invalid CURURU_FAIL_ON: {value}"))?;
        }

        Ok(Self {
            github: GitHubConfig {
                token: env::env_required("GITHUB_TOKEN")?,
                repository,
                owner,
                repo,
                pr_number,
                api_url: std::env::var("GITHUB_API_URL")
                    .unwrap_or_else(|_| "https://api.github.com".to_string()),
                server_url: std::env::var("GITHUB_SERVER_URL")
                    .unwrap_or_else(|_| "https://github.com".to_string()),
            },
            llm: LlmConfig {
                provider,
                base_url,
                api_key: env::env_required("LLM_API_KEY")?,
                model,
                temperature: env::env_parse("LLM_TEMPERATURE", 0.1)?,
                max_output_tokens: env::env_parse("LLM_MAX_OUTPUT_TOKENS", 4000)?,
            },
            review: ReviewConfig {
                max_diff_bytes: env::env_parse("CURURU_MAX_DIFF_BYTES", 180_000)?,
                chunk_bytes: env::env_parse("CURURU_CHUNK_BYTES", 45_000)?,
                ignore: env::build_globs(&ignore_globs)?,
                language: env::env_optional("CURURU_LANGUAGE").unwrap_or_else(|| "pt-BR".into()),
                comment_mode: CommentMode::Inline,
                policy,
            },
            context: ContextConfig::default(),
            summary: SummaryConfig::default(),
            analysis: AnalysisConfig::default(),
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
            if env::env_optional("CURURU_PROVIDER").is_none()
                && let Some(name) = tp.name
                && let Some(p) = LlmProvider::from_name(&name)
            {
                self.llm.provider = p;
                if env::env_optional("LLM_BASE_URL").is_none() {
                    self.llm.base_url = p.default_base_url().to_string();
                }
                if env::env_optional("LLM_MODEL").is_none() {
                    self.llm.model = p.default_model().to_string();
                }
            }
            if env::env_optional("LLM_BASE_URL").is_none()
                && let Some(url) = tp.base_url
            {
                self.llm.base_url = url;
            }
            if env::env_optional("LLM_MODEL").is_none()
                && let Some(model) = tp.model
            {
                self.llm.model = model;
            }
            if env::env_optional("LLM_TEMPERATURE").is_none()
                && let Some(temperature) = tp.temperature
            {
                self.llm.temperature = temperature;
            }
            if env::env_optional("LLM_MAX_OUTPUT_TOKENS").is_none()
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
                self.review.ignore = env::build_globs(&patterns.join(","))?;
            }
            if env::env_optional("CURURU_LANGUAGE").is_none()
                && let Some(lang) = tr.language
            {
                self.review.language = lang;
            }
            if let Some(mode) = tr.comment_mode
                && let Some(m) = CommentMode::from_name(&mode)
            {
                self.review.comment_mode = m;
            }

            if env::env_optional("CURURU_PROFILE").is_none()
                && let Some(profile) = tr.profile
            {
                self.review.policy = review::profile_defaults(&profile)?;
            }
        }

        if let Some(tp) = parsed.policy {
            if let Some(v) = tp.minimum_confidence {
                review::validate_confidence(v)?;
                self.review.policy.minimum_confidence = v;
            }
            if let Some(v) = tp.max_findings {
                self.review.policy.max_findings = v;
            }
            if env::env_optional("CURURU_FAIL_ON").is_none()
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

        if let Some(ta) = parsed.analysis {
            if let Some(v) = ta.enabled {
                self.analysis.enabled = v;
            }
            if let Some(v) = ta.manifest {
                self.analysis.manifest = Some(v);
            }
            if let Some(v) = ta.sarif_paths {
                self.analysis.sarif_paths = v;
            }
            if let Some(v) = ta.max_findings {
                self.analysis.max_findings = v;
            }
            if let Some(v) = ta.require_current_head {
                self.analysis.require_current_head = v;
            }
            if let Some(v) = ta.check_runs {
                self.analysis.check_runs = v;
            }
            if let Some(v) = ta.check_run_names {
                self.analysis.check_run_names = v;
            }
        }

        Ok(())
    }
}
