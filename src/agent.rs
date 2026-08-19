use crate::config::{LlmConfig, ReviewPolicy, Severity};
use crate::diff::DiffChunk;
use crate::provider::{ChatResponse, ProviderUsage};
use crate::retry::retry_with_backoff;
use anyhow::Context;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub model: String,
    pub files_reviewed: usize,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewFinding {
    pub severity: String,
    pub path: String,
    pub line: Option<u32>,
    pub title: String,
    pub message: String,
    pub suggestion: String,
    pub confidence: f32,
    #[serde(default)]
    pub suggested_change: Option<SuggestedChange>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub rule: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SuggestedChange {
    pub replacement: String,
}

#[derive(Debug, Clone)]
pub struct ChunkResult {
    pub review: ReviewResult,
    pub usage: Option<ProviderUsage>,
}

#[async_trait]
pub trait ReviewAgent: Send + Sync {
    async fn review_chunk(&self, chunk: &DiffChunk) -> anyhow::Result<ChunkResult>;
}

pub fn build_agent(config: &LlmConfig, prompt: String) -> anyhow::Result<Box<dyn ReviewAgent>> {
    Ok(Box::new(OpenAiCompatibleAgent::new(
        config.clone(),
        prompt,
    )?))
}

struct OpenAiCompatibleAgent {
    client: reqwest::Client,
    config: LlmConfig,
    system_prompt: String,
}

impl OpenAiCompatibleAgent {
    fn new(config: LlmConfig, system_prompt: String) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent("cururu/0.1")
                .build()?,
            config,
            system_prompt,
        })
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    max_tokens: u32,
    response_format: ResponseFormat,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[async_trait]
impl ReviewAgent for OpenAiCompatibleAgent {
    async fn review_chunk(&self, chunk: &DiffChunk) -> anyhow::Result<ChunkResult> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let user = format!(
            "Review this unified diff chunk. Return JSON only matching the schema.\n\nFiles: {:?}\n\n```diff\n{}\n```",
            chunk.files, chunk.text
        );

        let req = ChatRequest {
            model: &self.config.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: self.system_prompt.clone(),
                },
                ChatMessage {
                    role: "user",
                    content: user,
                },
            ],
            temperature: self.config.temperature,
            max_tokens: self.config.max_output_tokens,
            response_format: ResponseFormat {
                kind: "json_object",
            },
        };

        let response = retry_with_backoff(
            || async {
                self.client
                    .post(&url)
                    .timeout(Duration::from_mins(2))
                    .bearer_auth(&self.config.api_key)
                    .json(&req)
                    .send()
                    .await
                    .context("failed to send LLM request")?
                    .error_for_status()
                    .context("LLM API error")?
                    .json::<ChatResponse>()
                    .await
                    .context("failed to parse LLM response")
            },
            3,
        )
        .await?;

        let content = response
            .choices
            .first()
            .context("LLM returned no choices")?
            .message
            .content
            .trim()
            .to_string();

        let mut review: ReviewResult = serde_json::from_str(&content)
            .with_context(|| format!("invalid LLM JSON: {content}"))?;
        review.model.clone_from(&self.config.model);

        let meta = response.extract_metadata();

        Ok(ChunkResult {
            review,
            usage: meta.usage,
        })
    }
}

pub fn merge_results(
    model: String,
    files_reviewed: usize,
    results: Vec<ChunkResult>,
    policy: &ReviewPolicy,
    additional_findings: Vec<ReviewFinding>,
) -> ReviewResult {
    let mut findings: Vec<ReviewFinding> = results
        .into_iter()
        .flat_map(|r| r.review.findings)
        .collect();
    findings.extend(additional_findings);

    findings.retain(|f| {
        f.confidence.is_finite()
            && f.confidence >= policy.minimum_confidence
            && Severity::from_name(&f.severity)
                .is_some_and(|severity| policy.allowed_severities.contains(&severity))
    });
    if !policy.suggested_changes {
        for finding in &mut findings {
            finding.suggested_change = None;
        }
    }
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    if policy.synthesis {
        findings = deduplicate_findings(findings);
    }
    findings.truncate(policy.max_findings);

    ReviewResult {
        model,
        files_reviewed,
        summary: format!(
            "Found {} high-confidence review finding(s).",
            findings.len()
        ),
        findings,
    }
}

fn deduplicate_findings(findings: Vec<ReviewFinding>) -> Vec<ReviewFinding> {
    let mut unique: Vec<ReviewFinding> = Vec::with_capacity(findings.len());
    for finding in findings {
        let matches = unique.iter().position(|existing| {
            existing.path == finding.path
                && existing.line == finding.line
                && same_rule(existing, &finding)
                && titles_overlap(existing, &finding)
        });
        match matches {
            Some(index) => {
                let existing = &mut unique[index];
                if merge_prefers(&finding, existing) {
                    *existing = finding;
                }
            }
            None => unique.push(finding),
        }
    }
    unique
}

fn same_rule(a: &ReviewFinding, b: &ReviewFinding) -> bool {
    match (&a.rule, &b.rule) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

fn titles_overlap(a: &ReviewFinding, b: &ReviewFinding) -> bool {
    match (a.rule.as_deref(), b.rule.as_deref()) {
        (Some(x), Some(y)) => x == y,
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => a.title.eq_ignore_ascii_case(&b.title),
    }
}

fn merge_prefers(candidate: &ReviewFinding, existing: &ReviewFinding) -> bool {
    let candidate_is_tool = candidate.source.is_some();
    let existing_is_tool = existing.source.is_some();
    match (candidate_is_tool, existing_is_tool) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate.confidence > existing.confidence,
    }
}

fn severity_rank(severity: &str) -> u8 {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let merged = deduplicate_findings(findings);
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
        let merged = deduplicate_findings(findings);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source.as_deref(), Some("clippy"));
    }

    #[test]
    fn different_rules_on_same_line_stay_separate() {
        let findings = vec![
            tool_finding("src/a.rs", 3, "rule_a", "high"),
            tool_finding("src/a.rs", 3, "rule_b", "high"),
        ];
        let merged = deduplicate_findings(findings);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn higher_confidence_llm_wins_over_lower_confidence_llm() {
        let findings = vec![
            llm_finding("src/a.rs", 7, "same issue", 0.7),
            llm_finding("src/a.rs", 7, "same issue", 0.95),
        ];
        let merged = deduplicate_findings(findings);
        assert_eq!(merged.len(), 1);
        assert!((merged[0].confidence - 0.95).abs() < f32::EPSILON);
    }
}
