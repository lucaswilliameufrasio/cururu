use std::process::{Command, Stdio};

use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[test]
fn signed_webhook_observe_completes_and_rejects_changed_config() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/webhook_review_smoke.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cururu"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn signed_webhook_filter_stops_before_external_side_effects() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/webhook_filter_smoke.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cururu"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn api_review_pr_confirms_and_revalidates_without_publication() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/api_pr_smoke.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cururu"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn hosted_review_does_not_publish_when_feedback_or_evidence_changes() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/hosted_review_smoke.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cururu"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn api_review_anthropic_uses_native_messages_only_after_consent() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .current_dir(dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q"]);
    std::fs::write(dir.path().join("code.txt"), "before\n").unwrap();
    git(&["add", "code.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    std::fs::write(dir.path().join("code.txt"), "after\n").unwrap();
    std::fs::write(
        dir.path().join(".cururu.toml"),
        "version = 1\n[policy]\nfocus = ['security']\n",
    )
    .unwrap();
    let server = MockServer::start().await;
    let result =
        json!({"model":"fixture", "files_reviewed":1, "summary":"ok", "findings":[]}).to_string();
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"stop_reason":"end_turn","content":[{"type":"text","text":result}], "usage":{"input_tokens":12,"output_tokens":4,"cache_creation_input_tokens":3,"cache_read_input_tokens":6}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let run = |consent: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cururu"));
        cmd.current_dir(dir.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("CURURU_PROVIDER", "anthropic")
            .env("LLM_BASE_URL", server.uri())
            .env("LLM_API_KEY", "fixture-key")
            .arg("api-review");
        if consent {
            cmd.arg("--confirm-send");
        }
        cmd.output().unwrap()
    };
    assert!(!run(false).status.success());
    let output = run(true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["review"]["files_reviewed"], 1);
    assert_eq!(receipt["usage"]["prompt_tokens"], 21);
    assert_eq!(receipt["usage"]["total_tokens"], 25);
    assert!(receipt["usage"].get("cost").is_none());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "claude-sonnet-4-6");
    assert!(body["messages"].to_string().contains("+after"));
    assert!(body["messages"].to_string().contains("security"));
    assert!(body.get("tools").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn api_review_requires_consent_and_uses_configured_api_for_local_diff() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .current_dir(dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q"]);
    std::fs::write(dir.path().join("code.txt"), "before\n").unwrap();
    git(&["add", "code.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    std::fs::write(dir.path().join("code.txt"), "after\n").unwrap();
    let server = MockServer::start().await;
    let result =
        json!({"model":"fixture", "files_reviewed":1, "summary":"ok", "findings":[]}).to_string();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"choices":[{"message":{"content":result},"finish_reason":"stop"}]}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cururu"))
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("CURURU_PROVIDER", "deepseek")
            .env("LLM_BASE_URL", server.uri())
            .env("LLM_API_KEY", "fixture-key")
            .args(["api-review"])
            .args(extra)
            .output()
            .unwrap()
    };
    assert!(!run(&[]).status.success());
    assert!(
        !run(&["--confirm-send", "--expected-snapshot", "stale"])
            .status
            .success()
    );
    let output = run(&["--confirm-send"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["source"], "local");
    assert_eq!(receipt["review"]["files_reviewed"], 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "deepseek-flash");
    assert!(body.to_string().contains("+after"));
}

#[cfg(unix)]
#[test]
fn jsonc_update_requires_interactive_approval_and_makes_private_backup() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/jsonc_update_pty.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cururu"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn jsonc_update_refuses_noninteractive_confirmation() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("mcp.json");
    let backup = dir.path().join("backup.json");
    std::fs::write(&config, "{\"mcpServers\":{}}\n").unwrap();
    let mut process = Command::new(env!("CARGO_BIN_EXE_cururu"))
        .current_dir(dir.path())
        .env_clear()
        .args([
            "update-jsonc-mcp",
            "cursor",
            "mcp.json",
            "--backup",
            "backup.json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .take()
        .unwrap()
        .write_all(b"UPDATE\n")
        .unwrap();
    let output = process.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert_eq!(
        std::fs::read_to_string(config).unwrap(),
        "{\"mcpServers\":{}}\n"
    );
    assert!(!backup.exists());
}

#[cfg(unix)]
#[test]
fn publication_with_sarif_requires_interactive_confirmation() {
    let status = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/publication_pty.py"
        ))
        .arg(env!("CARGO_BIN_EXE_cururu"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn publish_with_local_evidence_refuses_piped_confirmation() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("receipt.json"), "{}").unwrap();
    std::fs::write(dir.path().join("analysis.sarif"), r#"{"runs":[]}"#).unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        r#"{"schema_version":1,"commit_sha":"fixture","tools":[]}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cururu"))
        .current_dir(dir.path())
        .env_clear()
        .args([
            "publish-agent-review",
            "receipt.json",
            "--sarif",
            "analysis.sarif",
            "--analysis-manifest",
            "manifest.json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"PUBLISH\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("publication requires an interactive terminal")
    );
    assert_eq!(output.stdout, [] as [u8; 0]);
}

#[test]
fn client_mcp_install_and_check_cli_are_read_only_and_fail_on_mismatch() {
    for (client, config) in [
        ("codex", ".codex/config.toml"),
        ("cursor", ".cursor/mcp.json"),
        ("claude", ".mcp.json"),
        ("opencode", "opencode.jsonc"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_cururu"))
                .current_dir(dir.path())
                .env_clear()
                .args(args)
                .output()
                .unwrap()
        };
        let installed = run(&["install-mcp", client, "--executable", "/fixture/cururu"]);
        assert!(installed.status.success());
        let original = std::fs::read(dir.path().join(config)).unwrap();
        let checked = run(&[
            "check-mcp",
            client,
            config,
            "--executable",
            "/fixture/cururu",
        ]);
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        assert!(
            !run(&["check-mcp", client, config, "--executable", "/wrong"])
                .status
                .success()
        );
        assert!(
            !run(&[
                "check-mcp",
                client,
                "missing",
                "--executable",
                "/fixture/cururu"
            ])
            .status
            .success()
        );
        assert_eq!(std::fs::read(dir.path().join(config)).unwrap(), original);
    }
}

#[test]
fn prepare_review_previews_explicit_sarif_without_invoking_an_agent() {
    let dir = tempfile::tempdir().unwrap();
    let run_git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(dir.path())
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        output.stdout
    };
    run_git(&["init", "-q"]);
    std::fs::write(dir.path().join("code.txt"), "before\n").unwrap();
    run_git(&["add", "code.txt"]);
    run_git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    let head = String::from_utf8(run_git(&["rev-parse", "HEAD"])).unwrap();
    std::fs::write(dir.path().join("code.txt"), "after\n").unwrap();
    std::fs::write(dir.path().join("analysis.sarif"), r#"{"runs":[{"tool":{"driver":{"name":"fixture"}},"results":[{"ruleId":"X","level":"warning","message":{"text":"fixture finding"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"code.txt"},"region":{"startLine":1}}}]}]}]}"#).unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        json!({"schema_version":1,"commit_sha":head.trim(),"tools":[]}).to_string(),
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_cururu"))
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .args([
                "prepare-review",
                "--local",
                "--sarif",
                "analysis.sarif",
                "--analysis-manifest",
                "manifest.json",
            ])
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prepared: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        prepared["analyzer_evidence"][0]["message"],
        "fixture finding"
    );
    std::fs::write(
        dir.path().join("manifest.json"),
        r#"{"schema_version":1,"commit_sha":"old","tools":[]}"#,
    )
    .unwrap();
    assert!(!run().status.success());
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn api_review_sarif_requires_consent_and_revalidates_both_files() {
    for changed_file in [None, Some("analysis.sarif"), Some("manifest.json")] {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            output.stdout
        };
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("code.txt"), "before\n").unwrap();
        git(&["add", "code.txt"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ]);
        let head = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        std::fs::write(dir.path().join("code.txt"), "after\n").unwrap();
        std::fs::write(dir.path().join("analysis.sarif"), r#"{"runs":[{"tool":{"driver":{"name":"fixture"}},"results":[{"ruleId":"X","level":"warning","message":{"text":"fixture finding"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"code.txt"},"region":{"startLine":1}}}]}]}]}"#).unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            json!({"schema_version":1,"commit_sha":head.trim(),"tools":[]}).to_string(),
        )
        .unwrap();
        let server = MockServer::start().await;
        let changed_path = changed_file.map(|name| dir.path().join(name));
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let messages = body["messages"].as_array().unwrap();
                assert!(messages.iter().any(|message| {
                    message["role"] == "user"
                        && message["content"].as_str().is_some_and(|text| {
                            text.contains("fixture finding") && text.contains("untrusted")
                        })
                }));
                assert!(
                    messages
                        .iter()
                        .filter(|message| message["role"] == "system")
                        .all(|message| {
                            !message["content"]
                                .as_str()
                                .unwrap()
                                .contains("fixture finding")
                        })
                );
                if let Some(path) = &changed_path {
                    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
                    std::io::Write::write_all(&mut file, b" ").unwrap();
                }
                let review =
                    json!({"model":"fixture", "files_reviewed":1,"summary":"ok","findings":[]})
                        .to_string();
                ResponseTemplate::new(200).set_body_json(
                    json!({"choices":[{"message":{"content":review},"finish_reason":"stop"}]}),
                )
            })
            .expect(1)
            .mount(&server)
            .await;
        let run = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_cururu"))
                .current_dir(dir.path())
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("CURURU_PROVIDER", "openai")
                .env("LLM_BASE_URL", server.uri())
                .env("LLM_API_KEY", "fixture-key")
                .args(args)
                .output()
                .unwrap()
        };
        let options = [
            "--sarif",
            "analysis.sarif",
            "--analysis-manifest",
            "manifest.json",
        ];
        let preview = run(&[
            "prepare-review",
            "--local",
            options[0],
            options[1],
            options[2],
            options[3],
        ]);
        assert!(
            preview.status.success(),
            "{}",
            String::from_utf8_lossy(&preview.stderr)
        );
        let prepared: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
        let hash = prepared["snapshot_sha256"].as_str().unwrap();
        assert!(
            !run(&["api-review", options[0], options[1], options[2], options[3]])
                .status
                .success()
        );
        assert!(
            !run(&["api-review", "--confirm-send", "--sarif", "analysis.sarif"])
                .status
                .success()
        );
        assert!(
            !run(&[
                "api-review",
                "--confirm-send",
                "--expected-snapshot",
                "stale",
                options[0],
                options[1],
                options[2],
                options[3]
            ])
            .status
            .success()
        );
        let output = run(&[
            "api-review",
            "--confirm-send",
            "--expected-snapshot",
            hash,
            options[0],
            options[1],
            options[2],
            options[3],
        ]);
        if changed_file.is_some() {
            assert!(
                !output.status.success(),
                "changed evidence must reject the review"
            );
            assert_eq!(output.stdout, [] as [u8; 0]);
        } else {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(receipt["snapshot_sha256"], hash);
            assert_eq!(receipt["review"]["findings"][0]["source"], "fixture");
            assert_eq!(
                receipt["review"]["findings"][0]["message"],
                "fixture finding"
            );
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn api_review_multiple_chunks_fails_closed_for_late_failure_and_evidence_change() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for mode in ["success", "late_failure", "evidence_change"] {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            output.stdout
        };
        git(&["init", "-q"]);
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.path().join(name), format!("{}\n", "old".repeat(20))).unwrap();
        }
        git(&["add", "a.txt", "b.txt"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ]);
        let head = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.path().join(name), format!("{}\n", "new".repeat(20))).unwrap();
        }
        let sarif = dir.path().join("analysis.sarif");
        std::fs::write(&sarif, r#"{"runs":[]}"#).unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            json!({"schema_version":1,"commit_sha":head.trim(),"tools":[]}).to_string(),
        )
        .unwrap();
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        Mock::given(method("POST")).and(path("/messages")).respond_with(move |_: &wiremock::Request| {
            let number = counter.fetch_add(1, Ordering::SeqCst) + 1;
            if mode == "evidence_change" && number == 2 {
                std::fs::write(&sarif, "{\"runs\":[]} ").unwrap();
            }
            let review = json!({"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]}).to_string();
            ResponseTemplate::new(200).set_body_json(json!({"stop_reason":if mode == "late_failure" && number == 2 { "max_tokens" } else { "end_turn" },"content":[{"type":"text","text":review}]}))
        }).expect(2).mount(&server).await;
        let run = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_cururu"))
                .current_dir(dir.path())
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("CURURU_PROVIDER", "anthropic")
                .env("LLM_BASE_URL", server.uri())
                .env("LLM_API_KEY", "fixture")
                .env("CURURU_CHUNK_BYTES", "250")
                .args(args)
                .output()
                .unwrap()
        };
        let options = [
            "--sarif",
            "analysis.sarif",
            "--analysis-manifest",
            "manifest.json",
        ];
        let preview = run(&[
            "prepare-review",
            "--local",
            options[0],
            options[1],
            options[2],
            options[3],
        ]);
        assert!(
            preview.status.success(),
            "{}",
            String::from_utf8_lossy(&preview.stderr)
        );
        let prepared: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
        assert_eq!(prepared["chunks"].as_array().unwrap().len(), 2);
        let hash = prepared["snapshot_sha256"].as_str().unwrap();
        let output = run(&[
            "api-review",
            "--confirm-send",
            "--expected-snapshot",
            hash,
            options[0],
            options[1],
            options[2],
            options[3],
        ]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        if mode == "success" {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(receipt["review"]["files_reviewed"], 2);
        } else {
            assert!(!output.status.success());
            assert_eq!(output.stdout, [] as [u8; 0]);
        }
    }
}

#[test]
fn codex_mcp_update_cli_preserves_existing_configuration_and_refuses_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# keep comment\n[mcp_servers.other]\ncommand = 'other'\n";
    std::fs::write(&path, original).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_cururu"))
            .current_dir(dir.path())
            .env_clear()
            .args([
                "update-codex-mcp",
                "config.toml",
                "--executable",
                "/usr/local/bin/cururu",
            ])
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.starts_with(original));
    let config: toml::Value = toml::from_str(&updated).unwrap();
    assert_eq!(
        config["mcp_servers"]["other"]["command"].as_str(),
        Some("other")
    );
    assert_eq!(
        config["mcp_servers"]["cururu"]["command"].as_str(),
        Some("/usr/local/bin/cururu")
    );
    assert!(!run().status.success());
    assert_eq!(std::fs::read_to_string(path).unwrap(), updated);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn retired_opencode_review_never_contacts_server() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q"]);
    std::fs::write(dir.path().join("code.txt"), "before\n").unwrap();
    git(&["add", "code.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    std::fs::write(dir.path().join("code.txt"), "after\n").unwrap();
    let server = MockServer::start().await;
    let review =
        json!({"model":"synthetic", "files_reviewed":1, "summary":"ok", "findings":[]}).to_string();
    Mock::given(method("POST"))
        .and(path("/api/experimental/generate/text"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"text":review}})))
        .expect(0)
        .mount(&server)
        .await;
    let run = |consent: bool, expected: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cururu"));
        command
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .args([
                "agent-review",
                "--local",
                "--opencode-server",
                &server.uri(),
            ]);
        if consent {
            command.arg("--confirm-send");
        }
        if let Some(hash) = expected {
            command.args(["--expected-snapshot", hash]);
        }
        command.output().unwrap()
    };
    assert!(!run(false, None).status.success());
    let no_terminal = Command::new(env!("CARGO_BIN_EXE_cururu"))
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .args(["publish-agent-review", "missing.json"])
        .output()
        .unwrap();
    assert!(!no_terminal.status.success());
    assert!(
        String::from_utf8_lossy(&no_terminal.stderr).contains("requires an interactive terminal")
    );
    let output = run(true, Some("any-hash"));
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("agent-review is retired"));
    assert!(error.contains("api-review") && error.contains("MCP"));
    let requests = server.received_requests().await.unwrap();
    assert!(requests.is_empty());
}
