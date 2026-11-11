"""Exercise the publication CLI against a loopback-only synthetic GitHub API."""
import http.server
import json
import os
import pty
import select
import subprocess
import sys
import tempfile
import threading
import time

posts = []
head = "a" * 40
diff = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n"
suggestion_policy = None

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def send_data(self, status, data):
        raw = data.encode() if isinstance(data, str) else json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == "/repos/test/repo/pulls/1":
            if self.headers.get("Accept") == "application/vnd.github.diff":
                self.send_data(200, diff)
            else:
                self.send_data(200, {"head": {"sha": head, "ref": "branch"}, "base": {"sha": head, "ref": "main"}})
        elif self.path.startswith("/repos/test/repo/git/ref/heads/"):
            self.send_data(200, {"object": {"sha": head}})
        elif self.path.startswith("/repos/test/repo/contents/.cururu.toml?") and suggestion_policy is not None:
            self.send_data(200, "version = 1\n[policy]\nsuggested_changes = " + ("true" if suggestion_policy else "false") + "\n")
        elif "/contents/" in self.path:
            self.send_data(404, {})
        elif self.path == "/user":
            self.send_data(200, {"login": "fixture"})
        elif "/comments?" in self.path:
            self.send_data(200, [])
        else:
            self.send_data(404, {})

    def do_POST(self):
        posts.append((self.path, self.rfile.read(int(self.headers["Content-Length"]))))
        self.send_data(201, {})

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
binary = sys.argv[1]
with tempfile.TemporaryDirectory() as directory:
    env = {"PATH": os.environ.get("PATH", ""), "HOME": directory,
           "CURURU_REPOSITORY": "test/repo", "CURURU_CHANGE_REQUEST_NUMBER": "1",
           "CURURU_SCM_TOKEN": "synthetic", "CURURU_SCM_API_URL": "http://127.0.0.1:%d" % server.server_port,
           "CURURU_SCM_SERVER_URL": "https://example.invalid"}
    with open(os.path.join(directory, "analysis.sarif"), "w") as file:
        json.dump({"runs": [{"tool": {"driver": {"name": "fixture"}}, "results": [{"ruleId": "RULE", "level": "error", "message": {"text": "off-diff analyzer finding"}, "locations": [{"physicalLocation": {"artifactLocation": {"uri": "a"}, "region": {"startLine": 9}}}]}]}]}, file)
    with open(os.path.join(directory, "manifest.json"), "w") as file:
        json.dump({"schema_version": 1, "commit_sha": head, "tools": []}, file)
    options = ["--sarif", "analysis.sarif", "--analysis-manifest", "manifest.json"]
    prepared = subprocess.run([binary, "prepare-review", "--pr", *options], cwd=directory, env=env, capture_output=True, text=True, timeout=20)
    assert prepared.returncode == 0, prepared.stderr
    data = json.loads(prepared.stdout)
    receipt = {key: data[key] for key in ("source", "revision", "diff_sha256", "snapshot_sha256", "scope", "ignored_paths")}
    receipt["review"] = {"model": "fixture", "files_reviewed": len(data["files"]), "summary": "", "findings": data["analyzer_evidence"]}
    receipt["usage"] = {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13, "cached_tokens": 0, "reasoning_tokens": 0}
    with open(os.path.join(directory, "receipt.json"), "w") as file:
        json.dump(receipt, file)

    def publish(answer, receipt_name="receipt.json", publish_options=options, change_before_answer=False, preview_text=None, policy_after_preview=None):
        master, slave = pty.openpty()
        process = subprocess.Popen([binary, "publish-agent-review", receipt_name, *publish_options], cwd=directory, env=env, stdin=slave, stderr=slave, stdout=subprocess.PIPE)
        os.close(slave)
        transcript = b""
        deadline = time.monotonic() + 20
        try:
            while b"Type PUBLISH" not in transcript and time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 1)
                if ready:
                    try:
                        transcript += os.read(master, 65536)
                    except OSError:
                        break
            assert b"Type PUBLISH" in transcript, transcript.decode(errors="replace")
            if preview_text is not None:
                assert (b"&lt;/pre&gt;&#64;everyone &amp; fix()" in transcript) == preview_text
            if change_before_answer:
                globals()["diff"] = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+changed\n"
            if policy_after_preview is not None:
                globals()["suggestion_policy"] = policy_after_preview
            os.write(master, answer + b"\n")
            process.communicate(timeout=20)
            return process.returncode
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)

    assert publish(b"cancel") != 0
    assert not posts
    assert publish(b"PUBLISH") == 0
    assert len(posts) == 1 and posts[0][0] == "/repos/test/repo/issues/1/comments"
    assert b"off-diff analyzer finding" in posts[0][1]
    posts.clear()
    mcp = subprocess.Popen([binary, "mcp"], cwd=directory, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    def call(identifier, name, arguments):
        mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "id": identifier, "method": "tools/call", "params": {"name": name, "arguments": arguments}}) + "\n")
        mcp.stdin.flush()
        response = json.loads(mcp.stdout.readline())
        assert response["id"] == identifier and response["result"]["isError"] is False, response
        return json.loads(response["result"]["content"][0]["text"])
    try:
        data = call(1, "prepare_review", {"source": "pr"})
        results = [{"chunk_index": chunk["index"], "review": {"model": "fixture", "files_reviewed": len(chunk["files"]), "summary": "synthetic", "findings": []}} for chunk in data["chunks"]]
        assembled = call(2, "assemble_review", {"source": "pr", "snapshot_sha256": data["snapshot_sha256"], "reviews": results})
    finally:
        mcp.stdin.close()
        assert mcp.wait(timeout=20) == 0, mcp.stderr.read()
    receipt = {key: data[key] for key in ("source", "revision", "diff_sha256", "snapshot_sha256", "scope", "ignored_paths")}
    receipt["review"] = assembled
    with open(os.path.join(directory, "mcp-receipt.json"), "w") as file:
        json.dump(receipt, file)
    assert publish(b"cancel", "mcp-receipt.json", []) != 0
    assert not posts
    assert publish(b"PUBLISH", "mcp-receipt.json", [], change_before_answer=True) != 0
    assert not posts
    diff = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n"
    assert publish(b"PUBLISH", "mcp-receipt.json", []) == 0
    assert len(posts) == 1 and posts[0][0] == "/repos/test/repo/issues/1/comments"
    for enabled in (False, True):
        suggestion_policy = enabled
        posts.clear()
        prepared = subprocess.run([binary, "prepare-review", "--pr"], cwd=directory, env=env, capture_output=True, text=True, timeout=20)
        assert prepared.returncode == 0, prepared.stderr
        data = json.loads(prepared.stdout)
        receipt = {key: data[key] for key in ("source", "revision", "diff_sha256", "snapshot_sha256", "scope", "ignored_paths")}
        receipt["review"] = {"model": "fixture", "files_reviewed": 1, "summary": "", "findings": [{"severity": "high", "path": "a", "line": 1, "title": "fixture", "message": "issue", "suggestion": "guidance", "confidence": 0.99, "suggested_change": {"replacement": "</pre>@everyone & fix()"}}]}
        with open(os.path.join(directory, "suggestion-receipt.json"), "w") as file:
            json.dump(receipt, file)
        assert publish(b"cancel", "suggestion-receipt.json", [], preview_text=enabled) != 0
        assert not posts
        if enabled:
            assert publish(b"PUBLISH", "suggestion-receipt.json", [], preview_text=True, policy_after_preview=False) != 0
            assert not posts
            suggestion_policy = True
        assert publish(b"PUBLISH", "suggestion-receipt.json", [], preview_text=enabled) == 0
        body = json.loads(posts[0][1])["body"]
        assert ("&lt;/pre&gt;&#64;everyone &amp; fix()" in body) == enabled
        assert "</pre>@everyone" not in body
server.shutdown()
