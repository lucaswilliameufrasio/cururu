use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn stdio_multichunk_requires_all_results_in_order() {
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
    assert_eq!(
        call(
            2,
            "assemble_review",
            arguments(serde_json::json!([reviews[0]]))
        )["isError"],
        true
    );
    assert_eq!(
        call(
            3,
            "assemble_review",
            arguments(serde_json::json!([reviews[1], reviews[0]]))
        )["isError"],
        true
    );
    let complete = call(4, "assemble_review", arguments(serde_json::json!(reviews)));
    assert_eq!(complete["isError"], false);
    let result: serde_json::Value =
        serde_json::from_str(complete["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["files_reviewed"], 2);
    drop(call);
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
