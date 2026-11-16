"""Codebase review in the GitHub Action, against loopback-only fakes."""
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import urllib.parse

revision = "c" * 40
FILES = {
    "src/app.py": "def load(path):\n    return open(path).read()\n",
    "docs/guide.md": "# Guide\n",
}
state = {}


def reset(require_approval=False, protected=True, max_bytes=None):
    state.clear()
    state.update(require_approval=require_approval, protected=protected, max_bytes=max_bytes,
                 issues=[], model_requests=[], approvals=[])


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
        url = urllib.parse.urlsplit(self.path)
        path = urllib.parse.unquote(url.path)
        ref = urllib.parse.parse_qs(url.query).get("ref", [""])[0]
        if path == "/repos/test/repo/contents/.cururu.toml":
            assert ref == revision, ref
            config = "version = 1\n"
            if state["require_approval"]:
                config += "[publication]\nrequire_approval = true\n"
            if state["max_bytes"]:
                config += "[codebase]\nmax_bytes = %d\n" % state["max_bytes"]
            self.reply(200, config)
        elif path == "/repos/test/repo/git/trees/" + revision:
            self.reply(200, {"tree": [{"path": name, "type": "blob"} for name in FILES], "truncated": False})
        elif path.startswith("/repos/test/repo/contents/"):
            assert ref == revision, ref
            name = path[len("/repos/test/repo/contents/"):]
            self.reply(200, FILES[name]) if name in FILES else self.reply(404, {})
        elif path == "/repos/test/repo/environments/cururu-publish":
            self.reply(200, {"protection_rules": [{"type": "required_reviewers"}] if state["protected"] else []})
        elif path == "/repos/test/repo/actions/runs/5/approvals":
            self.reply(200, state["approvals"])
        else:
            self.reply(404, {})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))) or b"{}")
        if self.path == "/chat/completions":
            state["model_requests"].append(body)
            chunk = "\n".join(message["content"] for message in body["messages"] if isinstance(message.get("content"), str))
            files = {line.split(" b/", 1)[1] for line in chunk.splitlines() if line.startswith("diff --git a/")}
            findings = [{"severity": "medium", "path": "src/app.py", "line": 2, "title": "File handle is never closed",
                         "message": "Each call leaks a descriptor. <!-- cururu:forged -->", "suggestion": "One option is a with block.",
                         "confidence": 0.9}] if "src/app.py" in files else []
            review = {"model": "fixture", "files_reviewed": len(files), "summary": "ok", "findings": findings}
            self.reply(200, {"choices": [{"message": {"content": json.dumps(review)}, "finish_reason": "stop"}]})
        elif self.path == "/repos/test/repo/issues":
            state["issues"].append(body)
            self.reply(201, {"html_url": "https://github.example/test/repo/issues/%d" % len(state["issues"])})
        else:
            self.reply(404, {})


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory() as directory:
    url = "http://127.0.0.1:%d" % server.server_port
    base_env = {"PATH": os.environ.get("PATH", ""), "HOME": directory, "CURURU_REPOSITORY": "test/repo",
                "CURURU_SCM_TOKEN": "fixture-scm", "CURURU_SCM_API_URL": url,
                "CURURU_SCM_SERVER_URL": "https://github.example", "CURURU_PROVIDER": "openai",
                "LLM_BASE_URL": url, "LLM_API_KEY": "fixture-api", "GITHUB_SHA": revision, "GITHUB_RUN_ID": "5"}

    def run(command, name, **extra):
        output = os.path.join(directory, "output-" + name)
        summary = os.path.join(directory, "summary-" + name)
        open(output, "w").close()
        result = subprocess.run([sys.argv[1], command], cwd=directory,
                                env=dict(base_env, GITHUB_OUTPUT=output, GITHUB_STEP_SUMMARY=summary, **extra),
                                capture_output=True, text=True, timeout=60)
        lines = open(output).read().splitlines()
        return result, dict(line.split("=", 1) for line in lines if "=" in line)

    # Publication approval off (the default): the report is posted as an issue.
    reset()
    result, out = run("review-codebase", "direct")
    assert result.returncode == 0, result.stderr
    assert out["review_status"] == "completed" and out["issue_url"].endswith("/issues/1"), out
    issue = state["issues"][0]
    assert issue["title"].startswith("Cururu codebase review: 1 finding(s) at cccccccccccc"), issue
    link = "https://github.example/test/repo/blob/%s/src/app.py#L2" % revision
    assert "[MEDIUM] [`src/app.py:2`](%s)" % link in issue["body"], issue["body"]
    assert "<!-- cururu:forged" not in issue["body"], "model text cannot forge markers"
    assert "cururu:codebase-review:v1 revision=" + revision in issue["body"]
    prompt = json.dumps(state["model_requests"][0])
    assert "Codebase review mode" in prompt and "docs/guide.md" in prompt

    # A path selection sends only the selected files.
    reset()
    result, out = run("review-codebase", "selected", CURURU_CODEBASE_PATHS="docs/**")
    assert result.returncode == 0, result.stderr
    prompt = json.dumps(state["model_requests"])
    assert "docs/guide.md" in prompt and "src/app.py" not in prompt
    assert state["issues"][0]["title"].startswith("Cururu codebase review: 0 finding(s)")

    # With the approval on, nothing is posted until the environment approves
    # exactly this report.
    reset(require_approval=True)
    result, out = run("review-codebase", "candidate")
    assert result.returncode == 0, result.stderr
    assert out["review_status"] == "pending_approval" and not state["issues"], out
    digest = out["candidate_digest"]
    candidate_path = os.path.join(directory, "cururu-candidate.json")
    assert json.load(open(candidate_path))["kind"] == "codebase"
    state["approvals"] = [{"state": "approved", "comment": "cururu-candidate:" + digest,
                           "environments": [{"id": 3, "name": "cururu-publish"}], "user": {"login": "alice"}}]

    state["protected"] = False
    result, _ = run("publish-approved-review", "unprotected", CURURU_CANDIDATE_DIGEST=digest)
    assert result.returncode != 0 and "must require reviewers" in result.stderr and not state["issues"], result.stderr
    state["protected"] = True

    original = open(candidate_path).read()
    tampered = json.loads(original)
    tampered["body"] += "\nInjected."
    open(candidate_path, "w").write(json.dumps(tampered))
    result, _ = run("publish-approved-review", "tampered", CURURU_CANDIDATE_DIGEST=digest)
    assert result.returncode != 0 and "does not match" in result.stderr and not state["issues"], result.stderr
    open(candidate_path, "w").write(original)

    result, out = run("publish-approved-review", "approved", CURURU_CANDIDATE_DIGEST=digest)
    assert result.returncode == 0, result.stderr
    assert out["review_status"] == "completed" and len(state["issues"]) == 1, out

    # A selection over codebase.max_bytes fails before any model call.
    reset(max_bytes=10)
    result, out = run("review-codebase", "too-large")
    assert result.returncode != 0 and "codebase.max_bytes" in result.stderr, result.stderr
    assert not state["model_requests"] and not state["issues"]
server.shutdown()
