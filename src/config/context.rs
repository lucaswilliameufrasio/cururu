#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub conventions: Vec<String>,
    pub specifications: Vec<String>,
    pub skills: Vec<String>,
    pub additional: Vec<String>,
    pub max_bytes: usize,
    pub auto: AutoContextConfig,
}

pub use cururu_engine::auto_context::AutoContextConfig;

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            conventions: Vec::new(),
            specifications: Vec::new(),
            skills: Vec::new(),
            additional: Vec::new(),
            max_bytes: 100_000,
            auto: AutoContextConfig::default(),
        }
    }
}
