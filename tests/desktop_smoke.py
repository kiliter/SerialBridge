"""Start the built native desktop process and verify its embedded Core API.

Use xvfb-run on Linux. This checks native startup, not interactive tray behavior
or installation on an end-user machine.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.request import build_opener, ProxyHandler, Request

binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="serialbridge-desktop-") as temp:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    token = os.urandom(32).hex()
    config = Path(temp) / "node.toml"
    config.write_text(f'node_name = "Desktop Smoke"\nhttp_port = {port}\napi_token = "{token}"\n', encoding="utf-8")
    environment = os.environ.copy()
    environment["SERIALBRIDGE_CONFIG"] = str(config)
    log = Path(temp) / "desktop.log"
    with log.open("wb") as output:
        process = subprocess.Popen([str(binary)], env=environment, stdout=output, stderr=output)
        try:
            opener = build_opener(ProxyHandler({}))
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise AssertionError(f"desktop exited {process.returncode}: {log.read_text(errors='replace')}")
                try:
                    request = Request(f"http://127.0.0.1:{port}/v1/health", headers={"Authorization":f"Bearer {token}"})
                    with opener.open(request, timeout=1) as response:
                        status = json.load(response)
                    assert status["node"]["name"] == "Desktop Smoke"
                    assert status["http"]["port"] == port
                    assert status["peer"]["connected"] is False
                    assert status["browser"]["connected"] is False
                    print("PASS native desktop starts its actual embedded Core with truthful offline state")
                    break
                except (URLError, OSError):
                    time.sleep(0.1)
            else:
                raise AssertionError(f"desktop API did not start: {log.read_text(errors='replace')}")
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
