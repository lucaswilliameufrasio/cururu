#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    OpenAI,
    OpenRouter,
    Groq,
}

impl LlmProvider {
    pub const fn default_base_url(&self) -> &str {
        match self {
            Self::OpenAI => "https://api.openai.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Groq => "https://api.groq.com/openai/v1",
        }
    }

    pub const fn default_model(&self) -> &str {
        match self {
            Self::OpenAI => "gpt-5.6-luna",
            Self::OpenRouter => "openai/gpt-5.6-luna",
            Self::Groq => "openai/gpt-oss-120b",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAI),
            "openrouter" => Some(Self::OpenRouter),
            "groq" => Some(Self::Groq),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_output_tokens: u32,
}
