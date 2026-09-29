#[derive(Debug, Clone)]
pub struct ScmConfig {
    /// Logical adapter name, selected independently of repository identity.
    pub provider: String,
    pub token: String,
    /// Canonical namespace/repository path, not an API-specific URL.
    pub repository: String,
    pub change_request_number: u64,
    pub api_url: String,
    pub web_url: String,
}
