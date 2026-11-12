//! Opt-in auto-approval triage.
//!
//! Deterministic repository rules decide whether a reviewed change request may
//! be approved without a human. Every rule must pass and any doubt fails
//! closed: the decision then asks for human review. The model never approves
//! on its own; its findings can only block. Rules are evaluated on the whole
//! change request at the reviewed head, not on the latest increment.

use crate::config::{AppConfig, AutoApprovalConfig, Severity};
use crate::review::ReviewOutput;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;
use std::fmt::Write as _;

pub const NOTICE_MARKER: &str = "<!-- cururu:auto-approval:v1 -->";
pub const TRIAGE_CHECK_NAME: &str = "Cururu triage";

/// Why a change request cannot be approved automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Veto {
    BlockedPath {
        path: String,
        pattern: String,
    },
    UnparsedPath {
        path: String,
    },
    OutsideAllowedPaths {
        path: String,
    },
    NotReviewed {
        path: String,
    },
    TooManyFiles {
        count: usize,
        max: usize,
    },
    TooManyLines {
        count: usize,
        max: usize,
    },
    IncompleteCoverage {
        reviewed: usize,
        changed: usize,
    },
    Finding {
        severity: String,
        path: String,
        line: Option<u32>,
        title: String,
    },
    AnalysisNotPassed {
        status: String,
    },
    ApprovalNotPermitted {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub approved: bool,
    pub head_sha: String,
    pub changed_files: usize,
    pub changed_lines: usize,
    pub vetoes: Vec<Veto>,
}

impl Decision {
    /// The value of the `auto_approval` workflow output.
    pub const fn status(&self) -> &'static str {
        if self.approved {
            "approved"
        } else {
            "needs_human"
        }
    }

    /// Record that the SCM refused the approval; a human must review instead.
    pub fn refuse(&mut self, detail: &str) {
        self.approved = false;
        self.vetoes.push(Veto::ApprovalNotPermitted {
            detail: detail.into(),
        });
    }
}

fn matcher(patterns: &[String]) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

/// Paths touched by one file patch, read from the per-path header lines
/// (`rename from`, `--- a/`, …) so renames and deletions out of a protected
/// area are caught. `Err` names a quoted path that cannot be checked safely.
fn touched_paths(file: &crate::diff::ChangedFile) -> Result<Vec<String>, String> {
    let mut paths = vec![file.path.clone()];
    for line in file.patch.lines().skip(1) {
        if line.starts_with("@@ ") {
            break;
        }
        if line.starts_with("--- \"") || line.starts_with("+++ \"") {
            return Err(file.path.clone());
        }
        let path = [
            "rename from ",
            "rename to ",
            "copy from ",
            "copy to ",
            "--- a/",
            "+++ b/",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix));
        let Some(path) = path else { continue };
        if path.starts_with('"') {
            return Err(file.path.clone());
        }
        if !paths.iter().any(|known| known == path) {
            paths.push(path.to_string());
        }
    }
    Ok(paths)
}

/// Added and removed lines inside hunks; header lines such as `+++ b/x` are
/// not counted, but an added line whose content starts with `++` is.
fn changed_line_count(file: &crate::diff::ChangedFile) -> usize {
    let mut in_hunk = false;
    let mut count = 0;
    for line in file.patch.lines() {
        if line.starts_with("@@ ") {
            in_hunk = true;
        } else if line.starts_with("diff --git ") {
            in_hunk = false;
        } else if in_hunk && (line.starts_with('+') || line.starts_with('-')) {
            count += 1;
        }
    }
    count
}

fn blocks(severity: &str, threshold: Severity) -> bool {
    // An unknown severity is treated as blocking.
    Severity::from_name(severity).is_none_or(|severity| severity.rank() <= threshold.rank())
}

/// Evaluate the policy for a completed review of the current head.
pub fn evaluate(
    policy: &AutoApprovalConfig,
    analysis_enabled: bool,
    output: &ReviewOutput,
) -> anyhow::Result<Decision> {
    let allowed = matcher(&policy.allowed_paths)?;
    let blocked = matcher(&policy.blocked_paths)?;
    let mut vetoes = Vec::new();

    for path in &output.ignored_paths {
        vetoes.push(Veto::NotReviewed { path: path.clone() });
    }
    let mut changed_lines = 0usize;
    for file in &output.changed_files {
        changed_lines = changed_lines.saturating_add(changed_line_count(file));
        let paths = match touched_paths(file) {
            Ok(paths) => paths,
            Err(path) => {
                vetoes.push(Veto::UnparsedPath { path });
                continue;
            }
        };
        for path in paths {
            if let Some(index) = blocked.matches(&path).first() {
                vetoes.push(Veto::BlockedPath {
                    path: path.clone(),
                    pattern: policy.blocked_paths[*index].clone(),
                });
            }
            if !policy.allowed_paths.is_empty() && !allowed.is_match(&path) {
                vetoes.push(Veto::OutsideAllowedPaths { path });
            }
        }
    }
    let changed_files = output.changed_files.len() + output.ignored_paths.len();
    if changed_files > policy.max_changed_files {
        vetoes.push(Veto::TooManyFiles {
            count: changed_files,
            max: policy.max_changed_files,
        });
    }
    if changed_lines > policy.max_changed_lines {
        vetoes.push(Veto::TooManyLines {
            count: changed_lines,
            max: policy.max_changed_lines,
        });
    }
    if output.review.files_reviewed < output.changed_files.len() {
        vetoes.push(Veto::IncompleteCoverage {
            reviewed: output.review.files_reviewed,
            changed: output.changed_files.len(),
        });
    }
    for finding in &output.review.findings {
        if blocks(&finding.severity, policy.block_on) {
            vetoes.push(Veto::Finding {
                severity: finding.severity.clone(),
                path: finding.path.clone(),
                line: finding.line,
                title: finding.title.clone(),
            });
        }
    }
    if analysis_enabled && output.analysis.status != "passed" {
        vetoes.push(Veto::AnalysisNotPassed {
            status: output.analysis.status.clone(),
        });
    }
    Ok(Decision {
        approved: vetoes.is_empty(),
        head_sha: output.head_sha.clone(),
        changed_files,
        changed_lines,
        vetoes,
    })
}

/// Attach the decision to a review when the policy is enabled.
pub fn attach(config: &AppConfig, output: &mut ReviewOutput) -> anyhow::Result<()> {
    if config.auto_approval.enabled {
        output.auto_approval = Some(evaluate(
            &config.auto_approval,
            config.analysis.enabled,
            output,
        )?);
    }
    Ok(())
}

fn portuguese(language: &str) -> bool {
    language.to_ascii_lowercase().starts_with("pt")
}

fn code(value: &str) -> String {
    format!("`{}`", value.replace('`', "'"))
}

fn location(path: &str, line: Option<u32>) -> String {
    line.map_or_else(|| code(path), |line| code(&format!("{path}:{line}")))
}

// A table of localized messages; splitting it would hurt readability.
#[allow(clippy::too_many_lines)]
pub fn describe(veto: &Veto, language: &str) -> String {
    let pt = portuguese(language);
    match veto {
        Veto::BlockedPath { path, pattern } => {
            if pt {
                format!(
                    "{} corresponde ao caminho protegido {}",
                    code(path),
                    code(pattern)
                )
            } else {
                format!(
                    "{} matches the protected path {}",
                    code(path),
                    code(pattern)
                )
            }
        }
        Veto::UnparsedPath { path } => {
            if pt {
                format!(
                    "um caminho em {} está entre aspas e não pode ser verificado com segurança",
                    code(path)
                )
            } else {
                format!(
                    "a path in {} is quoted and cannot be checked safely",
                    code(path)
                )
            }
        }
        Veto::OutsideAllowedPaths { path } => {
            if pt {
                format!(
                    "{} está fora dos caminhos permitidos para auto-aprovação",
                    code(path)
                )
            } else {
                format!(
                    "{} is outside the paths allowed for auto-approval",
                    code(path)
                )
            }
        }
        Veto::NotReviewed { path } => {
            if pt {
                format!(
                    "{} foi alterado, mas está excluído do review pela configuração",
                    code(path)
                )
            } else {
                format!(
                    "{} changed but is excluded from review by configuration",
                    code(path)
                )
            }
        }
        Veto::TooManyFiles { count, max } => {
            if pt {
                format!("{count} arquivos alterados (limite: {max})")
            } else {
                format!("{count} changed files (limit: {max})")
            }
        }
        Veto::TooManyLines { count, max } => {
            if pt {
                format!("{count} linhas alteradas (limite: {max})")
            } else {
                format!("{count} changed lines (limit: {max})")
            }
        }
        Veto::IncompleteCoverage { reviewed, changed } => {
            if pt {
                format!("o review cobriu {reviewed} de {changed} arquivos")
            } else {
                format!("the review covered {reviewed} of {changed} files")
            }
        }
        Veto::Finding {
            severity,
            path,
            line,
            title,
        } => {
            let title = title.replace('\n', " ");
            if pt {
                format!(
                    "achado {} em {}: {title}",
                    code(severity),
                    location(path, *line)
                )
            } else {
                format!(
                    "{} finding at {}: {title}",
                    code(severity),
                    location(path, *line)
                )
            }
        }
        Veto::AnalysisNotPassed { status } => {
            if pt {
                format!("a evidência dos analisadores está {}", code(status))
            } else {
                format!("analyzer evidence is {}", code(status))
            }
        }
        Veto::ApprovalNotPermitted { detail } => {
            if pt {
                format!(
                    "o GitHub recusou a aprovação ({detail}). Em GitHub Actions, habilite *Settings → Actions → General → Allow GitHub Actions to create and approve pull requests*, ou use o GitHub App do Cururu com permissão de escrita em Pull requests"
                )
            } else {
                format!(
                    "GitHub refused the approval ({detail}). In GitHub Actions, enable *Settings → Actions → General → Allow GitHub Actions to create and approve pull requests*, or use the Cururu GitHub App with Pull requests write permission"
                )
            }
        }
    }
}

/// Markdown section explaining the decision, in the configured language.
pub fn render_section(decision: &Decision, language: &str) -> String {
    let pt = portuguese(language);
    let mut out = String::new();
    if decision.approved {
        let _ = writeln!(
            out,
            "### {}\n",
            if pt {
                "✅ Auto-aprovado"
            } else {
                "✅ Auto-approved"
            }
        );
        let _ = writeln!(
            out,
            "{}",
            if pt {
                format!(
                    "Todas as regras de auto-aprovação passaram: {} arquivos e {} linhas alterados, caminhos permitidos, nenhum caminho protegido e nenhum achado bloqueante.",
                    decision.changed_files, decision.changed_lines
                )
            } else {
                format!(
                    "Every auto-approval rule passed: {} changed files and {} changed lines, allowed paths only, no protected path and no blocking finding.",
                    decision.changed_files, decision.changed_lines
                )
            }
        );
    } else {
        let _ = writeln!(
            out,
            "### {}\n",
            if pt {
                "👀 Precisa de review humano"
            } else {
                "👀 Needs human review"
            }
        );
        let _ = writeln!(
            out,
            "{}\n",
            if pt {
                "A auto-aprovação não se aplica porque:"
            } else {
                "Auto-approval does not apply because:"
            }
        );
        for veto in &decision.vetoes {
            let _ = writeln!(out, "- {}", describe(veto, language));
        }
    }
    out
}

/// A short PR notice for a change request that needs human review. It names
/// the reasons only; findings themselves are published after approval.
pub fn render_notice(decision: &Decision, language: &str) -> String {
    let pt = portuguese(language);
    let mut out = format!(
        "{NOTICE_MARKER}\n<!-- cururu:auto-approval:head={} -->\n",
        decision.head_sha
    );
    out.push_str(&render_section(decision, language));
    let _ = write!(
        out,
        "\n{}\n",
        if pt {
            format!(
                "Revisado no commit `{}`. Isto não é uma aprovação nem uma reprovação; uma pessoa revisora precisa avaliar o PR.",
                decision.head_sha
            )
        } else {
            format!(
                "Reviewed at commit `{}`. This is neither an approval nor a rejection; a human reviewer needs to evaluate the PR.",
                decision.head_sha
            )
        }
    );
    out
}

/// Body of the approval review, bound to the reviewed head.
pub fn render_approval(decision: &Decision, language: &str) -> String {
    format!(
        "<!-- cururu:auto-approval:approved head={} -->\n{}",
        decision.head_sha,
        render_section(decision, language)
    )
}

/// How the hosted flow continues after triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The policy is not enabled; the regular flow applies.
    Disabled,
    /// Approved at the reviewed head; publish the review directly.
    Approved,
    /// A human must review; the regular approval-gated flow applies.
    NeedsHuman,
}

/// Run the auto-approval policy for a completed review, approving or posting
/// the "needs human review" notice. Approval happens before anything else is
/// published, so the summary always reflects whether it actually succeeded.
pub async fn decide(
    config: &AppConfig,
    source_control: &dyn crate::scm::ScmProvider,
    output: &mut ReviewOutput,
) -> anyhow::Result<Route> {
    if !config.auto_approval.enabled {
        return Ok(Route::Disabled);
    }
    match source_control
        .dismiss_stale_auto_approvals(&output.head_sha)
        .await
    {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "dismissed stale Cururu auto-approvals"),
        Err(error) => tracing::warn!(%error, "could not dismiss stale Cururu auto-approvals"),
    }
    attach(config, output)?;
    let language = output.language.clone();
    let head = output.head_sha.clone();
    let decision = output
        .auto_approval
        .as_mut()
        .expect("decision attached when enabled");
    if decision.approved {
        crate::review::ensure_review_head_unchanged(
            &head,
            &source_control.fetch_head_sha().await?,
        )?;
        match source_control
            .approve_change_request(&head, &render_approval(decision, &language))
            .await?
        {
            crate::scm::ApprovalOutcome::Approved => {
                report_check(source_control, decision, &language).await;
                return Ok(Route::Approved);
            }
            crate::scm::ApprovalOutcome::NotPermitted(detail) => decision.refuse(&detail),
        }
    }
    if let Err(error) = source_control
        .upsert_auto_approval_notice(&render_notice(decision, &language))
        .await
    {
        tracing::warn!(%error, "could not post the needs-human-review notice");
    }
    report_check(source_control, decision, &language).await;
    Ok(Route::NeedsHuman)
}

async fn report_check(
    source_control: &dyn crate::scm::ScmProvider,
    decision: &Decision,
    language: &str,
) {
    let (conclusion, title) = if decision.approved {
        ("success", "Auto-approved")
    } else {
        ("neutral", "Needs human review")
    };
    if let Err(error) = source_control
        .create_status_check(
            TRIAGE_CHECK_NAME,
            &decision.head_sha,
            conclusion,
            title,
            &render_section(decision, language),
        )
        .await
    {
        tracing::warn!(%error, "could not report the Cururu triage check");
    }
}

#[derive(Serialize)]
struct OutputReason<'a> {
    #[serde(flatten)]
    veto: &'a Veto,
    message: String,
}

/// JSON for the `auto_approval_reasons` workflow output.
pub fn reasons_json(decision: &Decision, language: &str) -> String {
    let reasons: Vec<OutputReason<'_>> = decision
        .vetoes
        .iter()
        .map(|veto| OutputReason {
            veto,
            message: describe(veto, language),
        })
        .collect();
    serde_json::to_string(&reasons).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ReviewFinding, ReviewResult};

    fn file(new_path: &str, old: &str, added: usize) -> crate::diff::ChangedFile {
        let mut body = format!(
            "diff --git a/{old} b/{new_path}\n--- a/{old}\n+++ b/{new_path}\n@@ -1 +1 @@\n"
        );
        for _ in 0..added {
            body.push_str("+line\n");
        }
        crate::diff::ChangedFile {
            path: new_path.into(),
            patch: body,
            right_lines: vec![1],
        }
    }

    fn output(files: Vec<crate::diff::ChangedFile>, findings: Vec<ReviewFinding>) -> ReviewOutput {
        ReviewOutput {
            review: ReviewResult {
                model: "m".into(),
                files_reviewed: files.len(),
                summary: "s".into(),
                findings,
            },
            usage: None,
            context_files: vec![],
            model: "m".into(),
            show_usage: false,
            show_cost: false,
            logo_url: None,
            changed_files: files,
            head_sha: "head".into(),
            analysis: crate::analysis::AnalysisReport {
                status: "disabled".into(),
                tools: vec![],
                findings: vec![],
            },
            evaluation: None,
            run_id: "run".into(),
            pr_usage: None,
            ignored_paths: vec![],
            auto_approval: None,
            language: "en".into(),
        }
    }

    fn finding(severity: &str) -> ReviewFinding {
        ReviewFinding {
            severity: severity.into(),
            path: "docs/a.md".into(),
            line: Some(2),
            title: "Typo".into(),
            message: "m".into(),
            suggestion: "s".into(),
            confidence: 0.9,
            suggested_change: None,
            source: None,
            rule: None,
        }
    }

    fn policy() -> AutoApprovalConfig {
        AutoApprovalConfig {
            enabled: true,
            allowed_paths: vec!["docs/**".into(), "**/*.md".into()],
            blocked_paths: vec!["**/payments/**".into()],
            max_changed_files: 3,
            max_changed_lines: 10,
            block_on: Severity::Low,
        }
    }

    #[test]
    fn simple_docs_change_without_findings_is_approved() {
        let decision = evaluate(
            &policy(),
            false,
            &output(vec![file("docs/a.md", "docs/a.md", 2)], vec![]),
        )
        .unwrap();
        assert!(decision.approved, "{decision:?}");
        assert_eq!(decision.status(), "approved");
        assert_eq!((decision.changed_files, decision.changed_lines), (1, 2));
    }

    #[test]
    fn sensitive_paths_including_rename_sources_require_humans() {
        let decision = evaluate(
            &policy(),
            false,
            &output(
                vec![file("docs/moved.md", "src/payments/fees.rs", 1)],
                vec![],
            ),
        )
        .unwrap();
        assert!(!decision.approved);
        assert!(decision.vetoes.contains(&Veto::BlockedPath {
            path: "src/payments/fees.rs".into(),
            pattern: "**/payments/**".into()
        }));
        assert!(decision.vetoes.contains(&Veto::OutsideAllowedPaths {
            path: "src/payments/fees.rs".into()
        }));
    }

    #[test]
    fn ambiguous_or_quoted_paths_and_plus_prefixed_lines_are_handled() {
        // A header whose old path contains " b/" is ambiguous; the rename lines are not.
        let renamed = file("docs/new.md", "src/payments/a b/c.rs", 1);
        let decision = evaluate(&policy(), false, &output(vec![renamed], vec![])).unwrap();
        assert!(decision.vetoes.iter().any(|veto| matches!(veto, Veto::BlockedPath { path, .. } if path == "src/payments/a b/c.rs")), "{decision:?}");

        let mut quoted = file("docs/new.md", "docs/new.md", 1);
        quoted.patch = quoted
            .patch
            .replace("--- a/docs/new.md", "--- \"a/src/payments/\\303\\247.rs\"");
        let decision = evaluate(&policy(), false, &output(vec![quoted], vec![])).unwrap();
        assert!(decision.vetoes.contains(&Veto::UnparsedPath {
            path: "docs/new.md".into()
        }));

        let mut counted = file("docs/a.md", "docs/a.md", 0);
        counted.patch.push_str("+++i;\n---j;\n+x\n");
        assert_eq!(changed_line_count(&counted), 3);
    }

    #[test]
    fn quoted_file_headers_fail_review_preparation() {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.chunk_bytes = 10_000;
        config.review.max_diff_bytes = 10_000;
        let raw = "diff --git a/docs/a.md b/docs/a.md\n@@ -1 +1 @@\n-a\n+b\ndiff --git \"a/src/payments/\\303\\247.rs\" \"b/src/payments/\\303\\247.rs\"\n@@ -1 +1 @@\n-a\n+b\n";
        let error = crate::review_input::prepare(&config, raw, "head".into(), "pr").unwrap_err();
        assert!(
            error.to_string().contains("malformed file headers"),
            "{error}"
        );
    }

    #[test]
    fn any_finding_at_or_above_threshold_blocks_and_unknown_severity_fails_closed() {
        let low = evaluate(
            &policy(),
            false,
            &output(
                vec![file("docs/a.md", "docs/a.md", 1)],
                vec![finding("low")],
            ),
        )
        .unwrap();
        assert!(!low.approved);
        let mut tolerant = policy();
        tolerant.block_on = Severity::High;
        assert!(
            evaluate(
                &tolerant,
                false,
                &output(
                    vec![file("docs/a.md", "docs/a.md", 1)],
                    vec![finding("low")]
                )
            )
            .unwrap()
            .approved
        );
        assert!(
            !evaluate(
                &tolerant,
                false,
                &output(
                    vec![file("docs/a.md", "docs/a.md", 1)],
                    vec![finding("critical")]
                )
            )
            .unwrap()
            .approved
        );
        assert!(
            !evaluate(
                &tolerant,
                false,
                &output(
                    vec![file("docs/a.md", "docs/a.md", 1)],
                    vec![finding("bogus")]
                )
            )
            .unwrap()
            .approved
        );
    }

    #[test]
    fn size_coverage_ignored_files_and_analysis_fail_closed() {
        let files = (0..4)
            .map(|n| file(&format!("docs/{n}.md"), &format!("docs/{n}.md"), 3))
            .collect();
        let mut out = output(files, vec![]);
        out.review.files_reviewed = 2;
        out.ignored_paths = vec!["Cargo.lock".into()];
        out.analysis.status = "failed".into();
        let decision = evaluate(&policy(), true, &out).unwrap();
        let codes: Vec<String> = decision
            .vetoes
            .iter()
            .map(|veto| {
                serde_json::to_value(veto).unwrap()["code"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        for expected in [
            "not_reviewed",
            "too_many_files",
            "too_many_lines",
            "incomplete_coverage",
            "analysis_not_passed",
        ] {
            assert!(
                codes.contains(&expected.to_string()),
                "{expected}: {codes:?}"
            );
        }
        // Disabled analysis is not a veto.
        let mut clean = output(vec![file("docs/a.md", "docs/a.md", 1)], vec![]);
        clean.analysis.status = "disabled".into();
        assert!(evaluate(&policy(), false, &clean).unwrap().approved);
    }

    #[test]
    fn explanations_are_localized_and_outputs_are_machine_readable() {
        let mut decision = evaluate(
            &policy(),
            false,
            &output(
                vec![file("src/payments/x.rs", "src/payments/x.rs", 1)],
                vec![finding("high")],
            ),
        )
        .unwrap();
        decision.refuse("HTTP 422");
        let pt = render_notice(&decision, "pt-BR");
        assert!(pt.starts_with(NOTICE_MARKER));
        assert!(pt.contains("Precisa de review humano") && pt.contains("caminho protegido"));
        assert!(pt.contains("Allow GitHub Actions to create and approve pull requests"));
        let en = render_section(&decision, "en-US");
        assert!(en.contains("Needs human review") && en.contains("protected path"));
        let reasons: serde_json::Value =
            serde_json::from_str(&reasons_json(&decision, "en")).unwrap();
        assert_eq!(reasons[0]["code"], "blocked_path");
        assert!(
            reasons[0]["message"]
                .as_str()
                .unwrap()
                .contains("protected path")
        );
        assert!(
            reasons
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["code"] == "approval_not_permitted")
        );
        let approved = evaluate(
            &policy(),
            false,
            &output(vec![file("docs/a.md", "docs/a.md", 1)], vec![]),
        )
        .unwrap();
        assert!(render_approval(&approved, "pt-BR").contains("Auto-aprovado"));
        assert!(render_approval(&approved, "en").contains("head=head"));
    }
}
