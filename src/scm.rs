use async_trait::async_trait;
use serde::Deserialize;

pub use cururu_core::{
    FindingAnnotation, PriorReviewComment, PriorReviewFeedback, ReviewCommentDraft,
};

#[derive(Debug, Deserialize)]
pub struct ScmIdentity {
    pub login: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
    pub body: Option<String>,
    pub user: Option<ScmIdentity>,
    pub path: String,
    pub line: Option<u32>,
    #[allow(dead_code)]
    pub subject_type: Option<String>,
}

/// Source-control capabilities required by Cururu's core. Concrete adapters
/// translate native change-request APIs and events into these domain values.
#[async_trait]
pub trait ScmProvider: Send + Sync {
    async fn fetch_head_sha(&self) -> anyhow::Result<String>;
    async fn fetch_diff_with_limit(&self, max_bytes: usize) -> anyhow::Result<(String, bool)>;
    async fn fetch_base_sha(&self) -> anyhow::Result<String> {
        anyhow::bail!("SCM adapter does not support base revisions")
    }
    async fn fetch_file_at_ref(&self, _path: &str, _revision: &str) -> anyhow::Result<String> {
        anyhow::bail!("SCM adapter does not support repository file access")
    }
    async fn list_repository_paths_at_revision(
        &self,
        revision: &str,
    ) -> anyhow::Result<Vec<String>> {
        let _ = revision;
        anyhow::bail!("SCM adapter does not support repository tree access")
    }
    async fn fetch_prior_review_feedback(&self) -> anyhow::Result<Vec<PriorReviewFeedback>> {
        Ok(Vec::new())
    }
    async fn list_finding_annotations(
        &self,
        _revision: &str,
        _source_names: &[String],
    ) -> anyhow::Result<Vec<FindingAnnotation>> {
        Ok(Vec::new())
    }
    async fn list_review_comments(&self) -> anyhow::Result<Vec<ReviewComment>> {
        anyhow::bail!("SCM adapter does not support review-comment history")
    }
    async fn reconcile_review_comments(
        &self,
        _revision: &str,
        _desired: &[ReviewCommentDraft],
    ) -> anyhow::Result<()> {
        anyhow::bail!("SCM adapter does not support inline review comments")
    }
    async fn has_summary_for_revision(&self, _revision: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn upsert_summary_comment(&self, _body: &str) -> anyhow::Result<()> {
        anyhow::bail!("SCM adapter does not support summary comments")
    }
    async fn create_issue_comment(&self, _body: &str) -> anyhow::Result<()> {
        anyhow::bail!("SCM adapter does not support issue comments")
    }
    async fn reply_review_comment(&self, _comment_id: u64, _body: &str) -> anyhow::Result<()> {
        anyhow::bail!("SCM adapter does not support discussion replies")
    }
    async fn user_can_review(&self, _login: &str) -> anyhow::Result<bool> {
        anyhow::bail!("SCM adapter does not support collaborator authorization")
    }
    async fn request_cururu_as_reviewer(&self, _login: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn formal_review_exists(&self, _marker: &str, _login: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn create_pending_formal_review(
        &self,
        _revision: &str,
        _body: &str,
    ) -> anyhow::Result<u64> {
        anyhow::bail!("SCM adapter does not support formal reviews")
    }
    async fn delete_pending_formal_review(&self, _review_id: u64) -> anyhow::Result<()> {
        anyhow::bail!("SCM adapter does not support formal review cleanup")
    }
    async fn submit_formal_review(&self, _review_id: u64) -> anyhow::Result<()> {
        anyhow::bail!("SCM adapter does not support formal reviews")
    }
    async fn fetch_config_toml(&self, _revision: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    async fn fetch_repository_file_at_ref(
        &self,
        _repository: &str,
        _path: &str,
        _revision: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("SCM adapter does not support shared configuration files")
    }
    fn is_cururu_review_comment(&self, _comment: &ReviewComment, _login: Option<&str>) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{FindingAnnotation, ScmProvider};
    use async_trait::async_trait;

    struct MinimalScm;

    #[async_trait]
    impl ScmProvider for MinimalScm {
        async fn fetch_head_sha(&self) -> anyhow::Result<String> {
            Ok("revision".into())
        }

        async fn fetch_diff_with_limit(&self, _max_bytes: usize) -> anyhow::Result<(String, bool)> {
            Ok((String::new(), false))
        }
    }

    #[test]
    fn annotation_domain_type_contains_no_provider_specific_field_names() {
        let annotation = FindingAnnotation {
            path: "src/lib.rs".into(),
            line: Some(7),
            severity: "high".into(),
            title: Some("safe-title".into()),
            message: "diagnostic".into(),
            details: None,
        };
        assert_eq!(annotation.line, Some(7));
        assert_eq!(annotation.severity, "high");
    }

    #[tokio::test]
    async fn basic_adapter_can_omit_optional_capabilities() {
        let adapter = MinimalScm;
        assert!(
            adapter
                .fetch_prior_review_feedback()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            adapter
                .list_finding_annotations("revision", &[])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            adapter
                .upsert_summary_comment("summary")
                .await
                .unwrap_err()
                .to_string()
                .contains("does not support summary comments")
        );
    }
}
