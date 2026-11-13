//! Guided setup and a feature overview, so operators can see and enable
//! what Cururu does without reading every document.

use crate::config::AppConfig;
use anyhow::Context;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};
use std::path::Path;

pub const AUTO_APPROVAL_TEMPLATE: &str = r#"
[auto_approval]
# Opt-in: approve a PR without a human only when every rule below passes.
# A PR outside these rules gets a "needs human review" notice instead.
enabled = true
# Every changed file must match one of these globs (an empty list allows any path).
allowed_paths = ["docs/**", "**/*.md", "**/*.mdx", "**/locales/**", "**/i18n/**"]
# Any change in these paths always needs a human reviewer.
blocked_paths = [
  ".github/**",
  "**/migrations/**",
  "**/payments/**",
  "**/billing/**",
  "**/auth/**",
  "**/security/**",
  "**/infra/**",
  "**/*.lock",
  "Dockerfile",
]
max_changed_files = 20
max_changed_lines = 400
# Findings at or above this severity block approval: critical, high, medium or low.
block_on = "low"
"#;

const REQUIREMENTS: &str = "\
Requirements for Cururu to approve PRs:
  - GitHub Actions: enable Settings → Actions → General → \"Allow GitHub Actions to create
    and approve pull requests\"; the workflow needs `pull-requests: write` and `checks: write`.
    Whether an Actions approval counts toward required reviewers depends on your branch
    protection; use the Cururu GitHub App if it must count.
  - GitHub App: grant Pull requests (write) and Checks (write).
  - Cururu never merges. If approval is refused, it posts a \"needs human review\" notice
    explaining why, and the workflow output auto_approval is needs_human.
  - Workflow outputs for your own notifications: auto_approval (approved, needs_human,
    disabled, not_evaluated), auto_approval_reasons (JSON) and pr_url.";

/// Estimated diff bytes per changed line and per file header; GitHub reports
/// line counts, not diff sizes, so diff sizes are estimates.
const ESTIMATED_BYTES_PER_LINE: u64 = 60;
const ESTIMATED_BYTES_PER_FILE: u64 = 200;

#[derive(Debug, PartialEq, Eq)]
pub struct LimitSuggestion {
    pub sampled: usize,
    pub runs_with_usage: usize,
    pub usage_unavailable: usize,
    pub largest_completion: Option<u64>,
    pub output_tokens: Option<u32>,
    pub largest_estimated_bytes: u64,
    pub over_diff_limit: usize,
    pub max_diff_bytes: Option<usize>,
}

const fn round_up(value: u64, step: u64) -> u64 {
    value.div_ceil(step) * step
}

/// Suggest limits from sampled PRs. Output tokens use the largest recorded
/// completion plus 50% headroom; the diff limit covers the 90th percentile
/// estimated PR size plus 25%. Only increases are suggested.
pub fn suggest_limits(
    samples: &[crate::github::PullSample],
    config: &AppConfig,
) -> LimitSuggestion {
    let largest_completion = samples
        .iter()
        .filter_map(|sample| sample.completion_tokens)
        .max();
    let output_tokens = largest_completion
        .map(|tokens| {
            round_up(tokens.saturating_mul(3) / 2, 1000).clamp(
                4000,
                u64::from(crate::diagnostics::MAX_SUGGESTED_OUTPUT_TOKENS),
            )
        })
        .and_then(|tokens| u32::try_from(tokens).ok())
        // Only raise it: truncated reviews record no usage, so history
        // underestimates, and a lower limit saves little since only
        // generated tokens are billed.
        .filter(|tokens| *tokens > config.llm.max_output_tokens);
    let mut sizes: Vec<u64> = samples
        .iter()
        .map(|sample| {
            sample.changed_lines * ESTIMATED_BYTES_PER_LINE
                + sample.changed_files * ESTIMATED_BYTES_PER_FILE
        })
        .collect();
    sizes.sort_unstable();
    let current = config.review.max_diff_bytes as u64;
    let over_diff_limit = sizes.iter().filter(|size| **size > current).count();
    // Nearest-rank 90th percentile: ceil(0.9 n) - 1.
    let p90 = sizes
        .get(sizes.len().saturating_mul(9).div_ceil(10).saturating_sub(1))
        .copied()
        .unwrap_or_default();
    let max_diff_bytes = (p90 > current)
        .then(|| round_up(p90.saturating_mul(5) / 4, 10_000))
        .and_then(|bytes| usize::try_from(bytes).ok());
    LimitSuggestion {
        sampled: samples.len(),
        runs_with_usage: samples
            .iter()
            .filter(|sample| sample.completion_tokens.is_some())
            .count(),
        usage_unavailable: samples
            .iter()
            .filter(|sample| sample.usage_unavailable)
            .count(),
        largest_completion,
        output_tokens,
        largest_estimated_bytes: sizes.last().copied().unwrap_or_default(),
        over_diff_limit,
        max_diff_bytes,
    }
}

/// Sample recent PRs and print suggested limits with a TOML snippet.
pub async fn limits(path: &Path, sample: usize) -> anyhow::Result<()> {
    let mut config = AppConfig::from_local_env()?;
    if let Ok(raw) = std::fs::read_to_string(path) {
        config.merge_toml_str(&raw)?;
    }
    anyhow::ensure!(
        !config.source_control.repository.is_empty() && !config.source_control.token.is_empty(),
        "set GITHUB_TOKEN (or CURURU_SCM_TOKEN) and CURURU_REPOSITORY, or run inside the repository checkout"
    );
    let github = crate::github::GitHubClient::from_scm_config(&config.source_control)?;
    let samples = github.sample_pull_requests(sample).await?;
    let suggestion = suggest_limits(&samples, &config);
    println!(
        "Sampled {} recent PRs in {}.",
        suggestion.sampled, config.source_control.repository
    );
    println!(
        "  Output: {} token(s) per call now; {}.",
        config.llm.max_output_tokens,
        suggestion.largest_completion.map_or_else(
            || "no Cururu review usage recorded on these PRs yet".to_string(),
            |tokens| format!(
                "largest recorded completion {tokens} token(s) across {} PR(s)",
                suggestion.runs_with_usage
            )
        )
    );
    if suggestion.usage_unavailable > 0 {
        println!(
            "  Warning: the Cururu checks of {} PR(s) could not be read; their usage is missing from this sample.",
            suggestion.usage_unavailable
        );
    }
    println!(
        "  Diff: {} bytes now; largest PR is about {} bytes (estimated from line counts); {} of {} sampled PRs would exceed the limit.",
        config.review.max_diff_bytes,
        suggestion.largest_estimated_bytes,
        suggestion.over_diff_limit,
        suggestion.sampled
    );
    let mut snippet = String::new();
    if let Some(tokens) = suggestion.output_tokens {
        let _ = writeln!(snippet, "[provider]\nmax_output_tokens = {tokens}");
    }
    if let Some(bytes) = suggestion.max_diff_bytes {
        let _ = writeln!(snippet, "[review]\nmax_diff_bytes = {bytes}");
    }
    if snippet.is_empty() {
        println!(
            "\nThe current limits fit the sampled PRs; no increase suggested. Reviews that stopped at the output limit record no usage, so also check for \"Review could not complete\" checks."
        );
    } else {
        println!(
            "\nSuggested values for .cururu.toml (or the organization's shared configuration):\n\n{snippet}"
        );
        println!(
            "Diff sizes are estimates. A larger diff limit reviews bigger PRs at higher cost; splitting large PRs is often better. Only generated tokens are billed, so a higher output limit raises the worst case only."
        );
    }
    Ok(())
}

fn has_section(raw: &str, section: &str) -> anyhow::Result<bool> {
    let value: toml::Value = toml::from_str(raw).context("invalid .cururu.toml")?;
    Ok(value.get(section).is_some())
}

/// Append the auto-approval section to `.cururu.toml` after an interactive
/// confirmation, or only preview it. An existing section is never replaced.
pub async fn auto_approval(path: &Path, write: bool, check_remote: bool) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}; run `cururu init` first", path.display()))?;
    anyhow::ensure!(
        !has_section(&raw, "auto_approval")?,
        "{} already has an [auto_approval] section; edit it directly (see `cururu status`)",
        path.display()
    );
    let mut updated = raw.clone();
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(AUTO_APPROVAL_TEMPLATE);
    let mut validation = AppConfig::from_local_env()?;
    validation
        .merge_toml_str(&updated)
        .context("the resulting configuration would be invalid")?;

    println!(
        "Section to append to {}:\n{AUTO_APPROVAL_TEMPLATE}",
        path.display()
    );
    println!("{REQUIREMENTS}\n");
    if check_remote {
        println!("{}", repository_setting_report().await);
    }
    if !write {
        println!("\nPreview only. Re-run with --write to append it after confirmation.");
        return Ok(());
    }
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        "writing requires an interactive terminal; nothing was changed"
    );
    eprint!("Type WRITE to append this section to {}: ", path.display());
    std::io::stderr().flush()?;
    let mut confirmation = String::new();
    std::io::stdin().read_line(&mut confirmation)?;
    anyhow::ensure!(
        confirmation.trim_end() == "WRITE",
        "cancelled; nothing was changed"
    );
    anyhow::ensure!(
        std::fs::read_to_string(path)? == raw,
        "{} changed while waiting for confirmation; nothing was written",
        path.display()
    );
    replace_file(path, updated.as_bytes())?;
    println!(
        "Appended [auto_approval] to {}. Commit it to the default branch; Cururu reads it from the PR base.",
        path.display()
    );
    Ok(())
}

/// Replace `path` atomically through a new, uniquely named temporary file in
/// the same directory. The temporary is created exclusively, so a pre-existing
/// file or symlink planted in the repository is never followed or overwritten.
fn replace_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".cururu-toml-")
        .tempfile_in(directory)
        .with_context(|| {
            format!(
                "failed to create a temporary file in {}",
                directory.display()
            )
        })?;
    temporary.write_all(bytes)?;
    // Keep the original file's permissions; the temporary starts as 0600.
    if let Ok(metadata) = std::fs::metadata(path) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .with_context(|| format!("failed to keep the permissions of {}", path.display()))?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

/// Check the repository setting that lets GitHub Actions approve PRs.
async fn repository_setting_report() -> String {
    let Ok(config) = AppConfig::from_local_env() else {
        return "Could not load the local configuration to check the repository setting.".into();
    };
    if config.source_control.repository.is_empty() || config.source_control.token.is_empty() {
        return "Repository setting not checked: set GITHUB_TOKEN (or CURURU_SCM_TOKEN) and run inside the repository checkout to verify that GitHub Actions may approve PRs.".into();
    }
    let Ok(github) = crate::github::GitHubClient::from_scm_config(&config.source_control) else {
        return "Repository setting not checked: unsupported repository host.".into();
    };
    match github.actions_can_approve_pull_requests().await {
        Ok(true) => format!(
            "✓ {}: GitHub Actions is allowed to approve pull requests.",
            config.source_control.repository
        ),
        Ok(false) => format!(
            "⚠ {}: GitHub Actions is NOT allowed to approve pull requests. Enable Settings → Actions → General → \"Allow GitHub Actions to create and approve pull requests\", or use the Cururu GitHub App. Until then Cururu will post a \"needs human review\" notice explaining this.",
            config.source_control.repository
        ),
        Err(error) => format!(
            "Could not verify whether GitHub Actions may approve PRs in {} ({error}); reading this setting needs admin access. Check Settings → Actions → General.",
            config.source_control.repository
        ),
    }
}

const fn on_off(value: bool) -> &'static str {
    if value { "enabled" } else { "disabled" }
}

/// Print which Cururu features the local configuration enables.
pub fn status(path: &Path) -> anyhow::Result<()> {
    let mut config = AppConfig::from_local_env()?;
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => Some(raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("failed to read .cururu.toml"),
    };
    let shared = match &raw {
        Some(raw) => {
            config.merge_toml_str(raw)?;
            AppConfig::shared_base_from_toml(raw)?
        }
        None => None,
    };
    let repository = if config.source_control.repository.is_empty() {
        "(unknown repository)"
    } else {
        config.source_control.repository.as_str()
    };
    println!("Cururu status for {repository}");
    match (&raw, &shared) {
        (None, _) => println!(
            "  Configuration: no {} (defaults; run `cururu init`)",
            path.display()
        ),
        (Some(_), Some(shared)) => println!(
            "  Configuration: {} extending {}:{} (shared values not loaded here; use `cururu print-config`)",
            path.display(),
            shared.repository,
            shared.path
        ),
        (Some(_), None) => println!("  Configuration: {}", path.display()),
    }
    let review = &config.review;
    println!(
        "  Review: language {}, profile {}, {:?} comments, fail_on {:?}, suggested changes {}",
        review.language,
        review.policy.profile,
        review.comment_mode,
        review.policy.fail_on,
        on_off(review.policy.suggested_changes)
    );
    println!(
        "  Hosted publication: each review needs a separate approval (App check or `/cururu approve`; Action environment `cururu-publish`; or `cururu approve-hosted-review`)"
    );
    let approval = &config.auto_approval;
    if approval.enabled {
        println!(
            "  Auto-approval: enabled — {} allowed and {} blocked path patterns, at most {} files and {} lines, findings at or above {} block (setup: `cururu setup auto-approval`)",
            approval.allowed_paths.len(),
            approval.blocked_paths.len(),
            approval.max_changed_files,
            approval.max_changed_lines,
            approval.block_on.as_str()
        );
    } else {
        println!("  Auto-approval: disabled (enable with `cururu setup auto-approval`)");
    }
    println!("  Analyzer evidence: {}", on_off(config.analysis.enabled));
    println!(
        "  Automatic context: {}",
        on_off(config.context.auto.enabled)
    );
    println!(
        "  Summary: cost {}, token usage {}",
        if config.summary.show_cost {
            "shown"
        } else {
            "hidden"
        },
        if config.summary.show_usage {
            "shown"
        } else {
            "hidden"
        }
    );
    println!(
        "  Evaluator (this environment): {}",
        match config.evaluator.mode {
            Some(crate::evaluation::EvaluationMode::Observe) => "observe",
            Some(crate::evaluation::EvaluationMode::Filter) => "filter (disabled pending rollout)",
            None => "disabled",
        }
    );
    println!(
        "  Limits: diff {} bytes, chunk {} bytes, model output {} tokens per call (suggestions: `cururu setup limits`)",
        config.review.max_diff_bytes, config.review.chunk_bytes, config.llm.max_output_tokens
    );
    println!(
        "  Local review: `cururu api-review` (configured API) or the MCP server (`cururu install-mcp`)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_valid_and_existing_sections_are_detected() {
        let base = "version = 1\n[review]\nlanguage = \"pt-BR\"\n";
        let mut config = AppConfig::from_local_env().unwrap();
        config
            .merge_toml_str(&format!("{base}{AUTO_APPROVAL_TEMPLATE}"))
            .unwrap();
        assert!(config.auto_approval.enabled);
        assert!(
            config
                .auto_approval
                .blocked_paths
                .contains(&"**/payments/**".to_string())
        );
        assert!(!has_section(base, "auto_approval").unwrap());
        assert!(has_section(&format!("{base}{AUTO_APPROVAL_TEMPLATE}"), "auto_approval").unwrap());
    }

    #[test]
    fn limits_use_recorded_usage_and_estimated_sizes() {
        use crate::github::PullSample;
        let mut config = AppConfig::from_local_env().unwrap();
        config.llm.max_output_tokens = 4000;
        config.review.max_diff_bytes = 100_000;
        let sample = |lines, tokens| PullSample {
            number: 1,
            changed_lines: lines,
            changed_files: 10,
            completion_tokens: tokens,
            usage_unavailable: false,
        };
        let samples: Vec<_> = (0..9)
            .map(|_| sample(200, None))
            .chain([sample(5000, Some(5200))])
            .collect();
        let suggestion = suggest_limits(&samples, &config);
        assert_eq!(suggestion.largest_completion, Some(5200));
        assert_eq!(suggestion.output_tokens, Some(8000));
        assert_eq!(suggestion.over_diff_limit, 1);
        // A single outlier among ten PRs does not raise the diff limit.
        assert_eq!(suggestion.max_diff_bytes, None);

        // Nothing recorded and small PRs: no change.
        let quiet = suggest_limits(&[sample(100, None)], &config);
        assert_eq!((quiet.output_tokens, quiet.max_diff_bytes), (None, None));
        // Low recorded usage never lowers the limit.
        let low = suggest_limits(&[sample(100, Some(900))], &config);
        assert_eq!(low.output_tokens, None);
        // The 90th percentile of 10 samples is the 9th smallest, not the largest.
        let mut ten: Vec<_> = (1..=9).map(|n| sample(n * 1000, None)).collect();
        ten.push(sample(100_000, None));
        let p90 = suggest_limits(&ten, &config);
        assert_eq!(p90.max_diff_bytes, Some(680_000));
        // Suggestions are capped.
        let huge = suggest_limits(&[sample(10, Some(100_000))], &config);
        assert_eq!(
            huge.output_tokens,
            Some(crate::diagnostics::MAX_SUGGESTED_OUTPUT_TOKENS)
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacement_never_follows_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("victim");
        std::fs::write(&target, "keep").unwrap();
        let path = dir.path().join(".cururu.toml");
        std::fs::write(&path, "version = 1\n").unwrap();
        // The old fixed temporary name, planted as a symlink to another file.
        std::os::unix::fs::symlink(&target, dir.path().join(".cururu.toml.cururu-tmp")).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();
        replace_file(&path, b"version = 1\n# updated\n").unwrap();
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o644);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "version = 1\n# updated\n"
        );
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn preview_never_writes_and_existing_section_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".cururu.toml");
        std::fs::write(&path, "version = 1\n").unwrap();
        auto_approval(&path, false, false).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "version = 1\n");
        std::fs::write(&path, format!("version = 1\n{AUTO_APPROVAL_TEMPLATE}")).unwrap();
        assert!(auto_approval(&path, false, false).await.is_err());
        assert!(
            auto_approval(&dir.path().join("missing.toml"), false, false)
                .await
                .is_err()
        );
        status(&path).unwrap();
    }
}
