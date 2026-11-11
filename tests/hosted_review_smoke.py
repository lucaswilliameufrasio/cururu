"""Exercise hosted review publication against loopback-only synthetic services."""
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading

head = "a" * 40
state = {"mode": "stable", "changed": False, "models": 0, "posts": []}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, value):
        body = value.encode() if isinstance(value, str) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/repos/test/repo/pulls/1":
            if self.headers.get("Accept") == "application/vnd.github.diff":
                if state["mode"] in ("multi_stable", "multi_bad_coverage"):
                    self.reply(200, "".join(f"diff --git a/{name} b/{name}\n@@ -1 +1 @@\n-{'old' * 20}\n+{'new' * 20}\n" for name in ("a", "b")))
                else:
                    self.reply(200, "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n")
            else:
                self.reply(200, {"head": {"sha": head, "ref": "branch"}, "base": {"sha": head, "ref": "main"}})
        elif self.path.startswith("/repos/test/repo/git/ref/heads/"):
            self.reply(200, {"object": {"sha": head}})
        elif self.path.startswith("/repos/test/repo/contents/.cururu.toml?"):
            if state["mode"] in ("shared_stable", "shared_change", "shared_error"):
                self.reply(200, "version = 1\n[config]\nbase = 'test/shared'\nbase_ref = '" + head + "'\nbase_path = 'base.toml'\n")
                return
            policy = "true" if state["mode"] == "policy_change" and state["changed"] else "false"
            self.reply(200, "version = 1\n[analysis]\nenabled = true\ncheck_runs = true\n[policy]\nsuggested_changes = " + policy + "\n")
        elif self.path.startswith("/repos/test/shared/contents/base.toml?"):
            assert self.headers.get("Authorization") == "Bearer fixture-shared-token"
            if state["mode"] == "shared_error" and state["changed"]:
                self.reply(503, {})
            else:
                policy = "true" if state["mode"] == "shared_change" and state["changed"] else "false"
                self.reply(200, "version = 1\n[policy]\nsuggested_changes = " + policy + "\n")
        elif self.path == "/user":
            self.reply(200, {"login": "fixture"})
        elif self.path.startswith("/repos/test/repo/pulls/1/comments?"):
            text = "changed" if state["changed"] and state["mode"] == "feedback" else "original"
            self.reply(200, [{"id": 1, "body": "<!-- cururu:finding --> fixture", "user": {"login": "fixture", "type": "Bot"}, "path": "a", "line": 1},
                             {"id": 2, "in_reply_to_id": 1, "body": text, "user": {"login": "person", "type": "User"}, "path": "a", "line": 1}])
        elif self.path.startswith("/repos/test/repo/issues/1/comments?"):
            self.reply(200, [])
        elif self.path.startswith(f"/repos/test/repo/commits/{head}/check-runs?"):
            if state["mode"] == "malformed" or (state["mode"] == "malformed_after" and state["changed"]):
                self.reply(200, {})
            else:
                self.reply(200, {"check_runs": [{"id": 10, "name": "fixture", "conclusion": "failure"}]})
        elif self.path.startswith("/repos/test/repo/check-runs/10/annotations?"):
            text = "changed" if state["changed"] and state["mode"] == "evidence" else "original"
            self.reply(200, [{"path": "a", "start_line": 1, "annotation_level": "warning", "message": text}])
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        if self.path == "/chat/completions":
            state["models"] += 1
            state["changed"] = True
            review = {"model": "fixture", "files_reviewed": 1, "summary": "", "findings": []}
            if state["mode"] == "multi_bad_coverage" and state["models"] == 2:
                review["files_reviewed"] = 0
            self.reply(200, {"choices": [{"message": {"content": json.dumps(review)}, "finish_reason": "stop"}]})
        else:
            state["posts"].append((self.path, body))
            self.reply(201, {})


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory() as directory:
    url = "http://127.0.0.1:%d" % server.server_port
    env = {"PATH": os.environ.get("PATH", ""), "HOME": directory, "CURURU_REPOSITORY": "test/repo",
           "CURURU_CHANGE_REQUEST_NUMBER": "1", "CURURU_SCM_TOKEN": "fixture-scm",
           "CURURU_SCM_API_URL": url, "CURURU_SCM_SERVER_URL": "https://example.invalid",
           "CURURU_PROVIDER": "openai", "LLM_BASE_URL": url, "LLM_API_KEY": "fixture-api"}
    filter_env = dict(env, CURURU_EVALUATOR_MODE="filter", CURURU_EVALUATOR_ALLOWED_REPOSITORIES="test/repo", TYPESAFE_API_KEY="fixture-evaluator")
    rejected = subprocess.run([sys.argv[1], "review"], cwd=directory, env=filter_env, capture_output=True, text=True, timeout=30)
    assert rejected.returncode != 0 and "evaluator filtering is disabled" in rejected.stderr, rejected.stderr
    assert state["models"] == 0 and not state["posts"]
    rejected = subprocess.run([sys.argv[1], "dry-run"], cwd=directory, env=filter_env, capture_output=True, text=True, timeout=30)
    assert rejected.returncode != 0 and "evaluator filtering is disabled" in rejected.stderr, rejected.stderr
    assert state["models"] == 0 and not state["posts"]
    for mode in ("multi_stable", "multi_bad_coverage"):
        state.update(mode=mode, changed=False, models=0, posts=[])
        multi_env = dict(env, CURURU_CHUNK_BYTES="250")
        result = subprocess.run([sys.argv[1], "review"], cwd=directory, env=multi_env, capture_output=True, text=True, timeout=30)
        assert state["models"] == 2, (mode, result.stderr)
        if mode == "multi_stable":
            assert result.returncode == 0 and state["posts"][-1][0] == "/repos/test/repo/issues/1/comments", result.stderr
        else:
            assert result.returncode != 0 and "chunk review reports incomplete or inconsistent file coverage" in result.stderr, result.stderr
            assert not result.stdout and not state["posts"]
    env["CURURU_SHARED_CONFIG_TOKEN"] = "fixture-shared-token"
    for mode in ("malformed", "malformed_after", "feedback", "evidence", "policy_change", "shared_change", "shared_error", "shared_stable", "stable"):
        state.update(mode=mode, changed=False, models=0, posts=[])
        result = subprocess.run([sys.argv[1], "review"], cwd=directory, env=env, capture_output=True, text=True, timeout=30)
        if mode == "malformed":
            assert result.returncode != 0 and state["models"] == 0, result.stderr
            assert not state["posts"]
        elif mode == "malformed_after":
            assert result.returncode != 0 and state["models"] == 1, result.stderr
            assert not state["posts"]
        elif mode in ("stable", "shared_stable"):
            assert state["models"] == 1, result.stderr
            assert result.returncode == 0, result.stderr
            assert state["posts"] and state["posts"][-1][0] == "/repos/test/repo/issues/1/comments"
        elif mode == "policy_change":
            assert state["models"] == 1 and result.returncode != 0, result.stderr
            assert not state["posts"]
            assert "configuration changed" in result.stderr, result.stderr
        elif mode in ("shared_change", "shared_error"):
            assert state["models"] == 1 and result.returncode != 0, result.stderr
            assert not state["posts"]
            assert "trusted shared configuration" in result.stderr, result.stderr
            assert "fixture-shared-token" not in result.stderr
        else:
            assert state["models"] == 1, result.stderr
            assert result.returncode != 0 and "context or analyzer evidence changed" in result.stderr, result.stderr
            assert not state["posts"], mode
server.shutdown()
