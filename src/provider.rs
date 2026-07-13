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
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<UsageStats>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: AssistantMessage,
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
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }
    Some(total)
}
