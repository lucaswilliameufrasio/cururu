use crate::config::LlmConfig;
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
) -> ReviewResult {
    let mut findings: Vec<ReviewFinding> = results
        .into_iter()
        .flat_map(|r| r.review.findings)
        .collect();

    findings.retain(|f| f.confidence >= 0.65);
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    findings.truncate(30);

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

fn severity_rank(severity: &str) -> u8 {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}
