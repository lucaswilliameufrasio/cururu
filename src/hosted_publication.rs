//! Hosted publication candidates and their approval boundary.
//!
//! A hosted review (GitHub App or Action) no longer publishes directly. It
//! renders a candidate: the exact review, inline comments and summary that
//! would be posted. The candidate is shown for inspection as a canonical
//! preview, and its digest is the SHA-256 of that preview. Every approval
//! surface names the digest, so an approval authorizes only the content the
//! operator inspected. Publication rechecks the head commit and the trusted
//! configuration before the first write.

use crate::config::AppConfig;
use crate::review::{self, ReviewOutput};
use crate::scm::{ReviewCommentDraft, ScmProvider};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// GitHub limits check-run text to 65 535 characters. A candidate whose
/// preview does not fit cannot be inspected completely, so it is not approvable.
pub const MAX_PREVIEW_BYTES: usize = 65_000;
pub const CHECK_RUN_NAME: &str = "Cururu review";
pub const PUBLISH_ACTION_ID: &str = "cururu_publish";
pub const APPROVAL_COMMAND: &str = "/cururu approve";
pub const ACTION_APPROVAL_PREFIX: &str = "cururu-candidate:";
pub const DEFAULT_APPROVAL_ENVIRONMENT: &str = "cururu-publish";
const PREVIEW_MARKER: &str = "<!-- cururu:candidate:v1 -->";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostedCandidate {
    pub version: u32,
    pub language: String,
    pub repository: String,
    pub pr_number: u64,
    pub head_sha: String,
    pub config_fingerprint: String,
    /// Whether publication reconciles Cururu's inline comments for the head.
    pub reconcile_inline: bool,
    pub inline_comments: Vec<ReviewCommentDraft>,
    pub formal_review_body: Option<String>,
    pub summary: String,
    pub quality_gate_passed: bool,
    pub findings_count: usize,
}

impl HostedCandidate {
    pub fn from_review(
        config: &AppConfig,
        result: &ReviewOutput,
        inline_comments: Vec<ReviewCommentDraft>,
        reconcile_inline: bool,
        formal_review: bool,
    ) -> Self {
        let report = crate::quality::evaluate(&result.review, config.review.policy.fail_on);
        let formal_review_body = formal_review.then(|| {
            format!(
                "{}\n\nCururu has completed an automated review. See the Cururu summary and inline findings in the conversation. This review is advisory and does not replace human review.",
                formal_review_marker(&result.head_sha)
            )
        });
        // GitHub may normalize carriage returns in check text, which would
        // break digest verification of the inspected preview.
        let inline_comments = inline_comments
            .into_iter()
            .map(|comment| ReviewCommentDraft {
                body: comment.body.replace('\r', ""),
                ..comment
            })
            .collect();
        Self {
            version: 1,
            language: config.review.language.clone(),
            repository: config.source_control.repository.clone(),
            pr_number: config.source_control.change_request_number,
            head_sha: result.head_sha.clone(),
            config_fingerprint: config_fingerprint(config),
            reconcile_inline,
            inline_comments,
            formal_review_body,
            summary: crate::output::render_summary_comment(result).replace('\r', ""),
            quality_gate_passed: report.passed,
            findings_count: report.findings_count,
        }
    }

    fn portuguese(&self) -> bool {
        self.language.to_ascii_lowercase().starts_with("pt")
    }

    /// The canonical, complete rendering of everything this candidate publishes.
    pub fn preview(&self) -> String {
        let pt = self.portuguese();
        let mut out = String::new();
        out.push_str(PREVIEW_MARKER);
        out.push('\n');
        let _ = writeln!(
            out,
            "{} `{}` #{} @ `{}`",
            if pt {
                "Candidato para"
            } else {
                "Candidate for"
            },
            self.repository,
            self.pr_number,
            self.head_sha
        );
        let _ = writeln!(
            out,
            "{}: `{}`\n",
            if pt {
                "Configuração"
            } else {
                "Configuration"
            },
            self.config_fingerprint
        );
        let _ = writeln!(
            out,
            "## {}\n\n{}\n",
            if pt {
                "Comentário de resumo"
            } else {
                "Summary comment"
            },
            self.summary.trim_end()
        );
        if self.reconcile_inline {
            let _ = writeln!(
                out,
                "## {} ({})\n",
                if pt {
                    "Comentários inline"
                } else {
                    "Inline comments"
                },
                self.inline_comments.len()
            );
            for (index, comment) in self.inline_comments.iter().enumerate() {
                let location = comment.line.map_or_else(
                    || comment.path.clone(),
                    |line| format!("{}:{line}", comment.path),
                );
                let _ = writeln!(
                    out,
                    "### {}. `{}`\n\n{}\n",
                    index + 1,
                    location.replace('`', "'"),
                    comment.body.trim_end()
                );
            }
        }
        if let Some(body) = &self.formal_review_body {
            let _ = writeln!(
                out,
                "## {}\n\n{}\n",
                if pt { "Review formal" } else { "Formal review" },
                body.trim_end()
            );
        }
        out
    }

    pub fn digest(&self) -> String {
        preview_digest(&self.preview())
    }

    pub fn approvable(&self) -> bool {
        self.preview().len() <= MAX_PREVIEW_BYTES
    }

    /// Short instructions shown next to the preview; not part of the digest.
    pub fn approval_instructions(&self, hosted_app: bool) -> String {
        let digest = self.digest();
        let pt = self.portuguese();
        if !self.approvable() {
            return if pt {
                format!(
                    "Este candidato excede o limite de inspeção do GitHub ({MAX_PREVIEW_BYTES} bytes) e não pode ser aprovado. Nada foi publicado. Reduza o escopo do PR ou a quantidade de achados e revise novamente."
                )
            } else {
                format!(
                    "This candidate exceeds GitHub's inspection limit ({MAX_PREVIEW_BYTES} bytes) and cannot be approved. Nothing was published. Reduce the PR or finding scope and review again."
                )
            };
        }
        match (hosted_app, pt) {
            (true, true) => format!(
                "Nada foi publicado no PR. Inspecione o candidato abaixo e aprove **somente este conteúdo** por uma destas formas:\n\n- botão **Publicar review** neste check;\n- comentário `{APPROVAL_COMMAND} {digest}` de alguém com permissão de escrita;\n- `cururu approve-hosted-review --pr {}` na CLI.\n\nNovos commits ou mudança na configuração confiável invalidam a aprovação.",
                self.pr_number
            ),
            (true, false) => format!(
                "Nothing was published to the PR. Inspect the candidate below and approve **only this content** in one of these ways:\n\n- the **Publish review** button on this check;\n- a `{APPROVAL_COMMAND} {digest}` comment by someone with write access;\n- `cururu approve-hosted-review --pr {}` from the CLI.\n\nNew commits or a trusted configuration change invalidate the approval.",
                self.pr_number
            ),
            (false, true) => format!(
                "Nada foi publicado no PR. Inspecione o candidato abaixo e aprove o job de publicação no Environment protegido do workflow, ou use `cururu approve-hosted-review --pr {}` na CLI. Digest: `{digest}`.",
                self.pr_number
            ),
            (false, false) => format!(
                "Nothing was published to the PR. Inspect the candidate below and approve the publication job in the workflow's protected Environment, or use `cururu approve-hosted-review --pr {}` from the CLI. Digest: `{digest}`.",
                self.pr_number
            ),
        }
    }
}

pub fn preview_digest(preview: &str) -> String {
    format!("{:x}", Sha256::digest(preview.as_bytes()))
}

pub fn formal_review_marker(head_sha: &str) -> String {
    format!("<!-- cururu:formal-review:v1 head={head_sha} -->")
}

/// Fingerprint of the trusted repository and shared configuration content.
/// The base commit is excluded so that unrelated base-branch commits do not
/// invalidate a candidate whose configuration is unchanged.
pub fn config_fingerprint(config: &AppConfig) -> String {
    let local = config
        .trusted_config_snapshot
        .as_ref()
        .and_then(|(_, hash)| hash.clone())
        .unwrap_or_else(|| "none".into());
    let shared = config
        .shared_config_snapshot
        .as_ref()
        .map_or_else(|| "none".into(), |shared| shared.hash.clone());
    preview_digest(&format!("local={local}\nshared={shared}\n"))
}

/// Parse `/cururu approve <digest>`; the full 64-character digest is required.
pub fn parse_approval_command(body: &str) -> Option<String> {
    let rest = body.trim().strip_prefix(APPROVAL_COMMAND)?;
    let digest = rest.trim();
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

/// Recheck that publishing this candidate is still valid. No write happens here.
pub async fn ensure_candidate_current(
    candidate: &HostedCandidate,
    current: &AppConfig,
    source_control: &dyn ScmProvider,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        candidate.repository == current.source_control.repository
            && candidate.pr_number == current.source_control.change_request_number,
        "candidate does not belong to this pull request"
    );
    anyhow::ensure!(
        candidate.config_fingerprint == config_fingerprint(current),
        "trusted configuration changed after the candidate was prepared; review again"
    );
    review::ensure_review_head_unchanged(
        &candidate.head_sha,
        &source_control.fetch_head_sha().await?,
    )?;
    Ok(())
}

/// Publish exactly the candidate's content. Callers must have verified the
/// approval and called [`ensure_candidate_current`]. A failure after the first
/// write leaves an uncertain partial publication and must not be retried
/// automatically.
pub async fn publish_candidate(
    candidate: &HostedCandidate,
    source_control: &dyn ScmProvider,
    bot_login: Option<&str>,
) -> anyhow::Result<()> {
    let head = candidate.head_sha.as_str();
    if candidate.reconcile_inline {
        source_control
            .reconcile_review_comments(head, &candidate.inline_comments)
            .await?;
    }
    if let (Some(body), Some(login)) = (&candidate.formal_review_body, bot_login) {
        let marker = formal_review_marker(head);
        if !source_control.formal_review_exists(&marker, login).await? {
            let review_id = source_control
                .create_pending_formal_review(head, body)
                .await?;
            let current = match source_control.fetch_head_sha().await {
                Ok(current) => current,
                Err(error) => {
                    source_control
                        .delete_pending_formal_review(review_id)
                        .await?;
                    return Err(error).context("failed to verify PR head before submitting review");
                }
            };
            if let Err(error) = review::ensure_review_head_unchanged(head, &current) {
                source_control
                    .delete_pending_formal_review(review_id)
                    .await?;
                return Err(error.into());
            }
            source_control.submit_formal_review(review_id).await?;
        }
    }
    review::ensure_review_head_unchanged(head, &source_control.fetch_head_sha().await?)?;
    source_control
        .upsert_summary_comment(&candidate.summary)
        .await?;
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sample(language: &str) -> HostedCandidate {
        HostedCandidate {
            version: 1,
            language: language.into(),
            repository: "owner/repo".into(),
            pr_number: 7,
            head_sha: "head".into(),
            config_fingerprint: "fingerprint".into(),
            reconcile_inline: true,
            inline_comments: vec![ReviewCommentDraft {
                path: "src/lib.rs".into(),
                line: Some(3),
                body: "Finding body".into(),
            }],
            formal_review_body: Some(formal_review_marker("head")),
            summary: "<!-- cururu:summary -->\nSummary".into(),
            quality_gate_passed: true,
            findings_count: 1,
        }
    }

    #[test]
    fn digest_covers_every_published_part() {
        let base = sample("en-US");
        let digest = base.digest();
        assert_eq!(digest.len(), 64);
        let mut changed = base.clone();
        changed.inline_comments[0].body.push('!');
        assert_ne!(changed.digest(), digest);
        let mut changed = base.clone();
        changed.inline_comments[0].line = Some(4);
        assert_ne!(changed.digest(), digest);
        let mut changed = base.clone();
        changed.summary.push('!');
        assert_ne!(changed.digest(), digest);
        let mut changed = base.clone();
        changed.head_sha = "other".into();
        assert_ne!(changed.digest(), digest);
        let mut changed = base.clone();
        changed.config_fingerprint = "other".into();
        assert_ne!(changed.digest(), digest);
        let mut changed = base.clone();
        changed.formal_review_body = None;
        assert_ne!(changed.digest(), digest);
        assert_eq!(preview_digest(&base.preview()), digest);
    }

    #[test]
    fn preview_and_instructions_are_localized_and_size_bounded() {
        let english = sample("en-US");
        let portuguese = sample("pt-BR");
        assert!(english.preview().contains("Inline comments (1)"));
        assert!(portuguese.preview().contains("Comentários inline (1)"));
        assert!(
            english
                .approval_instructions(true)
                .contains(&english.digest())
        );
        assert!(
            portuguese
                .approval_instructions(false)
                .contains("Environment")
        );
        let mut large = sample("en-US");
        large.summary = "x".repeat(MAX_PREVIEW_BYTES);
        assert!(!large.approvable());
        assert!(
            large
                .approval_instructions(true)
                .contains("cannot be approved")
        );
    }

    #[test]
    fn approval_command_requires_full_digest() {
        let digest = "a".repeat(64);
        assert_eq!(
            parse_approval_command(&format!("/cururu approve {digest}\n")),
            Some(digest.clone())
        );
        assert_eq!(parse_approval_command("/cururu approve abc"), None);
        assert_eq!(
            parse_approval_command(&format!("please /cururu approve {digest}")),
            None
        );
        assert_eq!(
            parse_approval_command(&format!("/cururu approve {digest} x")),
            None
        );
    }
}
