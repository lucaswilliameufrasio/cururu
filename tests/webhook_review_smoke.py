"""Signed webhook through the real SQLite worker and loopback GitHub/LLM fakes."""
import hashlib
import hmac
import http.server
import json
import os
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

head = "a" * 40
test_evaluator = len(sys.argv) > 2 and sys.argv[2] == "--test-evaluator"
state = {"changed": False, "mutate": False, "models": 0, "evaluations": 0, "writes": []}


class Fake(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, value):
        data = value.encode() if isinstance(value, str) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.path = self.path.replace("/repos/other/repo/", "/repos/test/repo/")
        if self.path == "/repos/test/repo/pulls/1":
            if self.headers.get("Accept") == "application/vnd.github.diff":
                self.reply(200, "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n")
            else:
                self.reply(200, {"head": {"sha": head, "ref": "branch"}, "base": {"sha": head, "ref": "main"}})
        elif self.path.startswith("/repos/test/repo/contents/.cururu.toml?"):
            value = "true" if state["changed"] and state["mutate"] else "false"
            self.reply(200, "version = 1\n[policy]\nsuggested_changes = " + value + "\n")
        elif self.path.startswith("/repos/test/repo/git/ref/heads/"):
            self.reply(200, {"object": {"sha": head}})
        elif self.path == "/user":
            self.reply(200, {"login": "fixture[bot]"})
        elif self.path.startswith(("/repos/test/repo/pulls/1/comments?", "/repos/test/repo/issues/1/comments?", "/repos/test/repo/pulls/1/reviews?")):
            self.reply(200, [])
        else:
            self.reply(404, {})

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        if self.path == "/app/installations/1/access_tokens":
            self.reply(201, {"token": "synthetic-token"})
        elif self.path == "/chat/completions":
            state["models"] += 1
            state["changed"] = not test_evaluator
            review = {"model": "fixture", "files_reviewed": 1, "summary": "ok", "findings": []}
            if test_evaluator:
                review["findings"] = [{"severity": "high", "path": "a", "line": 1, "title": "fixture", "message": "issue", "suggestion": "fix", "confidence": 0.99}]
            self.reply(200, {"choices": [{"message": {"content": json.dumps(review)}, "finish_reason": "stop"}]})
        elif self.path == "/synthetic-evaluator":
            assert test_evaluator
            assert self.headers.get("Authorization") == "Bearer fixture-evaluator"
            assert len(json.loads(body)["state"]) == 1
            state["evaluations"] += 1
            state["changed"] = True
            self.reply(200, {"model": "fixture-evaluator", "answers": {
                "finding_0_is_defect": {"type": "noul", "noul": 0.9},
                "finding_0_severity": {"type": "choice", "choice": "high", "confidence": 0.9}}})
        else:
            state["writes"].append((self.path, body))
            self.reply(201, {"id": 42})


fake = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fake)
threading.Thread(target=fake.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory() as directory:
    key = os.path.join(directory, "synthetic.pem")
    subprocess.run(["openssl", "genrsa", "-out", key, "2048"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    database = os.path.join(directory, "queue.sqlite")
    sqlite3.connect(database).close()
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    url = "http://127.0.0.1:%d" % port
    fake_url = "http://127.0.0.1:%d" % fake.server_port
    env = {"PATH": os.environ.get("PATH", ""), "HOME": directory, "GITHUB_APP_ID": "1",
           "GITHUB_APP_PRIVATE_KEY_PATH": key, "GITHUB_APP_SLUG": "fixture", "GITHUB_WEBHOOK_SECRET": "fixture-secret",
           "CURURU_DATABASE_URL": "sqlite:" + database, "CURURU_HOST": "127.0.0.1", "PORT": str(port),
           "GITHUB_API_URL": fake_url, "LLM_BASE_URL": fake_url, "LLM_API_KEY": "fixture",
           "CURURU_EVALUATOR_MODE": "observe", "CURURU_EVALUATOR_ALLOWED_REPOSITORIES": "test/repo",
           "CURURU_REPOSITORY": "test/repo"}
    if test_evaluator:
        env["CURURU_TEST_JEV_ENDPOINT"] = fake_url + "/synthetic-evaluator"
        env["CURURU_RUN_SYNTHETIC_HTTP_TEST_SERVER"] = "1"
        env["TYPESAFE_API_KEY"] = "fixture-evaluator"
        server_args = [sys.argv[1], "--exact", "app::tests::synthetic_http_test_server", "--nocapture"]
    else:
        server_args = [sys.argv[1], "serve"]
    with tempfile.TemporaryFile(mode="w+") as log:
        process = subprocess.Popen(server_args, cwd=directory, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 20
            while True:
                try:
                    urllib.request.urlopen(url + "/health", timeout=1).close()
                    break
                except OSError:
                    assert process.poll() is None and time.monotonic() < deadline
                    time.sleep(0.05)
            for scenario in ("stable", "unauthorized", "changed"):
                mutate = scenario == "changed"
                state.update(changed=False, mutate=mutate, models=0, evaluations=0, writes=[])
                delivery = scenario
                repository = "other/repo" if scenario == "unauthorized" else "test/repo"
                body = json.dumps({"action": "synchronize", "installation": {"id": 1}, "repository": {"full_name": repository}, "pull_request": {"number": 1, "draft": False}}).encode()
                signature = hmac.new(b"fixture-secret", body, hashlib.sha256).hexdigest()
                request = urllib.request.Request(url + "/v1/webhooks/github", data=body, headers={"X-Hub-Signature-256": "sha256=" + signature, "X-GitHub-Delivery": delivery, "X-GitHub-Event": "pull_request"})
                assert urllib.request.urlopen(request, timeout=5).status == 202
                deadline = time.monotonic() + 20
                while True:
                    db = sqlite3.connect(database)
                    try:
                        row = db.execute("SELECT state, attempts FROM github_deliveries WHERE delivery_id = ?", (delivery,)).fetchone()
                    finally:
                        db.close()
                    if row and ((scenario == "stable" and row[0] == "done") or (scenario != "stable" and row[0] == "queued" and row[1] >= 1)):
                        break
                    if process.poll() is not None or time.monotonic() >= deadline:
                        log.flush()
                        log.seek(0)
                        raise AssertionError((delivery, row, log.read()))
                    time.sleep(0.05)
                assert state["models"] == (0 if scenario == "unauthorized" else 1), state
                assert state["evaluations"] == (1 if test_evaluator and scenario != "unauthorized" else 0), state
                if scenario != "stable":
                    assert not state["writes"], state["writes"]
                else:
                    paths = [item[0] for item in state["writes"]]
                    assert "/repos/test/repo/pulls/1/reviews" in paths, paths
                    assert "/repos/test/repo/pulls/1/reviews/42/events" in paths, paths
                    assert "/repos/test/repo/issues/1/comments" in paths, paths
        finally:
            process.terminate()
            process.wait(timeout=10)
fake.shutdown()
