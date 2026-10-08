/// Normalize a source path from an analysis or SCM diagnostic for matching.
#[must_use]
pub fn normalize_analysis_path(path: &str) -> String {
    path.strip_prefix("file://")
        .unwrap_or(path)
        .trim_start_matches("./")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::normalize_analysis_path;

    #[test]
    fn normalizes_file_uri_and_repeated_current_directory_prefixes() {
        assert_eq!(normalize_analysis_path("file://./src/lib.rs"), "src/lib.rs");
        assert_eq!(normalize_analysis_path("././src/lib.rs"), "src/lib.rs");
        assert_eq!(normalize_analysis_path("src/lib.rs"), "src/lib.rs");
    }
}
