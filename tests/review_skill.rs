use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

fn cururu(root: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cururu"));
    cmd.current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root)
        .env("USERPROFILE", root);
    cmd
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn global_install_uses_each_agents_discovery_directory() {
    let root = tempfile::tempdir().unwrap();
    for (target, directory) in [
        ("claude", ".claude"),
        ("codex", ".agents"),
        ("grok", ".grok"),
        ("opencode", "xdg/opencode"),
    ] {
        success(
            cururu(root.path())
                .env("XDG_CONFIG_HOME", root.path().join("xdg"))
                .args(["install-skill", "--global", "--target", target])
                .output()
                .unwrap(),
        );
        assert!(
            root.path()
                .join(directory)
                .join("skills/cururu-review/SKILL.md")
                .is_file()
        );
    }
}

#[test]
fn install_is_idempotent_and_updates_only_after_a_preview() {
    let root = tempfile::tempdir().unwrap();
    let install = || {
        let mut cmd = cururu(root.path());
        cmd.args(["install-skill", "--target", "claude"]);
        cmd
    };
    success(install().output().unwrap());
    let path = root.path().join(".claude/skills/cururu-review/SKILL.md");
    let skill = fs::read_to_string(&path).unwrap();
    assert_eq!(
        skill,
        include_str!("../skills/cururu-review/SKILL.md"),
        "regenerate with cururu install-skill --print"
    );
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(success(install().output().unwrap()).contains("Already current"));
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    let edited = skill.replace(env!("CARGO_PKG_VERSION"), "0.0.1") + "\nmy local note\n";
    fs::write(&path, &edited).unwrap();
    let preview = install().output().unwrap();
    assert!(!preview.status.success());
    assert!(String::from_utf8_lossy(&preview.stderr).contains("Skill version differs"));
    assert!(String::from_utf8_lossy(&preview.stdout).contains("-my local note"));
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
    let mut update = install()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    update.stdin.take().unwrap().write_all(b"UPDATE\n").unwrap();
    success(update.wait_with_output().unwrap());
    assert_eq!(fs::read_to_string(path).unwrap(), skill);
}

#[tokio::test(flavor = "multi_thread")]
async fn range_review_uses_canonical_instructions_and_merged_base_policy() {
    let root = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        success(
            Command::new("git")
                .current_dir(root.path())
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap(),
        )
    };
    git(&["init", "-q"]);
    let shared_sha = "a".repeat(40);
    fs::write(root.path().join(".cururu.toml"),format!(
        "version = 1\n[config]\nbase='example/shared'\nbase_ref='{shared_sha}'\nbase_path='base.toml'\n[review]\ntone='direct'\n"
    )).unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "base"]);
    fs::write(root.path().join("file"), "changed\n").unwrap();
    git(&["add", "file"]);
    git(&["commit", "-qm", "head"]);
    fs::write(root.path().join(".cururu.toml"), "invalid").unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/example/shared/contents/base.toml"))
        .and(query_param("ref", shared_sha))
        .and(header("authorization", "Bearer fixture"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("version=1\n[review]\nlanguage='es'\ntone='didactic'\n"),
        )
        .expect(2)
        .mount(&server)
        .await;
    let prepare = || {
        let mut cmd = cururu(root.path());
        cmd.env("GITHUB_API_URL", server.uri())
            .env("CURURU_SHARED_CONFIG_TOKEN", "fixture")
            .args([
                "prepare-review",
                "--local",
                "--base",
                "HEAD^",
                "--head",
                "HEAD",
            ]);
        cmd
    };
    let snapshot: Value = serde_json::from_str(&success(prepare().output().unwrap())).unwrap();
    let instructions = include_str!("../skills/cururu-review/SKILL.md")
        .split_once("## Canonical review instructions and JSON contract\n\n")
        .unwrap()
        .1;
    assert_eq!(
        instructions,
        snapshot["review_instructions"].as_str().unwrap()
    );
    assert_eq!(snapshot["review_preferences"]["language"], "es");
    assert_eq!(snapshot["review_preferences"]["tone"], "direct");
    let parts = json!({"snapshot_sha256":snapshot["snapshot_sha256"],"reviews":[{
        "chunk_index":snapshot["chunks"][0]["index"],
        "review":{"model":"fixture","files_reviewed":1,"summary":"Reviewed","findings":[]}
    }]});
    let saved = root.path().join("parts.json");
    fs::write(&saved, parts.to_string()).unwrap();
    let result = success(prepare().arg("--assemble").arg(saved).output().unwrap());
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap()["files_reviewed"],
        1
    );
}
