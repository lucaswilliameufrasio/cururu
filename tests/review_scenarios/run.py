#!/usr/bin/env python3
"""Review scenarios for reported failure modes, each run against a real
`cururu` binary with a loopback GitHub fake.

Every scenario states the expected behavior and fails when Cururu does not
show it, so a regression in any of them is caught before release.

Model backends (`--model`):

  fake                  Deterministic (default). A loopback fake answers the
                        model calls with what each scenario needs. Free.
  opencode:<provider/model>
                        Each model call is answered by `opencode run` with that
                        model and no tools, for example
                        `opencode:openai/gpt-6-luna`. It uses whatever login
                        opencode has for the provider (an OpenAI subscription
                        login costs no API credit). Scenarios that measure
                        review quality (repeated resolved findings, findings
                        filling the cap) only mean something with a real model;
                        their numbers are reported, and asserted only as
                        stated in the scenario.

MCP scenarios also run with `--agent opencode:<provider/model>`: opencode
itself is the MCP client and reviews a fixture repository through Cururu's
`prepare_review` / `assemble_review` tools, the way a user would.

Usage:
  python3 tests/review_scenarios/run.py target/debug/cururu
  python3 tests/review_scenarios/run.py target/debug/cururu --only resolved-overflow
  python3 tests/review_scenarios/run.py target/debug/cururu --model opencode:openai/gpt-6-luna \
      --agent opencode:openai/gpt-6-luna --out /tmp/scenarios

Nothing is published anywhere: every GitHub call goes to the loopback fake.
"""
import argparse
import difflib
import http.server
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import urllib.parse

HEAD = "a" * 40
BASE = "b" * 40
LOGIN = "cururu-fixture[bot]"
SHARED_TOKEN = "fixture-shared-token"


# --------------------------------------------------------------------------
# Model backends
# --------------------------------------------------------------------------

OPENCODE_AGENT = {
    "$schema": "https://opencode.ai/config.json",
    "agent": {"rawmodel": {
        "mode": "primary",
        "prompt": "You are a plain completion endpoint. Follow the attached system and user "
                  "messages exactly and output only the answer they ask for, with no commentary.",
        "permission": {"*": "deny"},
        "tools": {"*": False},
    }},
}


class OpencodeModel:
    """Answer an OpenAI-style chat request with `opencode run` and no tools."""

    def __init__(self, model):
        self.model = model
        self.workdir = tempfile.mkdtemp(prefix="cururu-opencode-model-")
        with open(os.path.join(self.workdir, "opencode.json"), "w") as out:
            json.dump(OPENCODE_AGENT, out)
        self.calls = 0
        self.lock = threading.Lock()

    def complete(self, request):
        with self.lock:
            self.calls += 1
            number = self.calls
        prompt = os.path.join(self.workdir, f"request-{number}.txt")
        with open(prompt, "w") as out:
            for message in request["messages"]:
                content = message["content"]
                if isinstance(content, list):
                    content = "\n".join(part.get("text", "") for part in content)
                out.write(f"=== {message['role']} message ===\n{content}\n\n")
        result = subprocess.run(
            ["opencode", "run", "--standalone", "--archived", "--agent", "rawmodel", "-m", self.model,
             "--format", "json", "-f", prompt,
             "Answer the attached chat request. Output only the JSON object it asks for."],
            # opencode finds its project (and this config) from PWD, not the cwd.
            cwd=self.workdir, env=dict(os.environ, PWD=self.workdir), stdin=subprocess.DEVNULL,
            capture_output=True, text=True, timeout=900)
        text = []
        for line in result.stdout.splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if event.get("type") == "text":
                text.append(event["part"]["text"])
        answer = "".join(text).strip()
        if answer.startswith("```"):
            answer = re.sub(r"^```[a-z]*\n|\n```$", "", answer)
        if not answer:
            raise RuntimeError(f"opencode returned no text: {result.stdout[-1000:]} {result.stderr[-1000:]}")
        return answer


# --------------------------------------------------------------------------
# Loopback GitHub fake
# --------------------------------------------------------------------------

def file_diff(path, before, after):
    old = before.splitlines(keepends=True) if before is not None else []
    new = after.splitlines(keepends=True)
    if old == new:
        return ""
    header = f"diff --git a/{path} b/{path}\n"
    if before is None:
        header += "new file mode 100644\n"
    body = difflib.unified_diff(old, new, "/dev/null" if before is None else f"a/{path}", f"b/{path}")
    return header + "".join(body)


def tree_diff(base, head):
    return "".join(file_diff(path, base.get(path), content) for path, content in sorted(head.items()))


def hunk_until(base, head, path, line):
    """GitHub's diff_hunk for a comment on new-side `line`: the hunk up to it."""
    diff = file_diff(path, base.get(path), head[path])
    out, number = [], None
    for text in diff.splitlines():
        if text.startswith("@@"):
            out, number = [text], int(text.split("+")[1].split(",")[0].split(" ")[0])
            continue
        if number is None:
            continue
        out.append(text)
        if not text.startswith("-"):
            if number == line:
                return "\n".join(out)
            number += 1
    return None


class World:
    """Mutable state behind the fake. Scenarios set fields and hooks."""

    def __init__(self):
        self.base = {}
        self.head = {}
        self.head_sha = HEAD
        self.config = "version = 1\n"
        self.shared = {}
        self.threads = []  # {id, path, line, finding, replies, resolved, outdated}
        self.model = None  # callable(request_json, world) -> review dict or raw string
        self.on_model_call = None
        self.model_requests = []
        self.posts = []
        self.checks = []
        self.lock = threading.Lock()

    def comments(self):
        out = []
        for thread in self.threads:
            in_diff = thread["path"] in self.head and file_diff(
                thread["path"], self.base.get(thread["path"]), self.head[thread["path"]])
            line = thread["line"] if in_diff and not thread.get("outdated") else None
            hunk = hunk_until(self.base, self.head, thread["path"], thread["line"]) if in_diff else None
            out.append({"id": thread["id"], "in_reply_to_id": None, "path": thread["path"],
                        "line": line, "original_line": thread["line"], "diff_hunk": hunk,
                        "body": "<!-- cururu:finding -->\n\n" + thread["finding"],
                        "user": {"login": LOGIN, "type": "Bot"}})
            for offset, reply in enumerate(thread["replies"], 1):
                out.append({"id": thread["id"] + offset, "in_reply_to_id": thread["id"],
                            "path": thread["path"], "line": line, "original_line": thread["line"],
                            "body": reply, "user": {"login": "author", "type": "User"}})
        return out

    def review_threads(self):
        nodes = [{"isResolved": bool(t.get("resolved")), "isOutdated": bool(t.get("outdated")),
                  "comments": {"nodes": [{"databaseId": t["id"]}]}} for t in self.threads]
        return {"data": {"repository": {"pullRequest": {
            "author": {"login": "author"},
            "reviewThreads": {"pageInfo": {"hasNextPage": False, "endCursor": None}, "nodes": nodes}}}}}


WORLD = World()


def model_text(request):
    parts = []
    for message in request["messages"]:
        content = message["content"]
        if isinstance(content, list):
            content = "\n".join(part.get("text", "") for part in content)
        parts.append(content)
    return "\n".join(parts)


def chunk_files(request):
    return sorted({line.split(" b/", 1)[1] for line in model_text(request).splitlines()
                   if line.startswith("diff --git a/")})


def default_review(request, world):
    files = chunk_files(request)
    return {"model": "fixture", "files_reviewed": len(files), "summary": "fixture", "findings": []}


class Fake(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, value):
        body = value.encode() if isinstance(value, str) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        world = WORLD
        split = urllib.parse.urlsplit(self.path)
        path, query = split.path, urllib.parse.parse_qs(split.query)
        prefix = "/repos/test/repo"
        if path == prefix + "/pulls/1":
            if self.headers.get("Accept") == "application/vnd.github.diff":
                self.reply(200, tree_diff(world.base, world.head))
            else:
                self.reply(200, {"head": {"sha": world.head_sha, "ref": "change"},
                                 "base": {"sha": BASE, "ref": "main"}, "user": {"login": "author", "type": "User"}})
        elif path.startswith(prefix + "/git/ref/heads/") or path.startswith("/repos/test/shared/git/ref/"):
            self.reply(200, {"object": {"sha": BASE if "shared" in path else world.head_sha, "type": "commit"}})
        elif path.startswith("/repos/test/shared/commits/"):
            self.reply(200, {"sha": BASE})
        elif path == prefix + "/contents/.cururu.toml":
            self.reply(200, world.config)
        elif path.startswith("/repos/test/shared/contents/"):
            if self.headers.get("Authorization") != "Bearer " + SHARED_TOKEN:
                self.reply(401, {"message": "Bad credentials"})
                return
            name = urllib.parse.unquote(path[len("/repos/test/shared/contents/"):])
            content = world.shared.get(name)
            self.reply(200, content) if content is not None else self.reply(404, {})
        elif path.startswith(prefix + "/contents/"):
            name = urllib.parse.unquote(path[len(prefix + "/contents/"):])
            content = world.base.get(name)
            self.reply(200, content) if content is not None else self.reply(404, {})
        elif path.startswith(prefix + "/git/trees/"):
            self.reply(200, {"tree": [{"path": n, "type": "blob"} for n in sorted(world.base)], "truncated": False})
        elif path == "/user":
            self.reply(200, {"login": LOGIN})
        elif path == prefix + "/pulls/1/comments":
            page = int(query.get("page", ["1"])[0])
            per_page = int(query.get("per_page", ["100"])[0])
            comments = world.comments()
            self.reply(200, comments[(page - 1) * per_page: page * per_page])
        elif path == prefix + "/issues/1/comments" or path == prefix + "/pulls/1/reviews":
            self.reply(200, [])
        elif "/check-runs" in path or "/check-suites" in path:
            self.reply(200, {"total_count": 0, "check_runs": []})
        elif path == prefix + "/pulls/1/files":
            self.reply(200, [])
        else:
            self.reply(404, {"message": "not found in fake: " + path})

    def do_POST(self):
        world = WORLD
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path == "/graphql":
            self.reply(200, world.review_threads())
            return
        if self.path == "/chat/completions":
            request = json.loads(body)
            with world.lock:
                world.model_requests.append(request)
                hook = world.on_model_call
            if hook:
                hook(world, len(world.model_requests))
            try:
                answer = world.model(request, world)
            except Exception as error:  # surfaced as a provider error
                self.reply(500, {"error": {"message": str(error)}})
                return
            content = answer if isinstance(answer, str) else json.dumps(answer)
            self.reply(200, {"choices": [{"message": {"role": "assistant", "content": content},
                                          "finish_reason": "stop"}],
                             "usage": {"prompt_tokens": len(model_text(request)) // 4,
                                       "completion_tokens": len(content) // 4,
                                       "total_tokens": (len(model_text(request)) + len(content)) // 4}})
            return
        if self.path.endswith("/check-runs"):
            world.checks.append(json.loads(body))
            self.reply(201, {"id": 77})
            return
        world.posts.append((self.path, body.decode()))
        self.reply(201, {"id": 900 + len(world.posts)})

    def do_PATCH(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        WORLD.posts.append((self.path, body.decode()))
        self.reply(200, {})


def start_fake():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fake)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, "http://127.0.0.1:%d" % server.server_port


# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

class Ctx:
    def __init__(self, binary, url, model, agent, out):
        self.binary, self.url, self.model_spec, self.agent, self.out = binary, url, model, agent, out
        self.backend = OpencodeModel(model.split(":", 1)[1]) if model.startswith("opencode:") else None

    @property
    def real(self):
        return self.backend is not None

    def model(self, fake):
        """The model callable: the real backend, or the scenario's fake."""
        if self.backend:
            return lambda request, world: self.backend.complete(request)
        return fake

    def env(self, workdir, **extra):
        env = {"PATH": os.environ.get("PATH", ""), "HOME": workdir, "CURURU_REPOSITORY": "test/repo",
               "CURURU_CHANGE_REQUEST_NUMBER": "1", "CURURU_SCM_TOKEN": "fixture-scm",
               "CURURU_SCM_API_URL": self.url, "CURURU_SCM_SERVER_URL": "https://example.invalid",
               "CURURU_PROVIDER": "openai", "LLM_BASE_URL": self.url, "LLM_API_KEY": "fixture-api",
               "LLM_MODEL": self.model_spec, "RUST_LOG": "info"}
        env.update({k: str(v) for k, v in extra.items()})
        return env

    def run(self, args, workdir, cwd=None, timeout=1800, **extra):
        return subprocess.run([self.binary, *args], cwd=cwd or workdir, env=self.env(workdir, **extra),
                              capture_output=True, text=True, timeout=timeout)

    def record(self, name, data):
        if self.out:
            os.makedirs(self.out, exist_ok=True)
            with open(os.path.join(self.out, name + ".json"), "w") as out:
                json.dump(data, out, indent=2, ensure_ascii=False)


def reset_world():
    global WORLD
    WORLD.__init__()
    WORLD.model = default_review
    return WORLD


def git_repo(root, files, message="fixture"):
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", HOME=root, GIT_AUTHOR_NAME="Fixture",
               GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_NAME="Fixture",
               GIT_COMMITTER_EMAIL="fixture@example.invalid")

    def git(*argv):
        return subprocess.run(["git", *argv], cwd=root, env=env, check=True, capture_output=True, text=True).stdout.strip()

    if not os.path.isdir(os.path.join(root, ".git")):
        os.makedirs(root, exist_ok=True)
        git("init", "-q", "-b", "main")
    write_files(root, files)
    git("add", "-A")
    git("commit", "-qm", message)
    return git


def write_files(root, files):
    for path, content in files.items():
        full = os.path.join(root, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w") as out:
            out.write(content)


class Failed(AssertionError):
    pass


def expect(condition, message):
    if not condition:
        raise Failed(message)


# --------------------------------------------------------------------------
# Scenario 1: resolved threads beyond the prior-feedback budget
# --------------------------------------------------------------------------

# Words that identify a topic in a finding, in the languages Cururu answers in.
TOPIC_WORDS = {
    "timeout": ["timeout", "tempo limite", "indefinid", "bloque", "sem limite de tempo", "time out"],
    "retry": ["retry", "backoff", "retent", "reintent", "tentativa"],
    "sql": ["sql", "injection", "injeção", "inyección", "query", "consulta"],
    "logging": ["log", "token", "sensitive", "sensív", "sensib"],
    "exception": ["exception", "exceção", "excepción", "swallow", "silenci", "ignora", "pass"],
    "global": ["cache", "global", "mutable", "mutáve", "mutable"],
}

TOPICS = [
    ("timeout", "HTTP call without timeout",
     "    return requests.get(url)\n",
     "Not applicable: the shared session in http_client.py mounts an adapter with a 5 s connect "
     "and 30 s read timeout, so every call made through it is bounded. Kept as is on purpose; "
     "adding a per-call timeout would override the tuned values. Resolving."),
    ("retry", "Retry loop without backoff",
     "    for _ in range(3):\n        send(payload)\n",
     "Kept on purpose: send() is idempotent and the broker already applies exponential backoff "
     "between redeliveries; this loop only covers a local socket reset, documented in ADR-12. "
     "Resolving."),
    ("sql", "SQL built with string formatting",
     "    cursor.execute('SELECT * FROM t WHERE kind = %s' % KIND)\n",
     "Not a risk here: KIND is a module constant from an internal enum, never user input; the "
     "query builder is not available in this worker. Decided with the team in the design review. "
     "Resolving."),
    ("logging", "Sensitive value logged",
     "    log.info('token prefix %s', token[:4])\n",
     "Intentional: only a 4-character prefix of an internal rotation id is logged, agreed with "
     "security to trace rotations; it is not a credential. Resolving."),
    ("exception", "Broad exception swallowed",
     "    try:\n        flush()\n    except Exception:\n        pass\n",
     "Kept: flush() runs during shutdown and a failure there must never block the drain; the "
     "metrics counter flush_failures records it. Resolving."),
    ("global", "Mutable module-level cache",
     "    CACHE[key] = value\n",
     "Kept on purpose: the worker is single-threaded (one process per core) and the cache is "
     "rebuilt on start; documented in worker/README. Resolving."),
]


def overflow_world(world, threads=60, files=20, in_diff=8):
    """`threads` resolved Cururu threads spread over `files` files, each with
    a human reply; the current diff touches the first `in_diff` files and
    re-adds the same patterns the decisions covered."""
    for index in range(files):
        path = f"svc/mod_{index:02d}.py"
        world.base[path] = "import requests\n\n" + "".join(
            f"def f{n}(url, payload, key, value, token):\n    pass\n\n" for n in range(len(TOPICS)))
        world.head[path] = world.base[path]
    for index in range(in_diff):
        path = f"svc/mod_{index:02d}.py"
        world.head[path] = "import requests\n\n" + "".join(
            f"def f{n}(url, payload, key, value, token):\n{TOPICS[n][2]}\n" for n in range(len(TOPICS)))
    for index in range(threads):
        file_index, topic = index % files, index % len(TOPICS)
        path = f"svc/mod_{file_index:02d}.py"
        line = 4 + topic * 3
        world.threads.append({"id": 1000 + 10 * index, "path": path, "line": line, "resolved": True,
                              "topic": TOPICS[topic][0],
                              "finding": f"**MEDIUM**: {TOPICS[topic][1]} in f{topic} (thread {index})\n\n"
                                         f"Details of the earlier finding #{index}.",
                              "replies": [TOPICS[topic][3]],
                              "outdated": file_index >= in_diff})


def scenario_resolved_overflow(ctx, workdir):
    """Issue 1. Sixty answered, resolved threads do not fit the prior-feedback
    budget. Expected: every one still reaches the model (in full or as a
    short summary), threads on files in the current diff and newer ones are
    kept in full first, and the log names what was summarized or dropped.
    With a real model, findings that repeat a resolved decision are counted."""
    world = reset_world()
    overflow_world(world)
    in_diff = {p for p in world.head if world.head[p] != world.base[p]}
    world.model = ctx.model(default_review)
    result = ctx.run(["api-review", "--pr", "--confirm-send"], workdir)
    expect(result.returncode == 0, f"api-review failed: {result.stderr[-3000:]}")
    sent = "\n".join(model_text(r) for r in world.model_requests)
    missing = [t["id"] for t in world.threads if f"(thread {(t['id'] - 1000) // 10})" not in sent]
    in_diff_missing = [t["id"] for t in world.threads if t["path"] in in_diff and t["id"] in missing]
    receipt = json.loads(result.stdout)
    findings = receipt["review"]["findings"]
    # Strict: a finding on a file with a resolved thread on the same topic.
    # Same topic elsewhere: the decision's reason applies project-wide, so
    # it is reported but not asserted.
    repeated, same_topic_elsewhere = [], []
    for finding in findings:
        text = (finding["title"] + " " + finding["message"]).lower()
        topics = {t for t, words in TOPIC_WORDS.items() if any(w in text for w in words)}
        item = {"path": finding["path"], "line": finding.get("line"), "title": finding["title"]}
        if any(t["path"] == finding["path"] and t["topic"] in topics for t in world.threads):
            repeated.append(item)
        elif topics:
            same_topic_elsewhere.append(item)
    ctx.record("resolved-overflow", {"missing_thread_ids": missing, "in_diff_missing": in_diff_missing,
                                     "findings": findings, "repeated": repeated,
                                     "same_topic_elsewhere": same_topic_elsewhere,
                                     "log": [l for l in result.stderr.splitlines() if "prior" in l or "resolved" in l]})
    expect(not in_diff_missing, f"{len(in_diff_missing)} resolved thread(s) on files in the diff never reached the model")
    expect(not missing, f"{len(missing)} of {len(world.threads)} resolved thread(s) never reached the model, not even summarized")
    expect("resolved_summarized" in result.stderr, "the log does not report summarized resolved threads")
    if ctx.real:
        expect(not repeated, f"the model repeated {len(repeated)} resolved decision(s): {repeated[:5]}")
    return (f"{len(world.threads)} resolved threads reached the model; findings={len(findings)}, "
            f"repeated on the same file={len(repeated)}, same topic elsewhere={len(same_topic_elsewhere)}")


# --------------------------------------------------------------------------
# Scenario 2: feedback changes while the model runs
# --------------------------------------------------------------------------

def race_world(world):
    world.base = {"app/a.py": "def a():\n    return 1\n", "app/b.py": "def b():\n    return 2\n"}
    world.head = {"app/a.py": "def a():\n    return requests.get(URL)\n",
                  "app/b.py": "def b():\n    return int(input())\n"}
    world.threads = [{"id": 500, "path": "app/a.py", "line": 2, "resolved": False,
                      "finding": "**MEDIUM**: HTTP call without timeout\n\nDetails.",
                      "replies": ["The shared session sets the timeout; resolving."]}]


def race_review(request, world):
    files = chunk_files(request)
    findings = []
    if "app/a.py" in files:
        findings.append({"severity": "medium", "path": "app/a.py", "line": 2, "title": "HTTP call without timeout",
                         "message": "requests.get has no timeout.", "suggestion": "Pass timeout=.", "confidence": 0.8,
                         "suggested_change": None})
    if "app/b.py" in files:
        findings.append({"severity": "high", "path": "app/b.py", "line": 2, "title": "Unvalidated integer input",
                         "message": "int(input()) raises on bad input.", "suggestion": "Validate.", "confidence": 0.9,
                         "suggested_change": None})
    return {"model": "fixture", "files_reviewed": len(files), "summary": "fixture", "findings": findings}


def scenario_feedback_race(ctx, workdir):
    """Issue 2. A person resolves a thread while the model call runs.
    Expected: the paid review is still published when the reviewed diff is
    unchanged, without the finding that falls on the newly resolved thread;
    a change to the reviewed code during the call still fails."""
    world = reset_world()
    race_world(world)
    world.model = race_review

    def resolve(world, calls):
        world.threads[0]["resolved"] = True
    world.on_model_call = resolve
    result = ctx.run(["review"], workdir, GITHUB_OUTPUT=os.path.join(workdir, "out-race"))
    published = "\n".join(body for _, body in world.posts)
    ctx.record("feedback-race", {"returncode": result.returncode, "stderr": result.stderr[-3000:], "posts": world.posts})
    expect(result.returncode == 0, "review failed after the paid model call: "
           + (re.findall(r"Error: .*", result.stderr) or [result.stderr[-500:]])[-1])
    expect("Unvalidated integer input" in published, "the finding on app/b.py was not published")
    expect("HTTP call without timeout" not in published, "a finding on the newly resolved thread was published")

    world = reset_world()
    race_world(world)
    world.model = race_review

    def push(world, calls):
        world.head["app/b.py"] = "def b():\n    return 3\n"
    world.on_model_call = push
    result = ctx.run(["review"], workdir, GITHUB_OUTPUT=os.path.join(workdir, "out-push"))
    expect(result.returncode != 0 and not any("/reviews" in p or "/comments" in p for p, _ in world.posts),
           "a review was published although the reviewed code changed during the model call")
    return "published without the newly resolved finding; code change still fails"


# --------------------------------------------------------------------------
# Scenario 3: local review with a shared configuration base
# --------------------------------------------------------------------------

def shared_local_repo(root):
    git = git_repo(root, {
        ".cururu.toml": "version = 1\n[config]\nbase = 'test/shared'\nbase_ref = '" + BASE + "'\nbase_path = 'base.toml'\n"
                        "[review]\ntone = 'direct'\n",
        "app.py": "def f():\n    return 1\n"})
    write_files(root, {"app.py": "def f():\n    return 2\n"})
    return git


def scenario_local_shared_base(ctx, workdir):
    """Issue 3. `prepare-review --local` in a repository whose .cururu.toml
    extends a remote shared base. Expected: the base is fetched with the same
    token `print-config` accepts and merged with the same rules as hosted
    review (the local file wins over the base)."""
    world = reset_world()
    world.shared = {"base.toml": "version = 1\n[review]\nlanguage = 'es'\ntone = 'didactic'\n"}
    repo = os.path.join(workdir, "shared-local")
    shared_local_repo(repo)
    env = {"CURURU_SHARED_CONFIG_TOKEN": SHARED_TOKEN, "GITHUB_REPOSITORY": "test/repo",
           "GITHUB_API_URL": ctx.url}
    printed = ctx.run(["print-config"], workdir, cwd=repo, **env)
    result = ctx.run(["prepare-review", "--local"], workdir, cwd=repo, **env)
    ctx.record("local-shared-base", {"print_config": printed.stdout[-2000:] + printed.stderr[-2000:],
                                     "stderr": result.stderr[-2000:]})
    expect(result.returncode == 0, "prepare-review --local failed: "
           + (re.findall(r"Error: .*", result.stderr) or [result.stderr[-500:]])[-1])
    prepared = json.loads(result.stdout)
    preferences = prepared["review_preferences"]
    expect(preferences["language"] == "es", f"shared base not applied: {preferences}")
    expect(preferences["tone"] == "direct", f"local file must win over the base: {preferences}")
    return "shared base merged locally"


# --------------------------------------------------------------------------
# Scenarios 4 and 5: MCP
# --------------------------------------------------------------------------

class Mcp:
    def __init__(self, ctx, workdir, cwd, **env):
        self.process = subprocess.Popen([ctx.binary, "mcp"], cwd=cwd, env=ctx.env(workdir, **env),
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True)
        self.next_id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "scenarios", "version": "1"}})
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")

    def request(self, method, params):
        self.next_id += 1
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}) + "\n")
        self.process.stdin.flush()
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise Failed("MCP server exited: " + self.process.stderr.read()[-2000:])
            message = json.loads(line)
            if message.get("id") == self.next_id:
                return message

    def call(self, name, arguments):
        message = self.request("tools/call", {"name": name, "arguments": arguments})
        if "error" in message:
            return True, message["error"]["message"]
        result = message["result"]
        return result.get("isError", False), result["content"][0]["text"]

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=10)


def stacked_repo(root):
    """main <- feature-a <- feature-b; the worktree also has an uncommitted edit."""
    git = git_repo(root, {"base.py": "def base():\n    return 0\n"})
    git("switch", "-qc", "feature-a")
    git_repo(root, {"a.py": "def a():\n    return 1\n"}, "feature a")
    git("switch", "-qc", "feature-b")
    git_repo(root, {"b.py": "def b(x):\n    return x / 0\n"}, "feature b")
    write_files(root, {"base.py": "def base():\n    return 99\n"})
    return git


def scenario_mcp_repository_path(ctx, workdir):
    """Issue 4. The MCP server starts outside the repository. Expected:
    `repository_path`, `base` and `head` select the repository and a commit
    range, and a directory that is not a Git repository gets an explicit
    error."""
    reset_world()
    repo = os.path.join(workdir, "stacked")
    stacked_repo(repo)
    outside = os.path.join(workdir, "not-a-repo")
    os.makedirs(outside, exist_ok=True)
    mcp = Mcp(ctx, workdir, cwd=outside)
    try:
        error, text = mcp.call("prepare_review", {"source": "local"})
        expect(error and "git" in text.lower() and "repository" in text.lower(),
               f"outside a repository the error does not say so: {text!r}")
        error, text = mcp.call("prepare_review", {"source": "local", "repository_path": repo})
        expect(not error, f"repository_path was not accepted: {text[:300]!r}")
        paths = [f["path"] for f in json.loads(text)["files"]]
        expect(paths == ["base.py"], f"local review of the worktree should hold the uncommitted edit only: {paths}")
        error, text = mcp.call("prepare_review", {"source": "local", "repository_path": repo,
                                                  "base": "feature-a", "head": "feature-b"})
        expect(not error, f"base/head range was not accepted: {text[:300]!r}")
        prepared = json.loads(text)
        paths = [f["path"] for f in prepared["files"]]
        expect(paths == ["b.py"], f"the stacked range feature-a..feature-b should hold b.py only: {paths}")
        review = {"model": "fixture", "files_reviewed": 1, "summary": "ok", "findings": []}
        args = {"source": "local", "repository_path": repo, "base": "feature-a", "head": "feature-b",
                "snapshot_sha256": prepared["snapshot_sha256"]}
        error, text = mcp.call("validate_review", dict(args, review=review))
        expect(not error, f"validate_review rejected the range: {text!r}")
        error, text = mcp.call("assemble_review", dict(args, reviews=[{"chunk_index": 0, "review": review}]))
        expect(not error, f"assemble_review rejected the range: {text!r}")
        error, text = mcp.call("prepare_review", {"source": "local", "repository_path": outside})
        expect(error and "not a git repository" in text.lower(), f"a non-repository path is not reported: {text!r}")
    finally:
        mcp.close()
    return "repository_path, base/head and the non-repository error work"


def multi_chunk_repo(root):
    files = {f"m{i}.py": "def f():\n    return 0\n" for i in range(3)}
    git = git_repo(root, files)
    write_files(root, {f"m{i}.py": "def f():\n    return " + ("1 + " * 120) + "1\n" for i in range(3)})
    return git


def scenario_mcp_assemble_errors(ctx, workdir):
    """Issue 5. assemble_review refuses chunk reviews. Expected: the error
    names the chunk index and the reason (missing chunk, invalid field,
    snapshot mismatch), and the accepted schema is documented."""
    reset_world()
    repo = os.path.join(workdir, "chunks")
    multi_chunk_repo(repo)
    mcp = Mcp(ctx, workdir, cwd=repo, CURURU_CHUNK_BYTES=700)
    try:
        # Small chunks so the fixture is split.
        error, text = mcp.call("prepare_review", {"source": "local"})
        expect(not error, f"prepare_review failed: {text[:300]!r}")
        prepared = json.loads(text)
        chunks = prepared["chunks"]
        snapshot = prepared["snapshot_sha256"]

        def chunk_review(index):
            return {"model": "fixture", "files_reviewed": len(chunks[index]["files"]), "summary": "ok", "findings": []}
        good = [{"chunk_index": i, "review": chunk_review(i)} for i in range(len(chunks))]
        error, text = mcp.call("assemble_review", {"source": "local", "snapshot_sha256": snapshot, "reviews": good})
        expect(not error, f"a complete, valid set was refused: {text!r}")
        cases = []
        if len(chunks) > 1:
            cases.append(("missing chunk", good[:-1], snapshot, [str(len(chunks) - 1), "missing"]))
        bad_field = json.loads(json.dumps(good))
        bad_field[0]["review"]["findings"] = [{"severity": "urgent", "path": chunks[0]["files"][0], "line": 2,
                                               "title": "t", "message": "m", "suggestion": "s", "confidence": 0.5}]
        cases.append(("invalid field", bad_field, snapshot, ["0", "severity"]))
        cases.append(("snapshot mismatch", good, "0" * 64, ["snapshot"]))
        problems = []
        for label, reviews, snap, words in cases:
            error, text = mcp.call("assemble_review", {"source": "local", "snapshot_sha256": snap, "reviews": reviews})
            if not error or not all(word in text.lower() for word in words):
                problems.append(f"{label}: {text!r}")
        expect(not problems, "assemble_review errors do not say what failed: " + "; ".join(problems))
    finally:
        mcp.close()
    return f"{len(chunks)} chunk(s); every refusal names the chunk and reason"


def scenario_mcp_agent(ctx, workdir):
    """Issues 4 and 5 through a real MCP client: opencode, started outside the
    repository, reviews a stacked range through Cururu's tools. Expected: it
    completes prepare_review and assemble_review without a refusal."""
    if not ctx.agent:
        return "skipped (no --agent)"
    reset_world()
    repo = os.path.join(workdir, "agent-stacked")
    stacked_repo(repo)
    home = os.path.join(workdir, "agent-home")
    os.makedirs(home, exist_ok=True)
    config = {"$schema": "https://opencode.ai/config.json",
              "mcp": {"cururu": {"type": "local", "command": [os.path.abspath(ctx.binary), "mcp"], "enabled": True}},
              "permission": {"*": "deny", "cururu_*": "allow"}}
    with open(os.path.join(home, "opencode.json"), "w") as out:
        json.dump(config, out)
    prompt = (f"Use only the cururu MCP tools. Review the commits between branch feature-a (base) and "
              f"feature-b (head) of the Git repository at {repo}: call prepare_review with source local, "
              f"repository_path, base and head; review every chunk yourself; then call assemble_review with "
              f"one review per chunk_index and the snapshot_sha256 you received. Finally print the exact "
              f"text assemble_review returned.")
    # --standalone: a private opencode server in `home`, so this config (and
    # the MCP server it starts, outside the repository) is the one used.
    result = subprocess.run(["opencode", "run", "--standalone", "--archived", "--auto", "-m",
                             ctx.agent.split(":", 1)[1], "--format", "json", prompt],
                            cwd=home, env=dict(os.environ, PWD=home), stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=1800)
    events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
    calls = [e["part"] for e in events if e.get("type") == "tool_use"]
    summary = [{"tool": c.get("tool"), "error": c.get("state", {}).get("status") == "error",
                "output": str(c.get("state", {}).get("output", ""))[:400]} for c in calls]
    ctx.record("mcp-agent", {"tool_calls": summary, "stderr": result.stderr[-2000:]})
    assembled = [c for c in summary if c["tool"] and c["tool"].endswith("assemble_review")]
    expect(any(c["tool"] and c["tool"].endswith("prepare_review") and not c["error"]
               and "Unable to prepare" not in c["output"] for c in summary),
           f"opencode could not prepare the review: {summary}")
    expect(assembled and "invalid" not in assembled[-1]["output"].lower()
           and "nothing was published" not in assembled[-1]["output"].lower(),
           f"assemble_review was refused or never called: {assembled}")
    return f"opencode made {len(calls)} tool call(s); assemble_review accepted"


# --------------------------------------------------------------------------
# Scenario 6: cost across pushes
# --------------------------------------------------------------------------

def scenario_push_cost(ctx, workdir):
    """Issue 6. Three pushes on a large PR, the last two adding one small
    file each. Measures the diff bytes sent to the model per push with
    `incremental` off and on. Expected: the README documents the measured
    behavior of `incremental` (what it skips and what it does not)."""
    rows = {}
    for incremental in (False, True):
        world = reset_world()
        world.config = f"version = 1\n[policy]\nincremental = {'true' if incremental else 'false'}\n"
        world.base = {f"big/f{i}.py": "x = 0\n" for i in range(12)}
        world.head = {f"big/f{i}.py": "".join(f"x{n} = {n}\n" for n in range(120)) for i in range(12)}
        world.model = default_review
        sent = []
        for push in range(3):
            if push:
                world.head[f"small/p{push}.py"] = "y = 1\n"
                world.head_sha = f"{push:x}" * 40
            before = len(world.model_requests)
            result = ctx.run(["review"], workdir, GITHUB_OUTPUT=os.path.join(workdir, f"out-cost-{push}"))
            expect(result.returncode == 0, f"push {push} failed: {result.stderr[-800:]}")
            sent.append(sum(len(model_text(r)) for r in world.model_requests[before:]))
        rows["incremental" if incremental else "full"] = sent
    readme = open(os.path.join(os.path.dirname(__file__), "..", "..", "README.md")).read()
    ctx.record("push-cost", rows)
    documented = re.search(r"incremental[^\n]*(push|commit)", readme, re.I)
    expect(documented and "cost" in readme[documented.start(): documented.start() + 1500].lower(),
           f"README does not document incremental's effect on cost; measured model input bytes per push: {rows}")
    return f"model input bytes per push: {rows}"


# --------------------------------------------------------------------------
# Scenario 7: findings fill max_findings
# --------------------------------------------------------------------------

def scenario_findings_cap(ctx, workdir):
    """Issue 7. A large diff with a few real defects and much harmless code.
    Expected: the prompt never asks the model to reach the cap, and when
    candidates exceed max_findings the kept ones are the most severe and,
    within a severity, the most confident (not the first paths). With a real
    model, the number of findings is reported against the cap."""
    world = reset_world()
    world.config = "version = 1\n[policy]\nmax_findings = 5\n"
    world.base = {f"lib/m{i}.py": "def f(v):\n    return v\n" for i in range(8)}
    world.head = {f"lib/m{i}.py": "def f(v):\n    return v + 1\n" for i in range(8)}
    world.head["lib/m7.py"] = "import os\n\ndef f(v):\n    os.system('rm -rf ' + v)\n    return v\n"

    def many(request, world):
        files = chunk_files(request)
        findings = [{"severity": "medium", "path": p, "line": 2, "title": f"Weak guess {p}",
                     "message": "m", "suggestion": "s", "confidence": 0.35, "suggested_change": None}
                    for p in files if p != "lib/m7.py"]
        if "lib/m7.py" in files:
            findings.append({"severity": "medium", "path": "lib/m7.py", "line": 4, "title": "Shell injection",
                             "message": "m", "suggestion": "s", "confidence": 0.95, "suggested_change": None})
        return {"model": "fixture", "files_reviewed": len(files), "summary": "s", "findings": findings}
    world.model = ctx.model(many)
    result = ctx.run(["api-review", "--pr", "--confirm-send"], workdir)
    expect(result.returncode == 0, f"api-review failed: {result.stderr[-1500:]}")
    findings = json.loads(result.stdout)["review"]["findings"]
    prompt = model_text(world.model_requests[0])
    ctx.record("findings-cap", {"findings": findings})
    expect(not re.search(r"report (up to|at most) \d+ findings", prompt, re.I)
           or "not a target" in prompt.lower(),
           "the prompt states a findings count without saying it is not a target")
    if ctx.real:
        return f"{len(findings)} finding(s) with cap 5: {[f['title'] for f in findings]}"
    titles = [f["title"] for f in findings]
    expect("Shell injection" in titles,
           f"the most confident medium finding was cut by the cap; kept: {titles}")
    return "the cap keeps severity, then confidence"


SCENARIOS = {
    "resolved-overflow": scenario_resolved_overflow,
    "feedback-race": scenario_feedback_race,
    "local-shared-base": scenario_local_shared_base,
    "mcp-repository-path": scenario_mcp_repository_path,
    "mcp-assemble-errors": scenario_mcp_assemble_errors,
    "mcp-agent": scenario_mcp_agent,
    "push-cost": scenario_push_cost,
    "findings-cap": scenario_findings_cap,
}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("binary")
    parser.add_argument("--model", default="fake", help="fake (default) or opencode:<provider/model>")
    parser.add_argument("--agent", help="opencode:<provider/model> for the MCP agent scenario")
    parser.add_argument("--only", action="append", help="run only these scenarios")
    parser.add_argument("--out", help="write per-scenario evidence JSON here")
    args = parser.parse_args()
    if (args.model.startswith("opencode:") or args.agent) and not shutil.which("opencode"):
        parser.error("opencode is not on PATH")
    server, url = start_fake()
    ctx = Ctx(os.path.abspath(args.binary), url, args.model, args.agent, args.out)
    failures = 0
    for name, scenario in SCENARIOS.items():
        if args.only and name not in args.only:
            continue
        started = time.monotonic()
        with tempfile.TemporaryDirectory(prefix=f"cururu-{name}-") as workdir:
            try:
                detail = scenario(ctx, workdir)
                print(f"ok   {name} ({time.monotonic() - started:.0f}s): {detail}", flush=True)
            except Failed as error:
                failures += 1
                print(f"FAIL {name}: {error}", flush=True)
            except Exception:
                failures += 1
                print(f"ERROR {name}:\n{traceback.format_exc()}", flush=True)
    server.shutdown()
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
