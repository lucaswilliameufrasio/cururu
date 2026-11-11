"""Signed webhook, real local SQLite queue, and loopback-only fake services."""
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

requests = []

class Fake(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        requests.append(self.path)
        self.send_response(500)
        self.end_headers()

    do_GET = do_POST

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
           "CURURU_EVALUATOR_MODE": "filter", "CURURU_EVALUATOR_ALLOWED_REPOSITORIES": "test/repo"}
    with tempfile.TemporaryFile(mode="w+") as log:
        process = subprocess.Popen([sys.argv[1], "serve"], cwd=directory, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 20
            while True:
                try:
                    urllib.request.urlopen(url + "/health", timeout=1).close()
                    break
                except OSError:
                    assert process.poll() is None and time.monotonic() < deadline
                    time.sleep(0.05)
            body = json.dumps({"action": "opened", "installation": {"id": 1}, "repository": {"full_name": "test/repo"}, "pull_request": {"number": 1, "draft": False}}).encode()
            signature = hmac.new(b"fixture-secret", body, hashlib.sha256).hexdigest()
            request = urllib.request.Request(url + "/v1/webhooks/github", data=body, headers={"X-Hub-Signature-256": "sha256=" + signature, "X-GitHub-Delivery": "fixture-delivery", "X-GitHub-Event": "pull_request"})
            assert urllib.request.urlopen(request, timeout=5).status == 202
            deadline = time.monotonic() + 20
            while True:
                with sqlite3.connect(database) as db:
                    row = db.execute("SELECT state, attempts FROM github_deliveries WHERE delivery_id = ?", ("fixture-delivery",)).fetchone()
                if row and row[0] == "queued" and row[1] >= 1:
                    break
                assert process.poll() is None and time.monotonic() < deadline, row
                time.sleep(0.05)
            assert not requests, requests
            log.flush()
            log.seek(0)
            assert "evaluator filtering is disabled" in log.read()
        finally:
            process.terminate()
            process.wait(timeout=10)
fake.shutdown()
