"""Run the synthetic review-quality scenarios through `cururu api-review --pr`.

Every scenario is served by a loopback GitHub fake; nothing is published.

  --check       Deterministic. A loopback fake model answers, and the run only
                proves that each scenario builds and that its diff, context and
                prior feedback reach the model, and that Cururu reports skipped
                or partial files. It says nothing about review quality.
  --send OUT    Sends each scenario to the provider configured through the
                usual environment (CURURU_PROVIDER, LLM_BASE_URL, LLM_API_KEY,
                LLM_MODEL, ...) and writes OUT/<scenario>.json with the receipt
                and the expectations, for a human or model judge. Costs money.
"""
import argparse
import difflib
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
HEAD = "a" * 40
BASE = "b" * 40
LOGIN = "cururu-fixture[bot]"
LONG_LINE = '{"items": ["' + "x" * 3000 + '"]}'


def expand(text):
    return text.replace("[GENERATED_LONG_LINE]", LONG_LINE)


def file_diff(path, before, after):
    """A git-style diff for one text file, derived from its base and head."""
    old = before.splitlines(keepends=True) if before is not None else []
    new = after.splitlines(keepends=True)
    if old == new:
        return ""
    header = f"diff --git a/{path} b/{path}\n"
    if before is None:
        header += "new file mode 100644\n"
    body = difflib.unified_diff(old, new, "/dev/null" if before is None else f"a/{path}", f"b/{path}")
    return header + "".join(body)


def scenario_diff(scenario):
    base, head = scenario["base"], scenario["head"]
    parts = [file_diff(path, base.get(path), expand(content)) for path, content in sorted(head.items())]
    for path in scenario.get("binary", []):
        parts.append(f"diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n")
    return "".join(part for part in parts if part)


def prior_comments(scenario):
    comments = []
    for index, item in enumerate(scenario.get("prior_feedback", [])):
        root = 100 + 2 * index
        comments.append({"id": root, "in_reply_to_id": None, "path": item["path"], "line": item["line"],
                         "body": "<!-- cururu:finding -->\n\n" + item["finding"], "user": {"login": LOGIN, "type": "Bot"}})
        comments.append({"id": root + 1, "in_reply_to_id": root, "path": item["path"], "line": item["line"],
                         "body": item["reply"], "user": {"login": "author", "type": "User"}})
    return comments


def config_toml(scenario):
    paths = json.dumps(scenario.get("context", []))
    return f"version = 1\n\n[context]\nadditional = {paths}\n"


class Fake(http.server.BaseHTTPRequestHandler):
    scenario = None
    requests = []

    def log_message(self, *args):
        pass

    def reply(self, status, value):
        body = value.encode() if isinstance(value, str) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        scenario = Fake.scenario
        path = urllib.parse.urlsplit(self.path).path
        prefix = "/repos/test/repo"
        if path == prefix + "/pulls/1":
            if self.headers.get("Accept") == "application/vnd.github.diff":
                self.reply(200, scenario_diff(scenario))
            else:
                self.reply(200, {"head": {"sha": HEAD, "ref": "change"}, "base": {"sha": BASE, "ref": "main"}})
        elif path.startswith(prefix + "/git/ref/heads/"):
            self.reply(200, {"object": {"sha": HEAD}})
        elif path == prefix + "/contents/.cururu.toml":
            self.reply(200, config_toml(scenario))
        elif path.startswith(prefix + "/contents/"):
            name = urllib.parse.unquote(path[len(prefix + "/contents/"):])
            content = scenario["base"].get(name)
            self.reply(200, content) if content is not None else self.reply(404, {})
        elif path.startswith(prefix + "/git/trees/"):
            tree = [{"path": name, "type": "blob"} for name in sorted(scenario["base"])]
            self.reply(200, {"tree": tree, "truncated": False})
        elif path == "/user":
            self.reply(200, {"login": LOGIN})
        elif path == prefix + "/pulls/1/comments":
            page = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query).get("page", ["1"])[0]
            self.reply(200, prior_comments(scenario) if page == "1" else [])
        elif path == prefix + "/issues/1/comments":
            self.reply(200, [])
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path != "/chat/completions":
            self.reply(404, {})
            return
        Fake.requests.append(body.decode())
        # Report the files of this chunk, so Cururu's coverage check passes.
        chunk = json.loads(body)["messages"][-1]["content"]
        files = {line.split(" b/", 1)[1] for line in chunk.splitlines() if line.startswith("diff --git a/")}
        review = {"model": "fixture", "files_reviewed": len(files), "summary": "fixture", "findings": []}
        self.reply(200, {"choices": [{"message": {"content": json.dumps(review)}, "finish_reason": "stop"}]})


def run_scenario(binary, scenario, model_env, workdir):
    Fake.scenario, Fake.requests = scenario, []
    env = {"PATH": os.environ.get("PATH", ""), "HOME": workdir, "CURURU_REPOSITORY": "test/repo",
           "CURURU_CHANGE_REQUEST_NUMBER": "1", "CURURU_SCM_TOKEN": "fixture-scm",
           "CURURU_SCM_API_URL": model_env["scm_url"], "CURURU_SCM_SERVER_URL": "https://example.invalid"}
    env.update(model_env["provider"])
    if "chunk_bytes" in scenario:
        env["CURURU_CHUNK_BYTES"] = str(scenario["chunk_bytes"])
    result = subprocess.run([binary, "api-review", "--pr", "--confirm-send"], cwd=workdir, env=env,
                            capture_output=True, text=True, timeout=600)
    if result.returncode != 0:
        raise SystemExit(f"{scenario['name']}: api-review failed: {result.stderr.strip()}")
    return json.loads(result.stdout)


def check(scenario, receipt):
    """Fixture validity only: inputs reach the model and coverage is reported."""
    sent = "\n".join(Fake.requests)
    name = scenario["name"]
    assert Fake.requests, f"{name}: no model request"
    for path in scenario["head"]:
        assert path in sent, f"{name}: {path} missing from the model input"
    for path in scenario.get("context", []):
        first_line = scenario["base"][path].splitlines()[1]
        assert json.dumps(first_line)[1:-1] in sent, f"{name}: context {path} missing from the model input"
    for item in scenario.get("prior_feedback", []):
        assert json.dumps(item["reply"])[1:-1] in sent, f"{name}: prior feedback missing from the model input"
    if name == "incomplete-review":
        assert receipt.get("truncated_paths") == ["data/fixtures.json"], receipt
        assert receipt.get("unreviewable_paths") == ["assets/logo.png"], receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("binary")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--send", metavar="OUT")
    parser.add_argument("--only", help="run one scenario by name")
    args = parser.parse_args()

    scenarios = json.load(open(os.path.join(HERE, "scenarios.json")))["scenarios"]
    if args.only:
        scenarios = [s for s in scenarios if s["name"] == args.only]
        assert scenarios, f"unknown scenario {args.only}"
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fake)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = "http://127.0.0.1:%d" % server.server_port
    if args.check:
        provider = {"CURURU_PROVIDER": "openai", "LLM_BASE_URL": url, "LLM_API_KEY": "fixture-api"}
    else:
        keys = [k for k in os.environ if k.startswith(("LLM_", "CURURU_PROVIDER", "OPENROUTER_", "OPENAI_", "ANTHROPIC_"))]
        provider = {k: os.environ[k] for k in keys}
        os.makedirs(args.send, exist_ok=True)
    with tempfile.TemporaryDirectory() as workdir:
        for scenario in scenarios:
            receipt = run_scenario(args.binary, scenario, {"scm_url": url, "provider": provider}, workdir)
            if args.check:
                check(scenario, receipt)
            else:
                record = {"scenario": scenario["name"], "goal": scenario["goal"],
                          "expectations": scenario["expectations"], "diff": scenario_diff(scenario),
                          "receipt": receipt}
                with open(os.path.join(args.send, scenario["name"] + ".json"), "w") as out:
                    json.dump(record, out, indent=2, ensure_ascii=False)
            print(f"ok {scenario['name']}")
    server.shutdown()


if __name__ == "__main__":
    sys.exit(main())
