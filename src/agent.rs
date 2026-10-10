use crate::config::{LlmConfig, ReviewPolicy, Severity};
use crate::provider::ProviderUsage;
#[cfg(test)]
use cururu_core::deduplicate_review_findings;
use cururu_core::{CandidateOptions, collect_review_candidates, sort_review_findings};
#[allow(unused_imports)]
pub use cururu_core::{ReviewFinding, ReviewResult, SuggestedChange};
pub use cururu_engine::{ChunkResult, InvalidReviewOutput, ReviewAgent};
use cururu_engine::{OpenAiCompatibleAgent, OpenAiCompatibleSettings};

pub fn build_agent(config: &LlmConfig, prompt: String) -> anyhow::Result<Box<dyn ReviewAgent>> {
    let settings = OpenAiCompatibleSettings {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        temperature: config.temperature,
        max_output_tokens: config.max_output_tokens,
    };
    Ok(Box::new(OpenAiCompatibleAgent::new(settings, prompt)?))
}

/// Answer an authorized conversation request via the configured LLM adapter.
pub async fn answer_conversation(
    config: &LlmConfig,
    tone: &str,
    technical_level: &str,
    question: &str,
    context: &str,
) -> anyhow::Result<String> {
    build_agent(config, String::new())?
        .answer_question(tone, technical_level, question, context)
        .await
}

pub async fn recommend_after_truncation(
    config: &LlmConfig,
    finish_reason: &str,
) -> anyhow::Result<(String, Option<ProviderUsage>)> {
    let settings = OpenAiCompatibleSettings {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        temperature: config.temperature,
        max_output_tokens: config.max_output_tokens,
    };
    OpenAiCompatibleAgent::new(settings, String::new())?
        .recommend_after_truncation(finish_reason)
        .await
}

#[cfg(test)]
pub fn merge_results(
    model: String,
    files_reviewed: usize,
    results: Vec<ChunkResult>,
    policy: &ReviewPolicy,
    additional_findings: Vec<ReviewFinding>,
) -> ReviewResult {
    apply_policy(
        collect_candidates(model, files_reviewed, results, policy, additional_findings),
        policy,
    )
}

pub fn collect_candidates(
    model: String,
    files_reviewed: usize,
    results: Vec<ChunkResult>,
    policy: &ReviewPolicy,
    additional_findings: Vec<ReviewFinding>,
) -> ReviewResult {
    collect_review_candidates(
        model,
        files_reviewed,
        results.into_iter().map(|result| result.review).collect(),
        additional_findings,
        CandidateOptions {
            include_suggested_changes: policy.suggested_changes,
            synthesize: policy.synthesis,
        },
    )
}

pub fn apply_policy(mut candidates: ReviewResult, policy: &ReviewPolicy) -> ReviewResult {
    candidates.findings.retain(|f| {
        f.confidence.is_finite()
            && f.confidence >= policy.minimum_confidence
            && Severity::from_name(&f.severity)
                .is_some_and(|severity| policy.allowed_severities.contains(&severity))
    });
    sort_review_findings(&mut candidates.findings);
    candidates.findings.truncate(policy.max_findings);
    candidates.summary = format!(
        "Found {} high-confidence review finding(s).",
        candidates.findings.len()
    );
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use cururu_core::DiffChunk;

    #[test]
    fn suggested_change_accepts_legacy_string_and_canonical_object() {
        let legacy: ReviewFinding = serde_json::from_str(
            r#"{"severity":"high","path":"src/lib.rs","line":1,"title":"x","message":"x","suggestion":"x","confidence":0.9,"suggested_change":"  safe_call()  "}"#,
        )
        .unwrap();
        assert_eq!(
            legacy.suggested_change.as_ref().unwrap().replacement,
            "  safe_call()  "
        );

        let canonical: ReviewFinding = serde_json::from_str(
            r#"{"severity":"high","path":"src/lib.rs","line":1,"title":"x","message":"x","suggestion":"x","confidence":0.9,"suggested_change":{"replacement":"safe_call()"}}"#,
        )
        .unwrap();
        assert_eq!(
            canonical.suggested_change.as_ref().unwrap().replacement,
            "safe_call()"
        );

        let absent: ReviewFinding = serde_json::from_str(
            r#"{"severity":"high","path":"src/lib.rs","line":1,"title":"x","message":"x","suggestion":"x","confidence":0.9,"suggested_change":null}"#,
        )
        .unwrap();
        assert!(absent.suggested_change.is_none());

        let invalid = serde_json::from_str::<ReviewFinding>(
            r#"{"severity":"high","path":"src/lib.rs","line":1,"title":"x","message":"x","suggestion":"x","confidence":0.9,"suggested_change":true}"#,
        );
        assert!(invalid.is_err());
    }

    #[test]
    fn suggested_change_serializes_to_canonical_object() {
        let change = SuggestedChange {
            replacement: "safe_call()".into(),
        };
        assert_eq!(
            serde_json::to_value(change).unwrap(),
            serde_json::json!({"replacement":"safe_call()"})
        );
    }
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn llm_finding(path: &str, line: u32, title: &str, confidence: f32) -> ReviewFinding {
        ReviewFinding {
            severity: "high".into(),
            path: path.into(),
            line: Some(line),
            title: title.into(),
            message: "llm".into(),
            suggestion: String::new(),
            confidence,
            suggested_change: None,
            source: None,
            rule: None,
        }
    }

    #[test]
    fn merge_discards_findings_with_placeholder_severity() {
        let mut finding = llm_finding("src/lib.rs", 1, "placeholder", 0.99);
        finding.severity = "<LEVEL>".into();
        let merged = merge_results(
            "test-model".into(),
            1,
            vec![ChunkResult {
                review: ReviewResult {
                    model: "test-model".into(),
                    files_reviewed: 1,
                    summary: String::new(),
                    findings: vec![finding],
                },
                usage: None,
            }],
            &ReviewPolicy::default(),
            Vec::new(),
        );

        assert!(merged.findings.is_empty());
    }

    #[test]
    fn candidates_are_available_before_confidence_and_severity_policies() {
        let mut candidate = llm_finding("src/lib.rs", 1, "possible issue", 0.2);
        candidate.severity = "low".into();
        let candidates = collect_candidates(
            "test-model".into(),
            1,
            vec![ChunkResult {
                review: ReviewResult {
                    model: "test-model".into(),
                    files_reviewed: 1,
                    summary: String::new(),
                    findings: vec![candidate],
                },
                usage: None,
            }],
            &ReviewPolicy::default(),
            Vec::new(),
        );

        assert_eq!(candidates.findings.len(), 1);
        assert!(
            apply_policy(candidates, &ReviewPolicy::default())
                .findings
                .is_empty()
        );
    }

    #[tokio::test]
    async fn review_reports_provider_error_envelope_without_choices() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": {"message": "No endpoints found for this model"},
                "request_id": "req-123"
            })))
            .mount(&server)
            .await;

        let agent = OpenAiCompatibleAgent::new(
            OpenAiCompatibleSettings {
                base_url: server.uri(),
                api_key: "test-key".into(),
                model: "test-model".into(),
                temperature: 0.1,
                max_output_tokens: 100,
            },
            "Review prompt".into(),
        )
        .unwrap();
        let result = agent
            .review_chunk(&DiffChunk {
                index: 0,
                text: "diff --git a/a.rs b/a.rs".into(),
                files: vec!["a.rs".into()],
            })
            .await;

        let error = result.unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("No endpoints found for this model"));
        assert!(!message.contains("missing field `choices`"));
    }

    fn tool_finding(path: &str, line: u32, rule: &str, severity: &str) -> ReviewFinding {
        ReviewFinding {
            severity: severity.into(),
            path: path.into(),
            line: Some(line),
            title: format!("tool: {rule}"),
            message: "tool".into(),
            suggestion: String::new(),
            confidence: 1.0,
            suggested_change: None,
            source: Some("clippy".into()),
            rule: Some(rule.into()),
        }
    }

    #[test]
    fn tool_finding_wins_over_llm_on_same_line() {
        let findings = vec![
            llm_finding("src/a.rs", 5, "dangerous unwrap", 0.9),
            tool_finding("src/a.rs", 5, "unused_must_use", "medium"),
        ];
        let merged = deduplicate_review_findings(findings);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source.as_deref(), Some("clippy"));
        assert_eq!(merged[0].severity, "medium");
    }

    #[test]
    fn tool_finding_not_overridden_by_higher_confidence_llm() {
        let findings = vec![
            tool_finding("src/a.rs", 9, "needless_return", "low"),
            llm_finding("src/a.rs", 9, "needless_return", 0.99),
        ];
        let merged = deduplicate_review_findings(findings);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source.as_deref(), Some("clippy"));
    }

    #[test]
    fn different_rules_on_same_line_stay_separate() {
        let findings = vec![
            tool_finding("src/a.rs", 3, "rule_a", "high"),
            tool_finding("src/a.rs", 3, "rule_b", "high"),
        ];
        let merged = deduplicate_review_findings(findings);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn higher_confidence_llm_wins_over_lower_confidence_llm() {
        let findings = vec![
            llm_finding("src/a.rs", 7, "same issue", 0.7),
            llm_finding("src/a.rs", 7, "same issue", 0.95),
        ];
        let merged = deduplicate_review_findings(findings);
        assert_eq!(merged.len(), 1);
        assert!((merged[0].confidence - 0.95).abs() < f32::EPSILON);
    }
}
