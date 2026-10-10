use serde::Deserialize;

use crate::ChunkResult;

#[derive(Debug, Clone)]
pub struct ReviewUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cached_tokens: u32,
    pub reasoning_tokens: u32,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub usage: Option<ReviewUsage>,
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

    #[must_use]
    pub fn extract_metadata(&self) -> ProviderMetadata {
        let usage = self.usage.as_ref().map(|usage| ReviewUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens: usage
                .prompt_tokens_details
                .as_ref()
                .map_or(0, |details| details.cached_tokens),
            reasoning_tokens: usage
                .completion_tokens_details
                .as_ref()
                .map_or(0, |details| details.reasoning_tokens),
            cost: usage.cost,
        });
        ProviderMetadata { usage }
    }
}

#[must_use]
pub fn merge_usage(results: &[ChunkResult]) -> Option<ReviewUsage> {
    let mut usages = results.iter().filter_map(|result| result.usage.as_ref());
    let mut total = usages.next()?.clone();
    for usage in usages {
        total.prompt_tokens += usage.prompt_tokens;
        total.completion_tokens += usage.completion_tokens;
        total.total_tokens += usage.total_tokens;
        total.cached_tokens += usage.cached_tokens;
        total.reasoning_tokens += usage.reasoning_tokens;
        total.cost = match (total.cost, usage.cost) {
            (Some(previous), Some(next)) => Some(previous + next),
            _ => None,
        };
    }
    Some(total)
}
