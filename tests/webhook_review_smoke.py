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
state = {"changed": False, "mutate": False, "models": 0, "evaluations": 0, "writes": [], "checks": [], "check_updates": [], "head": head}


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
                self.reply(200, {"head": {"sha": state["head"], "ref": "branch"}, "base": {"sha": head, "ref": "main"}})
        elif self.path.startswith("/repos/test/repo/contents/.cururu.toml?"):
            value = "true" if state["changed"] and state["mutate"] else "false"
            self.reply(200, "version = 1\n[policy]\nsuggested_changes = " + value + "\n[summary]\nshow_cost = true\nshow_usage = true\n")
        elif self.path.startswith("/repos/test/repo/git/ref/heads/"):
            self.reply(200, {"object": {"sha": head}})
        elif self.path.startswith("/repos/test/repo/pulls/1/commits?"):
            self.reply(200, [{"sha": head}, {"sha": "b" * 40}])
        elif "/check-runs?" in self.path and self.path.startswith("/repos/test/repo/commits/"):
            sha = self.path.split("/")[5]
            self.reply(200, {"check_runs": [{"id": 900 + index, "external_id": check["external_id"], "output": check["output"]}
                                            for index, check in enumerate(state["checks"], 1) if check["head_sha"] == sha]})
        elif self.path.startswith("/repos/test/repo/collaborators/"):
            login = self.path.split("/")[5]
            self.reply(200, {"permission": "write" if login == "alice" else "read"})
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
            self.reply(200, {"choices": [{"message": {"content": json.dumps(review)}, "finish_reason": "stop"}],
                             "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120, "cost": 0.25}})
        elif self.path == "/synthetic-evaluator":
            assert test_evaluator
            assert self.headers.get("Authorization") == "Bearer fixture-evaluator"
            assert len(json.loads(body)["state"]) == 1
            state["evaluations"] += 1
            state["changed"] = True
            self.reply(200, {"model": "fixture-evaluator", "answers": {
                "finding_0_is_defect": {"type": "noul", "noul": 0.9},
                "finding_0_severity": {"type": "choice", "choice": "high", "confidence": 0.9}}})
        elif self.path == "/repos/test/repo/check-runs":
            state["checks"].append(json.loads(body))
            self.reply(201, {"id": 900 + len(state["checks"])})
        else:
            state["writes"].append((self.path, body))
            self.reply(201, {"id": 42})

    def do_PATCH(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path.startswith("/repos/test/repo/check-runs/"):
            state["check_updates"].append((self.path, json.loads(body)))
            self.reply(200, {})
        else:
            state["writes"].append((self.path, body))
            self.reply(200, {})


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
            def send(event, payload, delivery):
                body = json.dumps(payload).encode()
                signature = hmac.new(b"fixture-secret", body, hashlib.sha256).hexdigest()
                request = urllib.request.Request(url + "/v1/webhooks/github", data=body, headers={"X-Hub-Signature-256": "sha256=" + signature, "X-GitHub-Delivery": delivery, "X-GitHub-Event": event})
                assert urllib.request.urlopen(request, timeout=5).status == 202

            def wait(delivery, done):
                deadline = time.monotonic() + 20
                while True:
                    db = sqlite3.connect(database)
                    try:
                        row = db.execute("SELECT state, attempts FROM github_deliveries WHERE delivery_id = ?", (delivery,)).fetchone()
                    finally:
                        db.close()
                    if row and ((done and row[0] == "done") or (not done and row[0] == "queued" and row[1] >= 1)):
                        return
                    if process.poll() is not None or time.monotonic() >= deadline:
                        log.flush()
                        log.seek(0)
                        raise AssertionError((delivery, row, log.read()))
                    time.sleep(0.05)

            def candidate_state(digest):
                db = sqlite3.connect(database)
                try:
                    return db.execute("SELECT state FROM hosted_candidates WHERE digest = ?", (digest,)).fetchone()[0]
                finally:
                    db.close()

            installation = {"id": 1}
            repo = {"full_name": "test/repo"}

            def press(check_id, login, delivery):
                send("check_run", {"action": "requested_action", "requested_action": {"identifier": "cururu_publish"}, "check_run": {"id": check_id}, "sender": {"login": login}, "repository": repo, "installation": installation}, delivery)
                wait(delivery, True)

            def comment(text, login, delivery):
                send("issue_comment", {"action": "created", "issue": {"number": 1, "pull_request": {}}, "comment": {"body": text, "user": {"login": login, "type": "User"}}, "repository": repo, "installation": installation}, delivery)
                wait(delivery, True)

            for scenario in ("stable", "unauthorized", "changed"):
                mutate = scenario == "changed"
                state.update(changed=False, mutate=mutate, models=0, evaluations=0, writes=[], checks=[], check_updates=[])
                repository = "other/repo" if scenario == "unauthorized" else "test/repo"
                send("pull_request", {"action": "synchronize", "installation": installation, "repository": {"full_name": repository}, "pull_request": {"number": 1, "draft": False}}, scenario)
                wait(scenario, scenario == "stable")
                assert state["models"] == (0 if scenario == "unauthorized" else 1), state
                assert state["evaluations"] == (1 if test_evaluator and scenario != "unauthorized" else 0), state
                # Preparing a candidate never comments on the PR.
                assert not state["writes"], state["writes"]
                if scenario != "stable":
                    assert not state["checks"], state["checks"]
                    continue
                assert len(state["checks"]) == 1, state["checks"]
                check = state["checks"][0]
                text = check["output"]["text"]
                digest = hashlib.sha256(text.encode()).hexdigest()
                assert check["external_id"] == digest and check["actions"][0]["identifier"] == "cururu_publish", check
                assert digest in check["output"]["summary"]
                assert candidate_state(digest) == "pending"
                state["mutate"] = False
                state["changed"] = False

                press(901, "mallory", "press-without-write-access")
                comment("/cururu approve " + "0" * 64, "alice", "approve-unknown-digest")
                assert not state["writes"] and candidate_state(digest) == "pending", state["writes"]

                comment("/cururu approve " + digest, "alice", "approve-by-comment")
                paths = [item[0] for item in state["writes"]]
                assert "/repos/test/repo/pulls/1/reviews" in paths, paths
                assert "/repos/test/repo/pulls/1/reviews/42/events" in paths, paths
                assert "/repos/test/repo/issues/1/comments" in paths, paths
                summary = [item[1].decode() for item in state["writes"] if item[0] == "/repos/test/repo/issues/1/comments"][0]
                assert json.loads(summary)["body"] in text
                assert "**Tokens (this review):** 100 prompt + 20 completion = 120 total" in text, text
                assert "PR total, 1 review(s)" in text and "`$0.250000`" in text, text
                assert "cururu:usage:v1" in check["output"]["summary"]
                assert candidate_state(digest) == "published"
                assert state["check_updates"][-1][1]["conclusion"] == "success", state["check_updates"]
                published = len(state["writes"])

                press(901, "alice", "duplicate-approval")
                assert len(state["writes"]) == published, state["writes"]

                # A newer head produces a new candidate; approving it after
                # another push invalidates it without publishing.
                state["head"] = "b" * 40
                send("pull_request", {"action": "synchronize", "installation": installation, "repository": repo, "pull_request": {"number": 1, "draft": False}}, "second-head")
                wait("second-head", True)
                assert len(state["checks"]) == 2, state["checks"]
                second_text = state["checks"][1]["output"]["text"]
                stale = hashlib.sha256(second_text.encode()).hexdigest()
                # The second run counts the first one, published or not.
                assert "**Tokens (PR total, 2 review(s)):** 200 prompt + 40 completion = 240 total" in second_text, second_text
                assert "**Provider-reported cost (PR total, 2 review(s)):** `$0.500000`" in second_text, second_text
                state["head"] = "c" * 40
                press(902, "alice", "stale-approval")
                assert len(state["writes"]) == published, state["writes"]
                assert candidate_state(stale) == "invalidated"
                assert state["check_updates"][-1][1]["conclusion"] == "neutral", state["check_updates"]
                state["head"] = head
        finally:
            process.terminate()
            process.wait(timeout=10)
fake.shutdown()
