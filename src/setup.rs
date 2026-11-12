//! Guided setup and a feature overview, so operators can see and enable
//! what Cururu does without reading every document.

use crate::config::AppConfig;
use anyhow::Context;
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
        "  Limits: diff {} bytes, chunk {} bytes, model output {} tokens per call (raise provider.max_output_tokens if reviews end with finish reason `length`)",
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
