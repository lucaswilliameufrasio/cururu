use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
#[allow(clippy::too_many_lines)]
fn stdio_multichunk_requires_one_result_per_index() {
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
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.path().join(name), format!("{}\n", "new".repeat(20))).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_cururu"))
        .arg("mcp")
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("CURURU_CHUNK_BYTES", "250")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut call = |id: u32, name: &str, arguments: serde_json::Value| {
        writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})).unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], id);
        response["result"].clone()
    };
    let prepared = call(1, "prepare_review", serde_json::json!({"source":"local"}));
    assert_eq!(prepared["isError"], false);
    let input: serde_json::Value =
        serde_json::from_str(prepared["content"][0]["text"].as_str().unwrap()).unwrap();
    let chunks = input["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 2);
    let reviews: Vec<_> = chunks.iter().map(|chunk| serde_json::json!({"chunk_index":chunk["index"],"review":{"model":"fixture","files_reviewed":chunk["files"].as_array().unwrap().len(),"summary":"ok","findings":[]}})).collect();
    let arguments = |items: serde_json::Value| serde_json::json!({"source":"local","snapshot_sha256":input["snapshot_sha256"],"reviews":items});
    // Every refusal names what failed, so the host can fix that part.
    let refusal = |result: serde_json::Value| {
        assert_eq!(result["isError"], true);
        result["content"][0]["text"].as_str().unwrap().to_owned()
    };
    let missing = refusal(call(
        2,
        "assemble_review",
        arguments(serde_json::json!([reviews[0]])),
    ));
    assert!(missing.contains("missing chunk_index [1]"), "{missing}");
    let duplicated = refusal(call(
        3,
        "assemble_review",
        arguments(serde_json::json!([reviews[0], reviews[0]])),
    ));
    assert!(
        duplicated.contains("duplicated chunk_index [0]"),
        "{duplicated}"
    );
    let mut invalid = reviews.clone();
    invalid[1]["review"]["findings"] = serde_json::json!([{"severity":"urgent","path":chunks[1]["files"][0],"line":1,"title":"t","message":"m","suggestion":"s","confidence":0.5}]);
    let field = refusal(call(
        5,
        "assemble_review",
        arguments(serde_json::json!(invalid)),
    ));
    assert!(
        field.contains("chunk_index 1")
            && field.contains("findings[0]")
            && field.contains("severity"),
        "{field}"
    );
    let mut wrong_count = reviews.clone();
    wrong_count[0]["review"]["files_reviewed"] = serde_json::json!(3);
    let count = refusal(call(
        6,
        "assemble_review",
        arguments(serde_json::json!(wrong_count)),
    ));
    assert!(
        count.contains("chunk_index 0") && count.contains("files_reviewed is 3"),
        "{count}"
    );
    let mut malformed = reviews.clone();
    malformed[1]["review"]
        .as_object_mut()
        .unwrap()
        .remove("summary");
    let schema = refusal(call(
        7,
        "assemble_review",
        arguments(serde_json::json!(malformed)),
    ));
    assert!(
        schema.contains("chunk_index 1") && schema.contains("summary"),
        "{schema}"
    );
    let stale = refusal(call(
        8,
        "assemble_review",
        serde_json::json!({"source":"local","snapshot_sha256":"0".repeat(64),"reviews":reviews}),
    ));
    assert!(stale.contains("snapshot_sha256 does not match"), "{stale}");
    let complete = call(
        4,
        "assemble_review",
        arguments(serde_json::json!([reviews[1], reviews[0]])),
    );
    assert_eq!(complete["isError"], false);
    let result: serde_json::Value =
        serde_json::from_str(complete["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["files_reviewed"], 2);
    drop(stdin);
    assert!(child.wait_with_output().unwrap().status.success());
}

#[test]
#[allow(clippy::too_many_lines)]
fn stdio_server_prepares_local_fixture_without_non_protocol_stdout() {
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
    std::fs::write(
        dir.path().join(".cururu.toml"),
        "version = 1\n[policy]\nsuggested_changes = true\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cururu"))
        .arg("mcp")
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})).unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"prepare_review","arguments":{"source":"local"}}})).unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let initialized: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(initialized["result"]["serverInfo"]["name"], "cururu");
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let prepared: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(prepared["result"]["isError"], false);
    let input: serde_json::Value =
        serde_json::from_str(prepared["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(input["files"][0]["path"], "code.txt");
    assert!(
        input["files"][0]["patch"]
            .as_str()
            .unwrap()
            .contains("+after")
    );
    let review = serde_json::json!({"model":"fixture", "files_reviewed":1, "summary":"ok", "findings":[{
        "severity":"high", "path":"code.txt", "line":1, "title":"fixture", "message":"issue", "suggestion":"guidance", "confidence":0.99,
        "source":"forged-tool", "rule":"forged-rule", "suggested_change":{"replacement":"fixture()"}
    }]});
    let incomplete =
        serde_json::json!({"model":"fixture", "files_reviewed":0, "summary":"ok", "findings":[]});
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"assemble_review","arguments":{"source":"local","snapshot_sha256":input["snapshot_sha256"],"reviews":[{"chunk_index":0,"review":incomplete}]}}})).unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let incomplete_response: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(incomplete_response["result"]["isError"], true);
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"assemble_review","arguments":{"source":"local","snapshot_sha256":input["snapshot_sha256"],"reviews":[{"chunk_index":0,"review":review}]}}})).unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let assembled: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(assembled["result"]["isError"], false);
    let result: serde_json::Value =
        serde_json::from_str(assembled["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let finding = &result["findings"][0];
    assert!(finding["source"].is_null());
    assert!(finding["rule"].is_null());
    assert_eq!(finding["suggested_change"]["replacement"], "fixture()");
    assert_eq!(finding["suggestion"], "guidance");
    std::fs::write(dir.path().join("code.txt"), "different change\n").unwrap();
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"assemble_review","arguments":{"source":"local","snapshot_sha256":input["snapshot_sha256"],"reviews":[{"chunk_index":0,"review":{"model":"fixture","files_reviewed":1,"summary":"ok","findings":[]}}]}}})).unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let changed: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(changed["result"]["isError"], true);
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"assemble_review","arguments":{"source":"local","snapshot_sha256":"stale","reviews":[]}}})).unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let rejected: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(rejected["result"]["isError"], true);
    drop(stdin);
    line.clear();
    assert_eq!(stdout.read_line(&mut line).unwrap(), 0);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "server exited unsuccessfully: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// MCP can prepare a codebase review of committed files, optionally limited
/// by `paths`, and validate a review against it; `paths` is refused for other
/// sources.
#[test]
fn stdio_server_prepares_and_validates_a_codebase_review() {
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
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/old.py"), "x = 1\ny = 2\n").unwrap();
    std::fs::write(dir.path().join("README.md"), "# Old project\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cururu"))
        .arg("mcp")
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut call = |id: u64, name: &str, arguments: serde_json::Value| -> serde_json::Value {
        writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})).unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let prepared = call(
        1,
        "prepare_review",
        serde_json::json!({"source":"codebase","paths":["src/**"]}),
    );
    assert_eq!(prepared["result"]["isError"], false, "{prepared}");
    let input: serde_json::Value =
        serde_json::from_str(prepared["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(input["source"], "codebase");
    assert_eq!(input["files"].as_array().unwrap().len(), 1);
    assert!(
        input["review_instructions"]
            .as_str()
            .unwrap()
            .contains("Codebase review mode")
    );
    let snapshot = input["snapshot_sha256"].as_str().unwrap();
    let review = serde_json::json!({"model":"host","files_reviewed":1,"summary":"ok","findings":[
        {"severity":"low","path":"src/old.py","line":2,"title":"t","message":"m","suggestion":"s","confidence":0.9}]});
    let validated = call(
        2,
        "validate_review",
        serde_json::json!({"source":"codebase","paths":["src/**"],"snapshot_sha256":snapshot,"review":review}),
    );
    assert_eq!(validated["result"]["isError"], false, "{validated}");
    // A line outside the selected file is still rejected.
    let mut outside = review;
    outside["findings"][0]["line"] = serde_json::json!(9);
    let rejected = call(
        4,
        "validate_review",
        serde_json::json!({"source":"codebase","paths":["src/**"],"snapshot_sha256":snapshot,"review":outside}),
    );
    assert_eq!(rejected["result"]["isError"], true, "{rejected}");
    // `paths` belongs to codebase reviews only.
    let refused = call(
        3,
        "prepare_review",
        serde_json::json!({"source":"local","paths":["src/**"]}),
    );
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn stdio_server_outside_the_repository_uses_repository_path_and_a_commit_range() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .current_dir(repo.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid"
                ])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "base"]);
    git(&["switch", "-qc", "feature-a"]);
    std::fs::write(repo.path().join("a.txt"), "a\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "a"]);
    git(&["switch", "-qc", "feature-b"]);
    std::fs::write(repo.path().join("b.txt"), "b\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "b"]);
    // An uncommitted edit, outside any committed range.
    std::fs::write(repo.path().join("base.txt"), "edited\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_cururu"))
        .arg("mcp")
        .current_dir(outside.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", outside.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut call = |id: u32, arguments: serde_json::Value| {
        writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"prepare_review","arguments":arguments}})).unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        let result = response["result"].clone();
        (
            result["isError"] == true,
            result["content"][0]["text"].as_str().unwrap().to_owned(),
        )
    };
    let files = |text: &str| -> Vec<String> {
        let prepared: serde_json::Value = serde_json::from_str(text).unwrap();
        prepared["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap().to_owned())
            .collect()
    };
    let (error, text) = call(1, serde_json::json!({"source":"local"}));
    assert!(error && text.contains("is not a Git repository"), "{text}");
    let path = repo.path().to_str().unwrap();
    let (error, text) = call(
        2,
        serde_json::json!({"source":"local","repository_path":path}),
    );
    assert!(!error, "{text}");
    assert_eq!(files(&text), ["base.txt"]);
    let (error, text) = call(
        3,
        serde_json::json!({"source":"local","repository_path":path,"base":"feature-a","head":"feature-b"}),
    );
    assert!(!error, "{text}");
    assert_eq!(files(&text), ["b.txt"]);
    let (error, text) = call(
        4,
        serde_json::json!({"source":"local","repository_path":path,"base":"no-such-branch"}),
    );
    assert!(error && text.contains("is not a commit"), "{text}");
    let (error, text) = call(
        5,
        serde_json::json!({"source":"local","repository_path":path,"base":"--output=/tmp/x"}),
    );
    assert!(error && text.contains("invalid revision"), "{text}");
    drop(stdin);
    assert!(child.wait_with_output().unwrap().status.success());
}
