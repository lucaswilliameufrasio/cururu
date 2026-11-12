"""Exercise opt-in auto-approval in the Action against loopback-only fakes."""
import hashlib
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading

head = "a" * 40
state = {}

POLICY = """version = 1
[policy]
suggested_changes = false
[auto_approval]
enabled = {enabled}
allowed_paths = ["docs/**", "**/*.md"]
blocked_paths = ["**/payments/**"]
max_changed_files = 5
max_changed_lines = 50
"""


def reset(path, findings=None, enabled="true", approval_status=200, reviews=None):
    state.clear()
    state.update(path=path, findings=findings or [], enabled=enabled, approval_status=approval_status,
                 reviews=reviews or [], posts=[], patches=[], puts=[], checks=[])


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
        path = state["path"]
        if self.path == "/repos/test/repo/pulls/1":
            if self.headers.get("Accept") == "application/vnd.github.diff":
                self.reply(200, f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+new\n")
            else:
                self.reply(200, {"head": {"sha": head, "ref": "branch"}, "base": {"sha": head, "ref": "main"}})
        elif self.path.startswith("/repos/test/repo/git/ref/heads/"):
            self.reply(200, {"object": {"sha": head}})
        elif self.path.startswith("/repos/test/repo/contents/.cururu.toml?"):
            self.reply(200, POLICY.format(enabled=state["enabled"]))
        elif self.path == "/user":
            self.reply(200, {"login": "fixture[bot]"})
        elif self.path.startswith("/repos/test/repo/pulls/1/reviews?"):
            self.reply(200, state["reviews"])
        elif self.path.startswith("/repos/test/repo/pulls/1/commits?"):
            self.reply(200, [{"sha": head}])
        elif self.path.startswith(("/repos/test/repo/pulls/1/comments?", "/repos/test/repo/issues/1/comments?")):
            self.reply(200, [])
        elif "/check-runs?" in self.path:
            self.reply(200, {"check_runs": []})
        else:
            self.reply(404, {})

    def body(self):
        return json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))) or b"{}")

    def do_POST(self):
        body = self.body()
        if self.path == "/chat/completions":
            review = {"model": "fixture", "files_reviewed": 1, "summary": "Reviewed the change.", "findings": state["findings"]}
            self.reply(200, {"choices": [{"message": {"content": json.dumps(review)}, "finish_reason": "stop"}]})
        elif self.path == "/repos/test/repo/check-runs":
            state["checks"].append(body)
            self.reply(201, {"id": 70 + len(state["checks"])})
        elif self.path == "/repos/test/repo/pulls/1/reviews" and body.get("event") == "APPROVE":
            state["posts"].append((self.path, body))
            if state["approval_status"] == 200:
                self.reply(200, {"id": 9})
            else:
                self.reply(state["approval_status"], {"message": "Unprocessable Entity",
                                                      "errors": ["GitHub Actions is not permitted to approve pull requests."]})
        else:
            state["posts"].append((self.path, body))
            self.reply(201, {"id": 5})

    def do_PATCH(self):
        state["patches"].append((self.path, self.body()))
        self.reply(200, {})

    def do_PUT(self):
        state["puts"].append((self.path, self.body()))
        self.reply(200, {})


def finding(severity, path):
    return {"severity": severity, "path": path, "line": 1, "title": "Wording", "message": "issue", "suggestion": "fix", "confidence": 0.99}


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory() as directory:
    url = "http://127.0.0.1:%d" % server.server_port
    env = {"PATH": os.environ.get("PATH", ""), "HOME": directory, "CURURU_REPOSITORY": "test/repo",
           "CURURU_CHANGE_REQUEST_NUMBER": "1", "CURURU_SCM_TOKEN": "fixture-scm",
           "CURURU_SCM_API_URL": url, "CURURU_SCM_SERVER_URL": "https://github.example",
           "CURURU_PROVIDER": "openai", "LLM_BASE_URL": url, "LLM_API_KEY": "fixture-api",
           "CURURU_LANGUAGE": "pt-BR", "GITHUB_RUN_ID": "5"}

    def review(name, **extra):
        output = os.path.join(directory, "output-" + name)
        open(output, "w").close()
        result = subprocess.run([sys.argv[1], "review"], cwd=directory, env=dict(env, GITHUB_OUTPUT=output, **extra),
                                capture_output=True, text=True, timeout=30)
        assert result.returncode == 0, (name, result.stderr)
        lines = open(output).read().splitlines()
        return dict(line.split("=", 1) for line in lines if "=" in line)

    def paths():
        return [item[0] for item in state["posts"]]

    def approvals():
        return [item for item in state["posts"] if item[1].get("event") == "APPROVE"]

    # Scenario 2: a simple docs change is approved, then its review is published directly.
    reset("docs/guide.md")
    out = review("docs")
    assert out["auto_approval"] == "approved" and out["review_status"] == "completed", out
    assert json.loads(out["auto_approval_reasons"]) == [] and out["pr_url"] == "https://github.example/test/repo/pull/1", out
    approval = approvals()[0][1]
    assert approval["commit_id"] == head and "Auto-aprovado" in approval["body"], approval
    assert paths().index("/repos/test/repo/pulls/1/reviews") < paths().index("/repos/test/repo/issues/1/comments"), paths()
    summary = [b for p, b in state["posts"] if p == "/repos/test/repo/issues/1/comments"][-1]["body"]
    assert "Auto-aprovado" in summary, summary
    triage = [c for c in state["checks"] if c["name"] == "Cururu triage"]
    assert triage and triage[0]["conclusion"] == "success", state["checks"]
    assert not [c for c in state["checks"] if c["name"] == "Cururu review"], "no candidate when approved"

    # Scenario 1: a payments change needs a human; nothing is approved and only the notice is posted.
    reset("src/payments/fees.rs")
    out = review("payments")
    assert out["auto_approval"] == "needs_human" and out["review_status"] == "pending_approval", out
    reasons = json.loads(out["auto_approval_reasons"])
    assert {r["code"] for r in reasons} >= {"blocked_path", "outside_allowed_paths"}, reasons
    assert all(r["message"] for r in reasons)
    assert not approvals(), state["posts"]
    notices = [b["body"] for p, b in state["posts"] if p == "/repos/test/repo/issues/1/comments"]
    assert len(notices) == 1 and "Precisa de review humano" in notices[0] and "caminho protegido" in notices[0], notices
    assert "Reviewed the change." not in notices[0], "the notice carries reasons only"
    assert [c for c in state["checks"] if c["name"] == "Cururu review"], "regular candidate flow"
    assert [c for c in state["checks"] if c["name"] == "Cururu triage"][0]["conclusion"] == "neutral"

    # A docs change with a finding is not approved until a re-review finds nothing.
    reset("docs/guide.md", findings=[finding("low", "docs/guide.md")])
    out = review("docs-finding")
    assert out["auto_approval"] == "needs_human" and not approvals(), out
    assert json.loads(out["auto_approval_reasons"])[0]["code"] == "finding"

    # GitHub refusing the approval turns the decision into a human review and explains the setting.
    reset("docs/guide.md", approval_status=422)
    out = review("refused")
    assert out["auto_approval"] == "needs_human" and out["review_status"] == "pending_approval", out
    reasons = json.loads(out["auto_approval_reasons"])
    assert reasons[-1]["code"] == "approval_not_permitted" and "Allow GitHub Actions" in reasons[-1]["message"], reasons
    notice = [b["body"] for p, b in state["posts"] if p == "/repos/test/repo/issues/1/comments"][0]
    assert "Allow GitHub Actions to create and approve pull requests" in notice, notice
    candidate = json.load(open(os.path.join(directory, "cururu-candidate.json")))
    assert "Auto-aprovado" not in candidate["summary"] and "Precisa de review humano" in candidate["summary"]

    # A new head dismisses Cururu's earlier auto-approval before re-triage.
    old = "<!-- cururu:auto-approval:approved head=" + "b" * 40 + " -->"
    reset("docs/guide.md", reviews=[{"id": 3, "state": "APPROVED", "body": old, "user": {"login": "fixture[bot]", "type": "Bot"}}])
    out = review("stale")
    assert state["puts"] and state["puts"][0][0] == "/repos/test/repo/pulls/1/reviews/3/dismissals", state["puts"]
    assert out["auto_approval"] == "approved"

    # A diff over the limit is not evaluated, and every output is still written.
    reset("docs/guide.md")
    out = review("too-large", CURURU_MAX_DIFF_BYTES="10")
    assert out["review_status"] == "skipped_diff_limit" and out["auto_approval"] == "not_evaluated", out
    assert out["auto_approval_reasons"] == "[]" and out["pr_url"] == "https://github.example/test/repo/pull/1", out
    assert not approvals(), state["posts"]

    # Disabled by default: no approval, no notice, regular flow.
    reset("docs/guide.md", enabled="false")
    out = review("disabled")
    assert out["auto_approval"] == "disabled" and out["review_status"] == "pending_approval", out
    assert not approvals() and not state["puts"]
    assert not [c for c in state["checks"] if c["name"] == "Cururu triage"]
server.shutdown()
