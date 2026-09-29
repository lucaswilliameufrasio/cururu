use std::fmt::Write;

use crate::provider::ProviderUsage;
use crate::review::ReviewOutput;
use crate::{agent::ReviewFinding, config::Severity};

const MARKER: &str = "<!-- cururu:summary -->";
const FINDING_MARKER: &str = "<!-- cururu:finding -->";

pub const fn marker() -> &'static str {
    MARKER
}

pub const fn finding_marker() -> &'static str {
    FINDING_MARKER
}

/// Branded footer appended to every Cururu comment so the automation is recognizable
/// regardless of which provider identity publishes it.
pub fn render_signature(logo_url: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str("\n\n---\n\n");
    out.push_str("> _Cururu_ — revisão automatizada por IA. Trate como auxiliar; não substitui\n");
    out.push_str("> uma revisão humana.\n>\n");
    if let Some(url) = logo_url {
        let _ = writeln!(out, "> ![Cururu](<{url}>)");
    } else {
        out.push_str("> ```\n>      _   _\n>     (o)_(o)\n>      (_) \\\n>     /   \\ \\\n>    |_____|_|\n>    cururu\n> ```");
    }
    out.push('\n');
    out
}

pub fn render_summary_comment(output: &ReviewOutput) -> String {
    let mut out = String::new();
    out.push_str(MARKER);
    let _ = writeln!(out, "<!-- cururu:state:v1 head={} -->", output.head_sha);
    out.push_str("\n## 🐸 Cururu review\n\n");

    render_header(output, &mut out);

    if output.review.findings.is_empty() {
        out.push_str("No high-confidence issues found.\n");
    } else {
        out.push_str("| Severity | File | Line | Finding | Suggestion |\n");
        out.push_str("|---|---|---:|---|---|\n");
        for finding in &output.review.findings {
            out.push_str(&render_finding_row(finding));
        }
    }

    out.push_str(&render_signature(output.logo_url.as_deref()));
    out
}

fn render_header(output: &ReviewOutput, out: &mut String) {
    let _ = writeln!(out, "**Model:** `{}`  ", output.model);
    let _ = writeln!(
        out,
        "**Files reviewed:** `{}`  ",
        output.review.files_reviewed
    );
    let _ = write!(out, "**Findings:** `{}`\n\n", output.review.findings.len());

    if !output.context_files.is_empty() {
        let _ = write!(
            out,
            "**Context files:** `{}`\n\n",
            output.context_files.join(", ")
        );
    }

    if output.analysis.status != "disabled" {
        let _ = writeln!(out, "**Analyzer status:** `{}`  ", output.analysis.status);
        if !output.analysis.tools.is_empty() {
            let _ = writeln!(
                out,
                "**Analyzer tools:** {}  ",
                output
                    .analysis
                    .tools
                    .iter()
                    .map(|tool| format!("`{}`: `{}`", tool.name, tool.status))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    if let Some(ref usage) = output.usage {
        if output.show_usage {
            render_usage(usage, output.show_cost, out);
        } else if output.show_cost {
            render_cost(usage.cost, out);
        }
    }
}

/// Body for a single inline review comment anchored to a diff line.
pub fn render_inline_finding(f: &ReviewFinding) -> String {
    let title = if f.title.trim().is_empty() {
        "Finding".to_string()
    } else {
        f.title.trim().to_string()
    };

    let mut out = String::new();
    out.push_str(FINDING_MARKER);
    out.push_str("\n\n");
    let severity = Severity::from_name(&f.severity).map_or_else(
        || "Finding".to_string(),
        |severity| severity.as_str().to_uppercase(),
    );
    let _ = write!(out, "**{severity}**: {title}");
    out.push('\n');
    let message = f.message.trim().replace('\n', " ");
    let suggestion = f.suggestion.trim().replace('\n', " ");
    if !message.is_empty() {
        let _ = write!(out, "\n{message}");
    }
    if !suggestion.is_empty() {
        let _ = write!(out, "\n\n> **Sugestão:** {suggestion}");
    }
    if let Some(change) = &f.suggested_change
        && !change.replacement.is_empty()
        && !change.replacement.contains('\n')
        && !change.replacement.contains("```")
    {
        let _ = write!(out, "\n\n```suggestion\n{}\n```", change.replacement);
    }
    out
}

fn render_usage(usage: &ProviderUsage, show_cost: bool, out: &mut String) {
    out.push_str("**Tokens:** ");
    let _ = write!(
        out,
        "{} prompt + {} completion = {} total",
        usage.prompt_tokens, usage.completion_tokens, usage.total_tokens,
    );
    if usage.cached_tokens > 0 {
        let _ = write!(out, " ({} cached)", usage.cached_tokens);
    }
    if usage.reasoning_tokens > 0 {
        let _ = write!(out, " ({} reasoning)", usage.reasoning_tokens);
    }
    out.push_str("  \n");

    if show_cost {
        render_cost(usage.cost, out);
    }
}

fn render_cost(cost: Option<f64>, out: &mut String) {
    match cost {
        Some(cost) => {
            let _ = write!(out, "**Provider-reported cost:** `${cost:.6}`\n\n");
        }
        None => out.push_str("**Cost:** unavailable from the LLM provider.\n\n"),
    }
}

fn render_finding_row(f: &ReviewFinding) -> String {
    let line_str = f.line.map_or_else(|| "-".to_string(), |v| v.to_string());
    let title = if f.title.trim().is_empty() {
        f.message.clone()
    } else {
        format!("{}: {}", f.title.trim(), f.message.trim())
    };
    format!(
        "| {} | `{}` | {} | {} | {} |\n",
        escape_md(Severity::from_name(&f.severity).map_or("finding", Severity::as_str)),
        escape_md(&f.path),
        line_str,
        escape_md(&title),
        escape_md(&f.suggestion),
    )
}

fn escape_md(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ReviewFinding;

    fn finding() -> ReviewFinding {
        ReviewFinding {
            severity: "critical".into(),
            path: "src/main.rs".into(),
            line: Some(65),
            title: "Command injection".into(),
            message: "Query is interpolated into sh -c.".into(),
            suggestion: "Use Command::new with args.".into(),
            confidence: 0.9,
            suggested_change: None,
            source: None,
            rule: None,
        }
    }

    #[test]
    fn inline_finding_contains_marker_and_fields() {
        let body = render_inline_finding(&finding());
        assert!(body.contains("<!-- cururu:finding -->"));
        assert!(body.starts_with("<!-- cururu:finding -->\n\n**CRITICAL**:"));
        assert!(body.contains("**CRITICAL**: Command injection"));
        assert!(body.contains("Query is interpolated"));
        assert!(body.contains("**Sugestão:** Use Command::new"));
    }

    #[test]
    fn inline_finding_does_not_publish_untrusted_severity_placeholder() {
        let mut finding = finding();
        finding.severity = "<LEVEL>".into();

        let body = render_inline_finding(&finding);

        assert!(!body.contains("<LEVEL>"));
        assert!(body.contains("**Finding**: Command injection"));
    }

    #[test]
    fn summary_row_does_not_publish_untrusted_severity_placeholder() {
        let mut finding = finding();
        finding.severity = "<LEVEL>".into();

        let row = render_finding_row(&finding);

        assert!(!row.contains("<LEVEL>"));
        assert!(row.starts_with("| finding |"));
    }

    #[test]
    fn inline_finding_handles_empty_title() {
        let mut f = finding();
        f.title = String::new();
        let body = render_inline_finding(&f);
        assert!(body.contains("**CRITICAL**: Finding"));
    }

    #[test]
    fn inline_finding_has_no_signature() {
        let body = render_inline_finding(&finding());
        assert!(!body.contains("_Cururu_"));
    }

    #[test]
    fn inline_finding_renders_safe_suggested_change() {
        let mut f = finding();
        f.suggested_change = Some(crate::agent::SuggestedChange {
            replacement: "use std::process::Command;".into(),
        });
        let body = render_inline_finding(&f);
        assert!(body.contains("```suggestion"));
        assert!(body.contains("use std::process::Command;"));
    }

    #[test]
    fn inline_finding_ignores_multiline_suggested_change() {
        let mut f = finding();
        f.suggested_change = Some(crate::agent::SuggestedChange {
            replacement: "first\nsecond".into(),
        });
        let body = render_inline_finding(&f);
        assert!(!body.contains("```suggestion"));
    }

    #[test]
    fn signature_is_appended_to_summary() {
        let output = crate::review::ReviewOutput {
            review: crate::agent::ReviewResult {
                model: "m".into(),
                files_reviewed: 1,
                summary: "s".into(),
                findings: vec![],
            },
            usage: None,
            context_files: vec![],
            model: "m".into(),
            show_usage: false,
            show_cost: false,
            logo_url: Some("https://example.test/cururu.svg".into()),
            changed_files: vec![],
            head_sha: "head".into(),
            analysis: crate::analysis::AnalysisReport {
                status: "disabled".into(),
                tools: vec![],
                findings: vec![],
            },
        };
        let body = render_summary_comment(&output);
        assert!(body.contains("<!-- cururu:state:v1 head=head -->"));
        assert!(body.contains("_Cururu_"));
        assert!(body.contains("![Cururu](<https://example.test/cururu.svg>)"));
        assert!(!body.contains("(o)_(o)"));
    }

    #[test]
    fn cost_is_rendered_even_when_token_usage_is_disabled() {
        let mut output = crate::review::ReviewOutput {
            review: crate::agent::ReviewResult {
                model: "m".into(),
                files_reviewed: 1,
                summary: "s".into(),
                findings: vec![],
            },
            usage: Some(ProviderUsage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
                cached_tokens: 0,
                reasoning_tokens: 0,
                cost: Some(0.123_456),
            }),
            context_files: vec![],
            model: "m".into(),
            show_usage: false,
            show_cost: true,
            logo_url: None,
            changed_files: vec![],
            head_sha: "head".into(),
            analysis: crate::analysis::AnalysisReport {
                status: "disabled".into(),
                tools: vec![],
                findings: vec![],
            },
        };
        let body = render_summary_comment(&output);
        assert!(body.contains("Provider-reported cost"));
        assert!(body.contains("0.123456"));
        assert!(!body.contains("**Tokens:**"));

        output.usage.as_mut().unwrap().cost = None;
        let body = render_summary_comment(&output);
        assert!(body.contains("Cost:** unavailable from the LLM provider"));
    }
}
