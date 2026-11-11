"""Exercise CLI approval of hosted candidates against a loopback-only synthetic GitHub API."""
import hashlib
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

head = "a" * 40
preview = "<!-- cururu:candidate:v1 -->\nCandidate for `test/repo` #1\n\n## Summary comment\n\nfixture\n"
digest = hashlib.sha256(preview.encode()).hexdigest()
state = {"checks": [], "posts": [], "can_approve": True}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, value):
        raw = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == "/repos/test/repo/pulls/1":
            self.reply(200, {"head": {"sha": head, "ref": "branch"}, "base": {"sha": head, "ref": "main"}})
        elif self.path.startswith(f"/repos/test/repo/commits/{head}/check-runs?"):
            assert "check_name=Cururu%20review" in self.path, self.path
            self.reply(200, {"check_runs": state["checks"]})
        elif self.path == "/repos/test/repo/actions/runs/5/pending_deployments":
            self.reply(200, [{"environment": {"id": 3, "name": "cururu-publish"}, "current_user_can_approve": state["can_approve"]}])
        else:
            self.reply(404, {})

    def do_POST(self):
        state["posts"].append((self.path, json.loads(self.rfile.read(int(self.headers["Content-Length"])))))
        self.reply(201, {})


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
binary = sys.argv[1]
with tempfile.TemporaryDirectory() as directory:
    env = {"PATH": os.environ.get("PATH", ""), "HOME": directory,
           "CURURU_REPOSITORY": "test/repo", "CURURU_SCM_TOKEN": "synthetic-user-token",
           "CURURU_SCM_API_URL": "http://127.0.0.1:%d" % server.server_port,
           "CURURU_SCM_SERVER_URL": "https://example.invalid"}

    def approve(answer):
        master, slave = pty.openpty()
        process = subprocess.Popen([binary, "approve-hosted-review", "--pr", "1"], cwd=directory, env=env, stdin=slave, stderr=slave, stdout=subprocess.PIPE)
        os.close(slave)
        transcript = b""
        deadline = time.monotonic() + 20
        try:
            while b"Type APPROVE" not in transcript and process.poll() is None:
                assert time.monotonic() < deadline, transcript
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    try:
                        transcript += os.read(master, 4096)
                    except OSError:
                        break
            if b"Type APPROVE" in transcript:
                os.write(master, answer.encode() + b"\n")
            process.wait(timeout=20)
            while True:
                ready, _, _ = select.select([master], [], [], 0.1)
                if not ready:
                    break
                try:
                    chunk = os.read(master, 4096)
                except OSError:
                    break
                if not chunk:
                    break
                transcript += chunk
        finally:
            os.close(master)
        return process.returncode, transcript.decode(errors="replace")

    # No candidate, or only a check whose preview does not match its digest.
    state["checks"] = [{"id": 1, "external_id": "0" * 64, "output": {"text": preview}}]
    code, transcript = approve("APPROVE")
    assert code != 0 and "no inspectable Cururu candidate" in transcript, transcript
    assert not state["posts"]

    # App candidate: the preview is shown and approval is a comment naming the digest.
    state["checks"] = [{"id": 2, "external_id": digest, "output": {"text": preview}}]
    code, transcript = approve("no")
    assert code != 0 and "approval cancelled" in transcript and "fixture" in transcript, transcript
    assert not state["posts"]
    code, transcript = approve("APPROVE")
    assert code == 0, transcript
    assert state["posts"] == [("/repos/test/repo/issues/1/comments", {"body": "/cururu approve " + digest})], state["posts"]

    # Action candidate: approval targets the run's environment and names the digest.
    state.update(posts=[], checks=[{"id": 3, "external_id": digest + ":run=5", "output": {"text": preview}}], can_approve=False)
    code, transcript = approve("APPROVE")
    assert code != 0 and "cannot approve" in transcript, transcript
    assert not state["posts"]
    state["can_approve"] = True
    code, transcript = approve("APPROVE")
    assert code == 0, transcript
    assert state["posts"] == [("/repos/test/repo/actions/runs/5/pending_deployments", {"environment_ids": [3], "state": "approved", "comment": "cururu-candidate:" + digest})], state["posts"]

    piped = subprocess.run([binary, "approve-hosted-review", "--pr", "1"], cwd=directory, env=env, input="APPROVE\n", capture_output=True, text=True, timeout=20)
    assert piped.returncode != 0 and "interactive terminal" in piped.stderr, piped.stderr
server.shutdown()
