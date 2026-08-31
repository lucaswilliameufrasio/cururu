#[derive(Debug, Clone)]
pub struct GitHubConfig {
    pub token: String,
    pub repository: String,
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub api_url: String,
    pub server_url: String,
}
