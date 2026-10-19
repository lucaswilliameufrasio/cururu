//! Portable `.cururu.toml` shape and parsing. No filesystem, environment,
//! credentials, SCM, or authorization side effects live in this module.

use anyhow::{Context as _, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CururuToml {
    pub version: u32,
    pub config: Option<ConfigToml>,
    pub provider: Option<ProviderToml>,
    pub review: Option<ReviewToml>,
    pub policy: Option<PolicyToml>,
    pub context: Option<ContextToml>,
    pub summary: Option<SummaryToml>,
    pub analysis: Option<AnalysisToml>,
    pub evaluator: Option<toml::Value>,
}

impl CururuToml {
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        let parsed: Self = serde_path_to_error::deserialize(toml::Deserializer::new(raw))
            .context("failed to parse .cururu.toml")?;
        if parsed.version != 1 {
            bail!(
                "unsupported .cururu.toml version {} (expected 1)",
                parsed.version
            );
        }
        Ok(parsed)
    }
}

#[derive(Debug, Deserialize)]
pub struct ConfigToml {
    pub base: Option<String>,
    pub base_ref: Option<String>,
    pub base_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProviderToml {
    pub name: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewToml {
    pub max_diff_bytes: Option<usize>,
    pub chunk_bytes: Option<usize>,
    pub ignore: Option<Vec<String>>,
    pub language: Option<String>,
    pub tone: Option<String>,
    pub technical_level: Option<String>,
    pub suggestion_detail: Option<String>,
    pub comment_mode: Option<String>,
    pub recommendations: Option<bool>,
    pub profile: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PolicyToml {
    pub minimum_confidence: Option<f32>,
    pub max_findings: Option<usize>,
    pub fail_on: Option<String>,
    pub allowed_severities: Option<Vec<String>>,
    pub suggested_changes: Option<bool>,
    pub incremental: Option<bool>,
    pub synthesis: Option<bool>,
    pub focus: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ContextToml {
    pub conventions: Option<Vec<String>>,
    pub specifications: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub additional: Option<Vec<String>>,
    pub max_bytes: Option<usize>,
    pub auto: Option<AutoContextToml>,
}

#[derive(Debug, Deserialize)]
pub struct AutoContextToml {
    pub enabled: Option<bool>,
    pub max_bytes: Option<usize>,
    pub max_files: Option<usize>,
    pub per_file_bytes: Option<usize>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct SummaryToml {
    pub show_cost: Option<bool>,
    pub show_usage: Option<bool>,
    pub logo_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AnalysisToml {
    pub enabled: Option<bool>,
    pub manifest: Option<String>,
    pub sarif_paths: Option<Vec<String>>,
    pub max_findings: Option<usize>,
    pub require_current_head: Option<bool>,
    pub check_runs: Option<bool>,
    pub check_run_names: Option<Vec<String>>,
}

pub fn validate_confidence(value: f32) -> anyhow::Result<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        bail!("policy.minimum_confidence must be finite and between 0 and 1")
    }
}

pub fn validate_choice(field: &str, value: &str, choices: &[&str]) -> anyhow::Result<()> {
    if !choices.contains(&value) {
        bail!(
            "invalid {field}: {value}; expected one of {}",
            choices.join(", ")
        );
    }
    Ok(())
}

/// Combines repository TOML documents without loading resources or applying host
/// configuration, environment precedence, permissions or schema validation.
pub fn compose_toml(base: &str, local: &str) -> anyhow::Result<String> {
    let base: toml::Value = toml::from_str(base).context("invalid shared base TOML")?;
    let local: toml::Value = toml::from_str(local).context("invalid local TOML")?;
    toml::to_string(&merge_values(base, local, ""))
        .context("failed to serialize effective configuration")
}

fn merge_values(base: toml::Value, local: toml::Value, path: &str) -> toml::Value {
    match (base, local) {
        (toml::Value::Table(mut base), toml::Value::Table(local)) => {
            for (key, local_value) in local {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let value = base.remove(&key).map_or_else(
                    || local_value.clone(),
                    |base_value| merge_values(base_value, local_value.clone(), &child_path),
                );
                base.insert(key, value);
            }
            toml::Value::Table(base)
        }
        (toml::Value::Array(mut base), toml::Value::Array(local))
            if matches!(
                path,
                "review.ignore"
                    | "policy.focus"
                    | "context.conventions"
                    | "context.specifications"
                    | "context.skills"
                    | "context.additional"
                    | "context.auto.include"
                    | "context.auto.exclude"
            ) =>
        {
            for value in local {
                if !base.contains(&value) {
                    base.push(value);
                }
            }
            toml::Value::Array(base)
        }
        (_, local) => local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_preserves_recursive_precedence_and_cumulative_array_order() {
        let base = r#"version = 1
future = "base"
[review]
max_diff_bytes = 100
ignore = ["base/**", "same/**"]
[policy]
minimum_confidence = 0.9
allowed_severities = ["critical", "high"]
focus = ["correctness", "security"]
[context]
conventions = ["BASE.md"]
specifications = ["BASE.md"]
skills = ["BASE.md"]
additional = ["BASE.md"]
[context.auto]
include = ["base/**"]
exclude = ["generated/**"]
"#;
        let local = r#"version = 1
future = "local"
[review]
max_diff_bytes = 200
ignore = ["same/**", "local/**"]
[policy]
allowed_severities = ["medium"]
focus = ["security", "performance"]
[context]
conventions = ["BASE.md", "LOCAL.md"]
specifications = ["LOCAL.md"]
skills = ["LOCAL.md"]
additional = ["LOCAL.md"]
[context.auto]
include = ["local/**"]
exclude = ["generated/**", "vendor/**"]
"#;
        let merged = compose_toml(base, local).unwrap();
        let value: toml::Value = toml::from_str(&merged).unwrap();
        assert_eq!(value["future"].as_str(), Some("local"));
        let parsed = CururuToml::parse(&merged).unwrap();
        let review = parsed.review.unwrap();
        assert_eq!(review.max_diff_bytes, Some(200));
        assert_eq!(review.ignore.unwrap(), ["base/**", "same/**", "local/**"]);
        let policy = parsed.policy.unwrap();
        assert_eq!(policy.minimum_confidence, Some(0.9));
        assert_eq!(policy.allowed_severities.unwrap(), ["medium"]);
        assert_eq!(
            policy.focus.unwrap(),
            ["correctness", "security", "performance"]
        );
        let context = parsed.context.unwrap();
        for paths in [
            context.conventions,
            context.specifications,
            context.skills,
            context.additional,
        ] {
            assert_eq!(paths.unwrap(), ["BASE.md", "LOCAL.md"]);
        }
        let auto = context.auto.unwrap();
        assert_eq!(auto.include.unwrap(), ["base/**", "local/**"]);
        assert_eq!(auto.exclude.unwrap(), ["generated/**", "vendor/**"]);
    }

    #[test]
    fn composition_preserves_errors_and_does_not_apply_or_authorize_settings() {
        assert_eq!(
            compose_toml("[", "version = 1").unwrap_err().to_string(),
            "invalid shared base TOML"
        );
        assert_eq!(
            compose_toml("version = 1", "[").unwrap_err().to_string(),
            "invalid local TOML"
        );
        let merged =
            compose_toml("version = 1\n[evaluator]\nmode = 'filter'", "version = 99").unwrap();
        let raw: toml::Value = toml::from_str(&merged).unwrap();
        assert_eq!(raw["evaluator"]["mode"].as_str(), Some("filter"));
        // Composition preserves document values. Hosts still reject unsupported
        // versions/settings and control every permission and runtime side effect.
        assert!(CururuToml::parse(&merged).is_err());
        let base: toml::Value = toml::from_str("[future]\na = 1").unwrap();
        let local: toml::Value = toml::from_str("future = 'replacement'").unwrap();
        assert_eq!(
            merge_values(base, local, "")["future"].as_str(),
            Some("replacement")
        );
    }

    #[test]
    fn minimal_config_keeps_optional_fields_absent_and_unknown_fields_compatible() {
        let parsed = CururuToml::parse("version = 1\nfuture_extension = true").unwrap();
        assert!(parsed.provider.is_none());
        assert!(parsed.policy.is_none());
        assert!(parsed.config.is_none());
    }

    #[test]
    fn version_and_nested_type_errors_retain_the_existing_diagnostics() {
        assert_eq!(
            CururuToml::parse("version = 2").unwrap_err().to_string(),
            "unsupported .cururu.toml version 2 (expected 1)"
        );
        let error =
            CururuToml::parse("version = 1\n[policy]\nmax_findings = 'invalid'").unwrap_err();
        assert!(format!("{error:#}").contains("policy.max_findings"));
        assert!(CururuToml::parse("[review]\ntone = 'direct'").is_err());
    }

    #[test]
    fn shared_schema_covers_all_domains_without_applying_host_policy() {
        let raw = r#"version = 1
[config]
base = "org/policy"
base_ref = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
base_path = ".cururu.toml"
[provider]
name = "openai"
model = "review-model"
[review]
max_diff_bytes = 500
tone = "direct"
[policy]
minimum_confidence = 0.9
suggested_changes = false
[context]
conventions = ["CONTEXT.md"]
[context.auto]
enabled = false
[summary]
show_cost = false
[analysis]
enabled = true
[evaluator]
mode = "filter"
"#;
        let parsed = CururuToml::parse(raw).unwrap();
        assert_eq!(parsed.review.unwrap().max_diff_bytes, Some(500));
        assert_eq!(parsed.policy.unwrap().minimum_confidence, Some(0.9));
        assert_eq!(parsed.context.unwrap().auto.unwrap().enabled, Some(false));
        // Parsing untrusted settings never enables a provider or evaluator.
        assert!(parsed.evaluator.is_some());
    }

    #[test]
    fn pure_validation_rejects_nonfinite_confidence_and_preserves_choice_errors() {
        for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(validate_confidence(value).is_err());
        }
        assert!(validate_confidence(0.0).is_ok());
        assert!(validate_confidence(1.0).is_ok());
        assert_eq!(
            validate_choice("review.tone", "other", &["neutral", "direct"])
                .unwrap_err()
                .to_string(),
            "invalid review.tone: other; expected one of neutral, direct"
        );
    }
}
