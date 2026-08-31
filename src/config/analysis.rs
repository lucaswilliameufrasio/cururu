#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisConfig {
    pub enabled: bool,
    pub manifest: Option<String>,
    pub sarif_paths: Vec<String>,
    pub max_findings: usize,
    pub require_current_head: bool,
    pub check_runs: bool,
    pub check_run_names: Vec<String>,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            manifest: None,
            sarif_paths: Vec::new(),
            max_findings: 100,
            require_current_head: true,
            check_runs: false,
            check_run_names: Vec::new(),
        }
    }
}
