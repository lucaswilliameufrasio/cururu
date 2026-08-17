use std::fmt::Write;

use crate::agent::ReviewFinding;
use crate::provider::ProviderUsage;
use crate::review::ReviewOutput;

const MARKER: &str = "<!-- cururu:summary -->";
const FINDING_MARKER: &str = "<!-- cururu:finding -->";

pub const fn marker() -> &'static str {
    MARKER
}

pub const fn finding_marker() -> &'static str {
    FINDING_MARKER
}

const CURURU_SIGNATURE: &str = "\
> _Cururu_ — revisão automatizada por IA. Trate como auxiliar; não substitui
> uma revisão humana.
>
> ```
>      _   _
>     (o)_(o)
>      (_) \\
>     /   \\ \\
>    |_____|_|
>    cururu
> ```";

/// Branded footer appended to every Cururu comment so the bot is recognizable
/// regardless of which GitHub identity runs the workflow.
pub fn render_signature() -> String {
    let mut out = String::new();
    out.push_str("\n\n---\n\n");
    out.push_str(CURURU_SIGNATURE);
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

    out.push_str(&render_signature());
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

    if output.show_usage
        && let Some(ref usage) = output.usage
    {
        render_usage(usage, output.show_cost, out);
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
    let _ = write!(out, "**{}:** {title}", f.severity.to_uppercase());
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

    if show_cost && let Some(cost) = usage.cost {
        let _ = write!(out, "**Cost:** `${cost:.6}`\n\n");
    }
}

fn render_finding_row(f: &ReviewFinding) -> String {
    let line_str = f.line.map_or_else(|| "-".to_string(), |v| v.to_string());
    format!(
        "| {} | `{}` | {} | {} | {} |\n",
        escape_md(&f.severity),
        escape_md(&f.path),
        line_str,
        escape_md(&f.message),
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
        }
    }

    #[test]
    fn inline_finding_contains_marker_and_fields() {
        let body = render_inline_finding(&finding());
        assert!(body.contains("<!-- cururu:finding -->"));
        assert!(body.contains("**CRITICAL:** Command injection"));
        assert!(body.contains("Query is interpolated"));
        assert!(body.contains("**Sugestão:** Use Command::new"));
    }

    #[test]
    fn inline_finding_handles_empty_title() {
        let mut f = finding();
        f.title = String::new();
        let body = render_inline_finding(&f);
        assert!(body.contains("**CRITICAL:** Finding"));
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
            changed_files: vec![],
            head_sha: "head".into(),
        };
        let body = render_summary_comment(&output);
        assert!(body.contains("_Cururu_"));
    }
}
