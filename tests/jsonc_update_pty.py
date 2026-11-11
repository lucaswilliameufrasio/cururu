"""Exercise interactive JSONC update only against disposable fixture files."""
import os
from pathlib import Path
import pty
import select
import subprocess
import sys
import tempfile
import time

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    config = root / "mcp.json"
    backup = root / "backup.json"
    original = '{\n // fixture comment\n "mcpServers": {"other": {}},\n}'
    config.write_text(original, encoding="utf-8")

    def run(answer):
        master, slave = pty.openpty()
        process = subprocess.Popen([sys.argv[1], "update-jsonc-mcp", "cursor", str(config),
                                    "--backup", str(backup), "--executable", "/fixture/cururu"],
                                   cwd=root, env={}, stdin=slave, stderr=slave, stdout=subprocess.PIPE)
        os.close(slave)
        transcript = b""
        try:
            deadline = time.monotonic() + 15
            while b"Type UPDATE" not in transcript and time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 1)
                if ready:
                    try:
                        transcript += os.read(master, 65536)
                    except OSError:
                        break
            assert b'Type UPDATE' in transcript, transcript.decode(errors="replace")
            assert b'/fixture/cururu' in transcript
            os.write(master, answer + b"\n")
            process.communicate(timeout=15)
            return process.returncode
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)

    assert run(b"cancel") != 0
    assert config.read_text(encoding="utf-8") == original
    assert not backup.exists()
    assert run(b"UPDATE") == 0
    assert backup.read_text(encoding="utf-8") == original
    assert backup.stat().st_mode & 0o777 == 0o600
    updated = config.read_text(encoding="utf-8")
    assert "// fixture comment" in updated and '"other"' in updated and '"cururu"' in updated
