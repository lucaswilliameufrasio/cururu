use super::*;
use globset::GlobSetBuilder;
use std::sync::{LazyLock, Mutex};

use super::env::{build_globs, env_optional, env_parse, env_required};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn base_config() -> AppConfig {
    AppConfig {
        github: GitHubConfig {
            token: "test-token".into(),
            repository: "owner/repo".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            pr_number: 42,
            api_url: "https://api.github.com".into(),
            server_url: "https://github.com".into(),
        },
        llm: LlmConfig {
            provider: LlmProvider::OpenRouter,
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: "sk-test".into(),
            model: "openai/gpt-5.6-luna".into(),
            temperature: 0.1,
            max_output_tokens: 4000,
        },
        review: ReviewConfig {
            max_diff_bytes: 180_000,
            chunk_bytes: 45_000,
            ignore: GlobSetBuilder::new().build().unwrap(),
            language: "pt-BR".into(),
            comment_mode: CommentMode::Inline,
            policy: ReviewPolicy::default(),
        },
        context: ContextConfig::default(),
        summary: SummaryConfig::default(),
        analysis: AnalysisConfig::default(),
    }
}

#[test]
fn provider_from_name() {
    assert_eq!(LlmProvider::from_name("openai"), Some(LlmProvider::OpenAI));
    assert_eq!(LlmProvider::from_name("OpenAI"), Some(LlmProvider::OpenAI));
    assert_eq!(LlmProvider::from_name("OPENAI"), Some(LlmProvider::OpenAI));
    assert_eq!(
        LlmProvider::from_name("openrouter"),
        Some(LlmProvider::OpenRouter)
    );
    assert_eq!(
        LlmProvider::from_name("OpenRouter"),
        Some(LlmProvider::OpenRouter)
    );
    assert_eq!(LlmProvider::from_name("groq"), Some(LlmProvider::Groq));
    assert_eq!(LlmProvider::from_name("GROQ"), Some(LlmProvider::Groq));
    assert_eq!(LlmProvider::from_name("invalid"), None);
    assert_eq!(LlmProvider::from_name(""), None);
}

#[test]
fn provider_defaults() {
    assert_eq!(
        LlmProvider::OpenAI.default_base_url(),
        "https://api.openai.com/v1"
    );
    assert_eq!(LlmProvider::OpenAI.default_model(), "gpt-5.6-luna");
    assert_eq!(
        LlmProvider::OpenRouter.default_base_url(),
        "https://openrouter.ai/api/v1"
    );
    assert_eq!(
        LlmProvider::OpenRouter.default_model(),
        "openai/gpt-5.6-luna"
    );
    assert_eq!(
        LlmProvider::Groq.default_base_url(),
        "https://api.groq.com/openai/v1"
    );
    assert_eq!(LlmProvider::Groq.default_model(), "openai/gpt-oss-120b");
}

#[test]
fn rejects_unsupported_version() {
    let mut cfg = base_config();
    let err = cfg.merge_toml_str("version = 2\n").unwrap_err();
    assert!(
        err.to_string().contains("unsupported"),
        "expected unsupported version error"
    );
}

#[test]
fn accepts_minimal_toml() {
    let mut cfg = base_config();
    cfg.merge_toml_str("version = 1\n").unwrap();
    assert_eq!(cfg.llm.provider, LlmProvider::OpenRouter);
    assert!(!cfg.summary.show_cost);
}

#[test]
fn overrides_provider() {
    let _guard = ENV_LOCK.lock().unwrap();
    let mut cfg = base_config();
    cfg.merge_toml_str("version = 1\n[provider]\nname = \"groq\"\nmodel = \"mixtral-8x7b\"\n")
        .unwrap();
    assert_eq!(cfg.llm.provider, LlmProvider::Groq);
    assert_eq!(cfg.llm.model, "mixtral-8x7b");
    assert_eq!(cfg.llm.base_url, "https://api.groq.com/openai/v1");
}

#[test]
fn provider_change_updates_default_model() {
    let _guard = ENV_LOCK.lock().unwrap();
    let mut cfg = base_config();
    assert_eq!(cfg.llm.model, "openai/gpt-5.6-luna");
    cfg.merge_toml_str("version = 1\n[provider]\nname = \"groq\"\n")
        .unwrap();
    assert_eq!(cfg.llm.provider, LlmProvider::Groq);
    assert_eq!(cfg.llm.model, "openai/gpt-oss-120b");
    assert_eq!(cfg.llm.base_url, "https://api.groq.com/openai/v1");
}

#[test]
fn toml_overrides_generation_parameters() {
    let mut cfg = base_config();
    cfg.merge_toml_str("version = 1\n[provider]\ntemperature = 0.3\nmax_output_tokens = 8192\n")
        .unwrap();
    assert!((cfg.llm.temperature - 0.3).abs() < f32::EPSILON);
    assert_eq!(cfg.llm.max_output_tokens, 8192);
}

#[test]
fn policy_profile_and_overrides_are_merged() {
    let mut cfg = base_config();
    cfg.merge_toml_str(
        "version = 1\n[review]\nprofile = \"security\"\n[policy]\nmax_findings = 7\nsuggested_changes = true\n",
    )
    .unwrap();
    assert_eq!(cfg.review.policy.profile, "security");
    assert!((cfg.review.policy.minimum_confidence - 0.75).abs() < f32::EPSILON);
    assert_eq!(cfg.review.policy.max_findings, 7);
    assert_eq!(cfg.review.policy.fail_on, FailOn::High);
    assert!(cfg.review.policy.suggested_changes);
}

#[test]
fn invalid_policy_values_are_rejected() {
    let mut cfg = base_config();
    assert!(
        cfg.merge_toml_str("version = 1\n[policy]\nminimum_confidence = 2.0\n")
            .is_err()
    );
    assert!(
        cfg.merge_toml_str("version = 1\n[policy]\nfail_on = \"urgent\"\n")
            .is_err()
    );
}

#[test]
fn analysis_config_is_loaded() {
    let mut cfg = base_config();
    cfg.merge_toml_str(
        "version = 1\n[analysis]\nenabled = true\nsarif_paths = [\"reports/**/*.sarif\"]\nmax_findings = 12\n",
    )
    .unwrap();
    assert!(cfg.analysis.enabled);
    assert_eq!(cfg.analysis.sarif_paths, vec!["reports/**/*.sarif"]);
    assert_eq!(cfg.analysis.max_findings, 12);
}

#[test]
fn env_overrides_toml_generation_parameters() {
    let _guard = ENV_LOCK.lock().unwrap();
    temp_env::with_var("LLM_TEMPERATURE", Some("0.7"), || {
        temp_env::with_var("LLM_MAX_OUTPUT_TOKENS", Some("2048"), || {
            let mut cfg = base_config();
            cfg.llm.temperature = 0.7;
            cfg.llm.max_output_tokens = 2048;
            cfg.merge_toml_str(
                "version = 1\n[provider]\ntemperature = 0.3\nmax_output_tokens = 8192\n",
            )
            .unwrap();
            assert!((cfg.llm.temperature - 0.7).abs() < f32::EPSILON);
            assert_eq!(cfg.llm.max_output_tokens, 2048);
        });
    });
}

#[test]
fn env_var_overrides_toml_provider() {
    let _guard = ENV_LOCK.lock().unwrap();
    temp_env::with_var("CURURU_PROVIDER", Some("openrouter"), || {
        temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
            temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                temp_env::with_var("PR_NUMBER", Some("1"), || {
                    temp_env::with_var("LLM_API_KEY", Some("key"), || {
                        let mut cfg = AppConfig::from_env().unwrap();
                        cfg.merge_toml_str("version = 1\n[provider]\nname = \"groq\"\n")
                            .unwrap();
                        assert_eq!(cfg.llm.provider, LlmProvider::OpenRouter);
                    });
                });
            });
        });
    });
}

#[test]
fn env_var_overrides_toml_base_url() {
    let _guard = ENV_LOCK.lock().unwrap();
    temp_env::with_var("LLM_BASE_URL", Some("https://custom.example.com"), || {
        temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
            temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                temp_env::with_var("PR_NUMBER", Some("1"), || {
                    temp_env::with_var("LLM_API_KEY", Some("key"), || {
                        let mut cfg = AppConfig::from_env().unwrap();
                        cfg.merge_toml_str(
                            "version = 1\n[provider]\nbase_url = \"https://ignored.com\"\n",
                        )
                        .unwrap();
                        assert_eq!(cfg.llm.base_url, "https://custom.example.com");
                    });
                });
            });
        });
    });
}

#[test]
fn env_var_overrides_toml_model() {
    let _guard = ENV_LOCK.lock().unwrap();
    temp_env::with_var("LLM_MODEL", Some("custom-model"), || {
        temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
            temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                temp_env::with_var("PR_NUMBER", Some("1"), || {
                    temp_env::with_var("LLM_API_KEY", Some("key"), || {
                        let mut cfg = AppConfig::from_env().unwrap();
                        cfg.merge_toml_str("version = 1\n[provider]\nmodel = \"ignored\"\n")
                            .unwrap();
                        assert_eq!(cfg.llm.model, "custom-model");
                    });
                });
            });
        });
    });
}

#[test]
fn sets_review_config() {
    let mut cfg = base_config();
    cfg.merge_toml_str(
        "version = 1\n[review]\nmax_diff_bytes = 9999\nchunk_bytes = 1111\nignore = [\"*.lock\"]\n",
    )
    .unwrap();
    assert_eq!(cfg.review.max_diff_bytes, 9999);
    assert_eq!(cfg.review.chunk_bytes, 1111);
}

#[test]
fn comment_mode_defaults_to_inline() {
    let cfg = base_config();
    assert_eq!(cfg.review.comment_mode, CommentMode::Inline);
}

#[test]
fn toml_overrides_comment_mode() {
    let mut cfg = base_config();
    cfg.merge_toml_str("version = 1\n[review]\ncomment_mode = \"summary\"\n")
        .unwrap();
    assert_eq!(cfg.review.comment_mode, CommentMode::Summary);

    let mut cfg = base_config();
    cfg.merge_toml_str("version = 1\n[review]\ncomment_mode = \"inline\"\n")
        .unwrap();
    assert_eq!(cfg.review.comment_mode, CommentMode::Inline);
}

#[test]
fn comment_mode_from_name() {
    assert_eq!(CommentMode::from_name("inline"), Some(CommentMode::Inline));
    assert_eq!(
        CommentMode::from_name("SUMMARY"),
        Some(CommentMode::Summary)
    );
    assert_eq!(
        CommentMode::from_name("  summary  "),
        Some(CommentMode::Summary)
    );
    assert_eq!(CommentMode::from_name("nope"), None);
}

#[test]
fn sets_context_files() {
    let mut cfg = base_config();
    cfg.merge_toml_str(
        r#"
            version = 1
            [context]
            conventions = ["AGENTS.md"]
            specifications = ["docs/sdd/**/*.md"]
            skills = [".agents/skills/**/SKILL.md"]
            additional = ["docs/adr/**/*.md"]
            max_bytes = 50000
            "#,
    )
    .unwrap();
    assert_eq!(cfg.context.conventions, vec!["AGENTS.md"]);
    assert_eq!(cfg.context.max_bytes, 50000);
}

#[test]
fn sets_summary_flags() {
    let mut cfg = base_config();
    cfg.merge_toml_str("version = 1\n[summary]\nshow_cost = true\nshow_usage = true\n")
        .unwrap();
    assert!(cfg.summary.show_cost);
    assert!(cfg.summary.show_usage);
}

#[test]
fn partial_toml_does_not_reset_unset_fields() {
    let mut cfg = base_config();
    cfg.context.max_bytes = 777;
    cfg.merge_toml_str("version = 1\n[context]\nconventions = [\"CONVENTIONS.md\"]\n")
        .unwrap();
    assert_eq!(cfg.context.conventions, vec!["CONVENTIONS.md"]);
    assert_eq!(cfg.context.max_bytes, 777);
}

#[test]
fn language_default_is_pt_br() {
    let cfg = base_config();
    assert_eq!(cfg.review.language, "pt-BR");
}

#[test]
fn toml_overrides_language() {
    let mut cfg = base_config();
    cfg.merge_toml_str("version = 1\n[review]\nlanguage = \"en-US\"\n")
        .unwrap();
    assert_eq!(cfg.review.language, "en-US");
}

#[test]
fn build_globs_empty() {
    let set = build_globs("").unwrap();
    assert!(set.is_empty());
}

#[test]
fn build_globs_multiple() {
    let set = build_globs("*.rs,*.toml").unwrap();
    assert!(set.is_match("main.rs"));
    assert!(set.is_match("Cargo.toml"));
    assert!(!set.is_match("README.md"));
}

#[test]
fn env_required_ok() {
    temp_env::with_var("TEST_ENV_REQUIRED", Some("value"), || {
        assert_eq!(env_required("TEST_ENV_REQUIRED").unwrap(), "value");
    });
}

#[test]
fn env_required_missing() {
    temp_env::with_var("TEST_ENV_UNSET", None::<&str>, || {
        assert!(env_required("TEST_ENV_UNSET").is_err());
    });
}

#[test]
fn env_required_empty() {
    temp_env::with_var("TEST_ENV_EMPTY", Some(""), || {
        assert!(env_required("TEST_ENV_EMPTY").is_err());
    });
}

#[test]
fn env_optional_returns_none_for_empty() {
    temp_env::with_var("TEST_OPT_EMPTY", Some(""), || {
        assert_eq!(env_optional("TEST_OPT_EMPTY"), None);
    });
}

#[test]
fn env_optional_returns_value() {
    temp_env::with_var("TEST_OPT_VAL", Some("hello"), || {
        assert_eq!(env_optional("TEST_OPT_VAL"), Some("hello".into()));
    });
}

#[test]
fn env_parse_invalid_returns_error() {
    temp_env::with_var("TEST_PARSE", Some("not-a-number"), || {
        assert!(env_parse::<u32>("TEST_PARSE", 0).is_err());
    });
}

#[test]
fn env_parse_valid() {
    temp_env::with_var("TEST_PARSE_VALID", Some("42"), || {
        assert_eq!(env_parse::<u32>("TEST_PARSE_VALID", 0).unwrap(), 42);
    });
}

#[test]
fn env_parse_missing_uses_default() {
    temp_env::with_var("TEST_PARSE_MISSING", None::<&str>, || {
        assert_eq!(env_parse::<u32>("TEST_PARSE_MISSING", 99).unwrap(), 99);
    });
}

#[test]
fn provider_name_changes_default_url() {
    let _guard = ENV_LOCK.lock().unwrap();
    temp_env::with_var("CURURU_PROVIDER", Some("groq"), || {
        temp_env::with_var("LLM_BASE_URL", None::<&str>, || {
            temp_env::with_var("LLM_API_KEY", Some("key"), || {
                temp_env::with_var("GITHUB_TOKEN", Some("token"), || {
                    temp_env::with_var("GITHUB_REPOSITORY", Some("owner/repo"), || {
                        temp_env::with_var("PR_NUMBER", Some("1"), || {
                            let cfg = AppConfig::from_env().unwrap();
                            assert_eq!(cfg.llm.provider, LlmProvider::Groq);
                            assert_eq!(cfg.llm.base_url, "https://api.groq.com/openai/v1");
                            assert_eq!(cfg.llm.model, "openai/gpt-oss-120b");
                        });
                    });
                });
            });
        });
    });
}
