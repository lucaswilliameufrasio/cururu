//! Pure candidate rules; hosts own revision selection, reads and consent.

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

/// A compiled pure predicate, not a capability to read or transmit candidates.
pub struct CandidateSelector {
    include: globset::GlobSet,
    exclude: globset::GlobSet,
    max_files: usize,
    max_bytes: usize,
}

impl CandidateSelector {
    pub fn new(config: &AutoContextConfig) -> anyhow::Result<Self> {
        Ok(Self {
            include: compile_globs(&config.include)?,
            exclude: compile_globs(&config.exclude)?,
            max_files: config.max_files,
            max_bytes: config.max_bytes,
        })
    }

    /// Call only in a host-authorized workflow. `enabled` is deliberately not
    /// interpreted here: a declaration cannot authorize reads or transmission.
    /// Count and bytes describe the entire existing context, not just automatic
    /// candidates. Re-evaluate after each successful host read.
    pub fn accepts<'a>(
        &self,
        path: &str,
        occupied_paths: impl IntoIterator<Item = &'a str>,
        file_count: usize,
        total_bytes: usize,
    ) -> bool {
        !occupied_paths.into_iter().any(|occupied| occupied == path)
            && self.include.is_match(path)
            && !self.exclude.is_match(path)
            && file_count < self.max_files
            && total_bytes < self.max_bytes
    }
}

fn compile_globs(patterns: &[String]) -> anyhow::Result<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_preserve_cli_contract_and_do_not_enable_automatic_io() {
        let config = AutoContextConfig::default();
        assert!(!config.enabled);
        assert_eq!(
            (config.max_bytes, config.max_files, config.per_file_bytes),
            (50_000, 20, 12_000)
        );
        assert_eq!(config.include, ["src/**", "tests/**"]);
        assert_eq!(config.exclude, ["**/generated/**", "**/*.min.js"]);
        let selector = CandidateSelector::new(&config).unwrap();
        for path in ["src/lib.rs", "tests/check.rs"] {
            assert!(selector.accepts(path, [], 0, 0));
        }
        for path in ["README.md", "src/generated/schema.rs", "src/vendor.min.js"] {
            assert!(!selector.accepts(path, [], 0, 0));
        }
    }

    #[test]
    fn capacity_and_duplicate_checks_follow_current_host_state() {
        let config = AutoContextConfig {
            max_files: 2,
            max_bytes: 10,
            ..AutoContextConfig::default()
        };
        let selector = CandidateSelector::new(&config).unwrap();
        assert!(selector.accepts("src/new.rs", ["docs/context.md"], 1, 9));
        assert!(!selector.accepts("src/new.rs", ["src/new.rs"], 1, 9));
        assert!(!selector.accepts("src/new.rs", [], 2, 0));
        assert!(!selector.accepts("src/new.rs", [], 0, 10));
        // A failed read doesn't mutate this selector or pre-consume a slot.
        assert!(selector.accepts("src/retry.rs", [], 1, 9));
        assert!(selector.accepts("src/retry.rs", [], 1, 9));
        assert!(!selector.accepts("src/retry.rs", ["src/retry.rs"], 2, 10));
    }

    #[test]
    fn malformed_patterns_fail_and_empty_include_never_matches() {
        for exclude in [false, true] {
            let mut config = AutoContextConfig::default();
            if exclude {
                config.exclude = vec!["[".to_string()];
            } else {
                config.include = vec!["[".to_string()];
            }
            assert!(CandidateSelector::new(&config).is_err());
        }
        let config = AutoContextConfig {
            include: Vec::new(),
            ..AutoContextConfig::default()
        };
        assert!(
            !CandidateSelector::new(&config)
                .unwrap()
                .accepts("src/lib.rs", [], 0, 0)
        );
    }
}
