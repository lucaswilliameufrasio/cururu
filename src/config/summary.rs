#[derive(Debug, Clone, Default)]
pub struct SummaryConfig {
    pub show_cost: bool,
    pub show_usage: bool,
    pub logo_url: Option<String>,
}
