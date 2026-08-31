#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub conventions: Vec<String>,
    pub specifications: Vec<String>,
    pub skills: Vec<String>,
    pub additional: Vec<String>,
    pub max_bytes: usize,
    pub auto: AutoContextConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoContextConfig {
    pub enabled: bool,
    pub max_bytes: usize,
    pub max_files: usize,
    pub per_file_bytes: usize,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for AutoContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bytes: 50_000,
            max_files: 20,
            per_file_bytes: 12_000,
            include: vec!["src/**".into(), "tests/**".into()],
            exclude: vec!["**/generated/**".into(), "**/*.min.js".into()],
        }
    }
}

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
