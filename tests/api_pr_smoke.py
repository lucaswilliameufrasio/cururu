"""Loopback-only hosted API review test; never contacts a real provider or SCM."""
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading

head = "a" * 40
state = {"diff": "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n", "change_after_send": False, "excess_feedback": False, "malformed_checks": False, "check_requests": 0, "api_calls": 0, "other_posts": 0, "multi_mode": None, "multi_calls": 0, "evidence_path": None, "suggestion_policy": None, "change_policy_after_send": False}

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, value):
        data = value.encode() if isinstance(value, str) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/repos/test/repo/pulls/1":
            if self.headers.get("Accept") == "application/vnd.github.diff":
                self.reply(200, state["diff"])
            else:
                self.reply(200, {"head": {"sha": head, "ref": "branch"}, "base": {"sha": head, "ref": "main"}})
        elif self.path.startswith("/repos/test/repo/git/ref/heads/"):
            self.reply(200, {"object": {"sha": head}})
        elif self.path.startswith("/repos/test/repo/contents/.cururu.toml?") and state["malformed_checks"]:
            self.reply(200, "version = 1\n[analysis]\nenabled = true\ncheck_runs = true\n")
        elif self.path.startswith("/repos/test/repo/contents/.cururu.toml?") and state["suggestion_policy"] is not None:
            self.reply(200, "version = 1\n[policy]\nsuggested_changes = " + ("true" if state["suggestion_policy"] else "false") + "\n")
        elif self.path.startswith(f"/repos/test/repo/commits/{head}/check-runs?") and state["malformed_checks"]:
            state["check_requests"] += 1
            self.reply(200, {})
        elif "/contents/" in self.path:
            self.reply(404, {})
        elif self.path == "/user":
            self.reply(200, {"login": "fixture"})
        elif self.path.startswith("/repos/test/repo/issues/1/comments?") and state["excess_feedback"]:
            self.reply(200, [{"id": 1, "body": "<!-- cururu:summary -->", "user": {"login": "fixture", "type": "Bot"}}] +
                       [{"id": index + 2, "body": "fixture", "user": {"login": "person", "type": "User"}} for index in range(11)])
        elif "/comments?" in self.path:
            self.reply(200, [])
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        if self.path != "/chat/completions":
            state["other_posts"] += 1
            self.reply(404, {})
            return
        state["api_calls"] += 1
        request = json.loads(body)
        assert "+new" in json.dumps(request)
        assert "prior_feedback" in json.dumps(request)
        if state["multi_mode"]:
            state["multi_calls"] += 1
            if state["multi_mode"] == "evidence_change" and state["multi_calls"] == 2:
                with open(state["evidence_path"], "a") as file:
                    file.write(" ")
        if state["change_after_send"]:
            state["diff"] = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+later\n"
        if state["change_policy_after_send"]:
            state["suggestion_policy"] = False
        review = {"model": "fixture", "files_reviewed": 1, "summary": "", "findings": []}
        if state["multi_mode"] == "bad_coverage" and state["multi_calls"] == 2:
            review["files_reviewed"] = 0
        reason = "length" if state["multi_mode"] == "late_failure" and state["multi_calls"] == 2 else "stop"
        self.reply(200, {"choices": [{"message": {"content": json.dumps(review)}, "finish_reason": reason}]})

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory() as directory:
    url = "http://127.0.0.1:%d" % server.server_port
    env = {"PATH": os.environ.get("PATH", ""), "HOME": directory, "CURURU_REPOSITORY": "test/repo",
           "CURURU_CHANGE_REQUEST_NUMBER": "1", "CURURU_SCM_TOKEN": "fixture-scm",
           "CURURU_SCM_API_URL": url, "CURURU_SCM_SERVER_URL": "https://example.invalid",
           "CURURU_PROVIDER": "openai", "LLM_BASE_URL": url, "LLM_API_KEY": "fixture-api"}
    def run(*args):
        return subprocess.run([sys.argv[1], *args], cwd=directory, env=env, capture_output=True, text=True, timeout=30)
    prepared = run("prepare-review", "--pr")
    assert prepared.returncode == 0, prepared.stderr
    snapshot = json.loads(prepared.stdout)["snapshot_sha256"]
    assert run("api-review", "--pr").returncode != 0
    assert run("api-review", "--pr", "--confirm-send", "--expected-snapshot", "stale").returncode != 0
    assert state["api_calls"] == 0
    state["excess_feedback"] = True
    rejected = run("api-review", "--pr", "--confirm-send")
    assert rejected.returncode != 0 and "comment limit" in rejected.stderr, rejected.stderr
    assert state["api_calls"] == 0 and state["other_posts"] == 0
    state["excess_feedback"] = False
    state["malformed_checks"] = True
    malformed = run("api-review", "--pr", "--confirm-send")
    assert malformed.returncode != 0, malformed.stderr
    assert "failed to list check runs" in malformed.stderr, malformed.stderr
    assert not malformed.stdout and state["check_requests"] > 0
    assert state["api_calls"] == 0 and state["other_posts"] == 0
    state["malformed_checks"] = False
    accepted = run("api-review", "--pr", "--confirm-send", "--expected-snapshot", snapshot)
    assert accepted.returncode == 0, accepted.stderr
    assert json.loads(accepted.stdout)["source"] == "pr"
    state["suggestion_policy"] = True
    policy_preview = run("prepare-review", "--pr")
    assert policy_preview.returncode == 0, policy_preview.stderr
    policy_snapshot = json.loads(policy_preview.stdout)["snapshot_sha256"]
    state["change_policy_after_send"] = True
    policy_changed = run("api-review", "--pr", "--confirm-send", "--expected-snapshot", policy_snapshot)
    assert policy_changed.returncode != 0 and "snapshot changed" in policy_changed.stderr, policy_changed.stderr
    assert not policy_changed.stdout and state["other_posts"] == 0
    state["change_policy_after_send"] = False
    state["suggestion_policy"] = None
    with open(os.path.join(directory, "analysis.sarif"), "w") as file:
        json.dump({"runs": [{"tool": {"driver": {"name": "fixture"}}, "results": []}]}, file)
    with open(os.path.join(directory, "manifest.json"), "w") as file:
        json.dump({"schema_version": 1, "commit_sha": head, "tools": []}, file)
    evidence = ("--sarif", "analysis.sarif", "--analysis-manifest", "manifest.json")
    preview = run("prepare-review", "--pr", *evidence)
    assert preview.returncode == 0, preview.stderr
    evidence_hash = json.loads(preview.stdout)["snapshot_sha256"]
    reviewed = run("api-review", "--pr", "--confirm-send", "--expected-snapshot", evidence_hash, *evidence)
    assert reviewed.returncode == 0, reviewed.stderr
    assert json.loads(reviewed.stdout)["snapshot_sha256"] == evidence_hash
    original_diff = state["diff"]
    state["diff"] = "".join(
        f"diff --git a/{name} b/{name}\n@@ -1 +1 @@\n-{'old' * 20}\n+{'new' * 20}\n"
        for name in ("a", "b")
    )
    env["CURURU_CHUNK_BYTES"] = "250"
    state["evidence_path"] = os.path.join(directory, "analysis.sarif")
    for mode in ("success", "late_failure", "evidence_change", "bad_coverage"):
        with open(state["evidence_path"], "w") as file:
            file.write('{"runs":[]}')
        preview = run("prepare-review", "--pr", *evidence)
        assert preview.returncode == 0, preview.stderr
        prepared = json.loads(preview.stdout)
        assert len(prepared["chunks"]) == 2
        state["multi_mode"] = mode
        state["multi_calls"] = 0
        result = run("api-review", "--pr", "--confirm-send", "--expected-snapshot", prepared["snapshot_sha256"], *evidence)
        assert state["multi_calls"] == 2
        if mode == "success":
            assert result.returncode == 0, result.stderr
            assert json.loads(result.stdout)["review"]["files_reviewed"] == 2
        else:
            assert result.returncode != 0 and not result.stdout, result.stderr
            if mode == "bad_coverage":
                assert "chunk review reports incomplete or inconsistent file coverage" in result.stderr, result.stderr
        assert state["other_posts"] == 0
    state["multi_mode"] = None
    state["diff"] = original_diff
    del env["CURURU_CHUNK_BYTES"]
    state["change_after_send"] = True
    stale = run("api-review", "--pr", "--confirm-send", "--expected-snapshot", snapshot)
    assert stale.returncode != 0 and "snapshot changed" in stale.stderr, stale.stderr
    assert state["api_calls"] == 12 and state["other_posts"] == 0
server.shutdown()
