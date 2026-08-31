use serde::Deserialize;

/// Schema of `.cururu.toml`. Deserialized once, merged field-by-field into
/// [`crate::config::AppConfig`](super::AppConfig) by the parent module.
#[derive(Debug, Deserialize)]
pub(super) struct CururuToml {
    pub version: u32,
    #[serde(default)]
    pub(super) provider: Option<ProviderToml>,
    #[serde(default)]
    pub(super) review: Option<ReviewToml>,
    #[serde(default)]
    pub(super) policy: Option<PolicyToml>,
    #[serde(default)]
    pub(super) context: Option<ContextToml>,
    #[serde(default)]
    pub(super) summary: Option<SummaryToml>,
    #[serde(default)]
    pub(super) analysis: Option<AnalysisToml>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProviderToml {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) base_url: Option<String>,
    #[serde(default)]
    pub(super) temperature: Option<f32>,
    #[serde(default)]
    pub(super) max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReviewToml {
    #[serde(default)]
    pub(super) max_diff_bytes: Option<usize>,
    #[serde(default)]
    pub(super) chunk_bytes: Option<usize>,
    #[serde(default)]
    pub(super) ignore: Option<Vec<String>>,
    #[serde(default)]
    pub(super) language: Option<String>,
    #[serde(default)]
    pub(super) comment_mode: Option<String>,
    #[serde(default)]
    pub(super) profile: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PolicyToml {
    #[serde(default)]
    pub(super) minimum_confidence: Option<f32>,
    #[serde(default)]
    pub(super) max_findings: Option<usize>,
    #[serde(default)]
    pub(super) fail_on: Option<String>,
    #[serde(default)]
    pub(super) allowed_severities: Option<Vec<String>>,
    #[serde(default)]
    pub(super) suggested_changes: Option<bool>,
    #[serde(default)]
    pub(super) incremental: Option<bool>,
    #[serde(default)]
    pub(super) synthesis: Option<bool>,
    #[serde(default)]
    pub(super) focus: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ContextToml {
    #[serde(default)]
    pub(super) conventions: Option<Vec<String>>,
    #[serde(default)]
    pub(super) specifications: Option<Vec<String>>,
    #[serde(default)]
    pub(super) skills: Option<Vec<String>>,
    #[serde(default)]
    pub(super) additional: Option<Vec<String>>,
    #[serde(default)]
    pub(super) max_bytes: Option<usize>,
    #[serde(default)]
    pub(super) auto: Option<AutoContextToml>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AutoContextToml {
    #[serde(default)]
    pub(super) enabled: Option<bool>,
    #[serde(default)]
    pub(super) max_bytes: Option<usize>,
    #[serde(default)]
    pub(super) max_files: Option<usize>,
    #[serde(default)]
    pub(super) per_file_bytes: Option<usize>,
    #[serde(default)]
    pub(super) include: Option<Vec<String>>,
    #[serde(default)]
    pub(super) exclude: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SummaryToml {
    #[serde(default)]
    pub(super) show_cost: Option<bool>,
    #[serde(default)]
    pub(super) show_usage: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnalysisToml {
    #[serde(default)]
    pub(super) enabled: Option<bool>,
    #[serde(default)]
    pub(super) manifest: Option<String>,
    #[serde(default)]
    pub(super) sarif_paths: Option<Vec<String>>,
    #[serde(default)]
    pub(super) max_findings: Option<usize>,
    #[serde(default)]
    pub(super) require_current_head: Option<bool>,
    #[serde(default)]
    pub(super) check_runs: Option<bool>,
    #[serde(default)]
    pub(super) check_run_names: Option<Vec<String>>,
}
