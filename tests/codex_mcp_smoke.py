#!/usr/bin/env python3
"""Opt-in Linux smoke: Codex app-server calls Cururu MCP without a model turn."""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--codex", required=True, type=Path)
    parser.add_argument("--cururu", required=True, type=Path)
    args = parser.parse_args()
    codex = args.codex.resolve(strict=True)
    cururu = args.cururu.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="cururu-codex-") as tmp:
        root = Path(tmp)
        home, repo = root / "home", root / "repo"
        home.mkdir()
        repo.mkdir()
        (home / ".codex").mkdir()
        (home / ".codex" / "config.toml").write_text(
            '[mcp_servers.cururu]\ncommand = "/cururu"\nargs = ["mcp"]\n', encoding="utf-8"
        )
        git_env = {"PATH": "/usr/bin", "HOME": str(home), "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                   "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"}
        def git(*argv):
            subprocess.run(["/usr/bin/git", *argv], cwd=repo, env=git_env, check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
        git("init", "-q")
        file = repo / "code.txt"
        file.write_text("before\n", encoding="utf-8")
        git("add", "code.txt")
        git("commit", "-qm", "fixture")
        file.write_text("after\n", encoding="utf-8")
        command = ["bwrap", "--unshare-all", "--die-with-parent", "--ro-bind", "/usr", "/usr",
                   "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib64", "/lib64",
                   "--symlink", "usr/bin", "/bin", "--proc", "/proc", "--dev", "/dev",
                   "--ro-bind", str(codex), "/codex", "--ro-bind", str(cururu), "/cururu",
                   "--bind", str(home), "/home", "--bind", str(repo), "/project", "--tmpfs", "/tmp",
                   "--chdir", "/project", "--clearenv", "--setenv", "HOME", "/home",
                   "--setenv", "CODEX_HOME", "/home/.codex", "--setenv", "PATH", "/usr/bin",
                   "--", "/codex", "app-server"]
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, bufsize=0)
        buffer = bytearray()
        def send(message):
            process.stdin.write((json.dumps(message) + "\n").encode())
        def response(request_id):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if b"\n" in buffer:
                    line, _, rest = buffer.partition(b"\n")
                    buffer[:] = rest
                    message = json.loads(line)
                    if message.get("id") == request_id:
                        if "error" in message:
                            raise RuntimeError("app-server returned an error")
                        return message["result"]
                    continue
                ready, _, _ = select.select([process.stdout], [], [], 1)
                if ready:
                    chunk = os.read(process.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    buffer.extend(chunk)
                    if len(buffer) > 4 * 1024 * 1024:
                        raise RuntimeError("app-server response exceeds limit")
            raise RuntimeError("app-server response timed out or ended")
        try:
            send({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "cururu_fixture", "version": "0.1.0"}}})
            response(1)
            send({"method": "initialized", "params": {}})
            send({"id": 2, "method": "thread/start", "params": {"cwd": "/project", "ephemeral": True}})
            thread = response(2)["thread"]["id"]
            def call(request_id, tool, arguments):
                send({"id": request_id, "method": "mcpServer/tool/call", "params": {
                    "threadId": thread, "server": "cururu", "tool": tool, "arguments": arguments}})
                return response(request_id)
            prepared = call(3, "prepare_review", {"source": "local"})
            assert prepared["isError"] is False
            input_data = json.loads(prepared["content"][0]["text"])
            assert input_data["files"][0]["path"] == "code.txt"
            assert "+after" in input_data["files"][0]["patch"]
            review = {"model": "fixture", "files_reviewed": 1, "summary": "synthetic", "findings": []}
            validation = {"source": "local", "snapshot_sha256": input_data["snapshot_sha256"], "review": review}
            assembly = {"source": "local", "snapshot_sha256": input_data["snapshot_sha256"],
                        "reviews": [{"chunk_index": 0, "review": review}]}
            assert call(4, "validate_review", validation)["isError"] is False
            assert call(5, "assemble_review", assembly)["isError"] is False
            file.write_text("changed after preparation\n", encoding="utf-8")
            assert call(6, "validate_review", validation)["isError"] is True
            assert call(7, "assemble_review", assembly)["isError"] is True
            print("PASS: prepare, validate, assemble; both reject changed snapshot")
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    main()
