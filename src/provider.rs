use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct ProviderUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cached_tokens: u32,
    pub reasoning_tokens: u32,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub usage: Option<ProviderUsage>,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<UsageStats>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: AssistantMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AssistantMessage {
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsageStats {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionDetails>,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct PromptDetails {
    #[serde(default)]
    pub cached_tokens: u32,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct CompletionDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

impl ChatResponse {
    pub fn first_choice(&self, empty_message: &str) -> anyhow::Result<&Choice> {
        if let Some(choice) = self.choices.first() {
            return Ok(choice);
        }

        let provider_message = self.error.as_ref().and_then(|error| match error {
            serde_json::Value::String(message) => Some(message.as_str()),
            serde_json::Value::Object(fields) => fields
                .get("message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| fields.get("code").and_then(serde_json::Value::as_str)),
            _ => None,
        });
        if let Some(message) = provider_message {
            let message: String = message.chars().take(400).collect();
            anyhow::bail!("{empty_message}: provider returned an error without choices: {message}");
        }

        anyhow::bail!(
            "{empty_message}: response did not contain an OpenAI-compatible `choices` array"
        );
    }

    pub fn extract_metadata(&self) -> ProviderMetadata {
        let usage = self.usage.as_ref().map(|u| ProviderUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
            cached_tokens: u
                .prompt_tokens_details
                .as_ref()
                .map_or(0, |d| d.cached_tokens),
            reasoning_tokens: u
                .completion_tokens_details
                .as_ref()
                .map_or(0, |d| d.reasoning_tokens),
            cost: u.cost,
        });
        ProviderMetadata { usage }
    }
}

pub fn merge_usage(results: &[super::agent::ChunkResult]) -> Option<ProviderUsage> {
    let usages: Vec<&ProviderUsage> = results.iter().filter_map(|r| r.usage.as_ref()).collect();
    if usages.is_empty() {
        return None;
    }
    let mut total = usages[0].clone();
    for u in &usages[1..] {
        total.prompt_tokens += u.prompt_tokens;
        total.completion_tokens += u.completion_tokens;
        total.total_tokens += u.total_tokens;
        total.cached_tokens += u.cached_tokens;
        total.reasoning_tokens += u.reasoning_tokens;
        total.cost = match (total.cost, u.cost) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::{ChatResponse, ProviderUsage, merge_usage};
    use crate::agent::{ChunkResult, ReviewResult};

    fn chunk(cost: Option<f64>) -> ChunkResult {
        ChunkResult {
            review: ReviewResult {
                model: "model".into(),
                files_reviewed: 1,
                summary: String::new(),
                findings: vec![],
            },
            usage: Some(ProviderUsage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
                cached_tokens: 0,
                reasoning_tokens: 0,
                cost,
            }),
        }
    }

    #[test]
    fn merged_cost_is_unavailable_if_any_chunk_lacks_provider_cost() {
        let merged = merge_usage(&[chunk(Some(0.25)), chunk(None)]).unwrap();
        assert_eq!(merged.prompt_tokens, 2);
        assert_eq!(merged.cost, None);
    }

    #[test]
    fn merged_cost_sums_provider_costs_when_every_chunk_reports_them() {
        let merged = merge_usage(&[chunk(Some(0.25)), chunk(Some(0.5))]).unwrap();
        assert_eq!(merged.cost, Some(0.75));
    }

    #[test]
    fn reports_provider_error_when_response_has_no_choices() {
        let response: ChatResponse = serde_json::from_str(
            r#"{"error":{"message":"No endpoints found for this model"},"request_id":"req-123"}"#,
        )
        .unwrap();

        let error = response
            .first_choice("LLM returned no choices")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("No endpoints found for this model")
        );
    }

    #[test]
    fn explains_unexpected_response_shape_when_choices_are_missing() {
        let response: ChatResponse = serde_json::from_str(r#"{"status":"ok"}"#).unwrap();

        let error = response
            .first_choice("LLM returned no choices")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("OpenAI-compatible `choices` array")
        );
    }
}
