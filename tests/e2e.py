"""Real headless nodes and HTTP/TCP flows; optional live UI and Chromium extension.

No API interception or simulated Core. Physical serial hardware and remote sites
are outside this suite. Use --browser for UI and --extension on a machine that
allows unpacked extensions (CI runs both).
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, SimpleHTTPRequestHandler, ThreadingHTTPServer
from functools import partial
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parents[1]
DATA = bytes(i % 251 for i in range(140000))
SHA = hashlib.sha256(DATA).hexdigest()
OPENER = build_opener(ProxyHandler({}))  # This fixture only talks to loopback.
RESULTS = []


def passed(name):
    RESULTS.append(name)
    print("PASS", name, flush=True)


def wait_for(fn, timeout=15):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        try:
            value = fn()
            if value:
                return value
        except (URLError, OSError):
            pass
        time.sleep(0.05)
    raise AssertionError(f"condition did not become true within {timeout}s")


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class FixtureHandler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/api")
            self.end_headers()
            return
        data = b"fixture-api" if self.path == "/api" else DATA
        if self.path == "/inline-large":
            data = b"x" * 20000
        if self.path == "/slow":
            data = b"s" * 1000000
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        try:
            if self.path == "/slow":
                for offset in range(0, len(data), 4096):
                    self.wfile.write(data[offset:offset + 4096])
                    self.wfile.flush()
                    time.sleep(0.05)
            else:
                self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass


class QuietUI(SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass


class NodeProcess:
    def __init__(self, root, name, origin, link_port, listen):
        self.port = free_port()
        self.token = os.urandom(32).hex()
        self.root = root / name
        self.root.mkdir()
        self.config = self.root / "node.toml"
        self.config.write_text(
            f'node_name = "{name}"\nhttp_port = {self.port}\n'
            f'api_token = "{self.token}"\nallowed_origins = [{json.dumps(origin)}]\n'
            'download_dir = "downloads"\nshared_dir = "shared"\n', encoding="utf-8")
        executable = os.environ.get("SERIALBRIDGE_BIN", str(ROOT / "target/debug" / ("serialbridge.exe" if os.name == "nt" else "serialbridge")))
        flag = "--dev-tcp-listen" if listen else "--dev-tcp-connect"
        self.command = [executable, "--config", str(self.config), "serve", flag, f"127.0.0.1:{link_port}"]
        self.log = (self.root / "process.log").open("wb")
        self.process = None
        self.start()

    def start(self):
        self.process = subprocess.Popen(self.command, stdout=self.log, stderr=self.log)
        wait_for(lambda: self.api("GET", "/v1/health"))

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()

    def api(self, method, path, data=None, *, raw=False, authorized=True, expected=200):
        headers = {"Authorization": f"Bearer {self.token}"} if authorized else {}
        if data is not None and not isinstance(data, bytes):
            data = json.dumps(data).encode()
            headers["Content-Type"] = "application/json"
        request = Request(f"http://127.0.0.1:{self.port}{path}", data=data, headers=headers, method=method)
        try:
            response = OPENER.open(request, timeout=10)
        except HTTPError as error:
            response = error
        with response:
            body = response.read()
            assert response.status == expected, (method, path, response.status, body[:300])
            return body if raw else json.loads(body)

    def job(self, tid):
        def done():
            value = self.api("GET", f"/v1/downloads/{tid}")
            return value if value["status"] in ("completed", "failed", "cancelled") else None
        return wait_for(done)

    def assert_file(self, job, expected=DATA):
        completed = self.job(job["id"])
        assert completed["status"] == "completed", completed
        assert completed["sha256"] == hashlib.sha256(expected).hexdigest()
        assert self.api("GET", f'/v1/downloads/{job["id"]}/content', raw=True) == expected
        return completed


def core_flows(a, b, origin):
    wait_for(lambda: a.api("GET", "/v1/health")["peer"]["connected"])
    wait_for(lambda: b.api("GET", "/v1/health")["peer"]["connected"])
    passed("two independent headless processes handshake over development TCP")
    assert a.api("GET", "/v1/health", authorized=False, expected=401)["error"]["code"] == "UNAUTHORIZED"
    passed("actual HTTP API rejects missing bearer token")
    settings = a.api("GET", "/v1/settings")
    assert "api_token" not in settings and "extension_token" not in settings
    passed("public settings omit private credentials")

    def ping(node):
        return node.api("POST", "/v1/rpc", {"target":"peer","method":"system.ping","params":{}})
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        responses = list(pool.map(ping, [a, b] * 10))
    assert all(r["result"]["protocol"] == 1 for r in responses)
    passed("twenty concurrent bidirectional RPC calls complete on one link")
    result = a.api("POST", "/v1/rpc", {"method":"http.fetch","params":{"url":origin + "/api"}})
    assert result["result"]["body"] == "fixture-api"
    passed("peer executes real HTTP fetch against a local fixture")
    for url, expected in [("https://disallowed.invalid/", "ORIGIN_NOT_ALLOWED"), (origin + "/redirect", "REDIRECT_NOT_ALLOWED"), (origin + "/inline-large", "RESPONSE_TOO_LARGE")]:
        error = a.api("POST", "/v1/rpc", {"method":"http.fetch","params":{"url":url}}, expected=403 if expected == "ORIGIN_NOT_ALLOWED" else 400)
        assert error["error"]["code"] == expected
        passed("real HTTP executor rejects " + expected)

    for target in ("local", "peer"):
        job = a.api("POST", "/v1/downloads", {"target":target,"executor":"http","url":origin + "/file","filename":target + ".bin"}, expected=202)
        a.assert_file(job)
        passed(target + " HTTP download returns byte-exact verified content")
    for payload, name in [(DATA,"push.bin"),(b"","empty.bin")]:
        job = a.api("PUT", "/v1/files/push/" + name, payload, expected=202)
        assert a.job(job["id"])["status"] == "completed"
        received = next(t for t in b.api("GET", "/v1/downloads")["transfers"] if t["filename"] == name and t["direction"] == "receive")
        b.assert_file(received, payload)
        passed("peer push verifies " + name)

    shared = b.root / "shared" / "shared.bin"
    shared.write_bytes(DATA)
    job = a.api("POST", "/v1/files/pull", {"filename":"shared.bin"}, expected=202)
    a.assert_file(job)
    assert shared.read_bytes() == DATA
    passed("peer pull preserves the original shared file")
    job = a.api("POST", "/v1/downloads", {"target":"peer","executor":"http","url":origin + "/slow","filename":"cancel.bin"}, expected=202)
    wait_for(lambda: a.api("GET", f'/v1/downloads/{job["id"]}')["received_bytes"] > 0)
    a.api("DELETE", f'/v1/downloads/{job["id"]}')
    assert a.job(job["id"])["status"] == "cancelled"
    a.api("GET", f'/v1/downloads/{job["id"]}/content', expected=404)
    assert not (a.root / "downloads" / f'{job["id"]}.part').exists()
    passed("cancelled peer download exposes no completed file")
    a.api("DELETE", f'/v1/downloads/{job["id"]}')
    a.api("GET", f'/v1/downloads/{job["id"]}', expected=404)
    passed("terminal transfer deletion removes its record")

    b.stop()
    wait_for(lambda: not a.api("GET", "/v1/health")["peer"]["connected"])
    error = a.api("POST", "/v1/rpc", {"method":"system.ping"}, expected=503)
    assert error["error"]["code"] == "PEER_DISCONNECTED"
    b.start()
    wait_for(lambda: a.api("GET", "/v1/health")["peer"]["connected"])
    assert ping(a)["result"]["protocol"] == 1
    passed("peer process exit reports offline and restart re-establishes RPC")


def browser_flows(a, b, origin, extension):
    from playwright.sync_api import sync_playwright, expect
    ui = ThreadingHTTPServer(("127.0.0.1", 4173), partial(QuietUI, directory=str(ROOT / "ui/dist")))
    threading.Thread(target=ui.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory() as profile, sync_playwright() as p:
            binary = os.environ.get("CHROMIUM_PATH") or shutil.which("chromium") or p.chromium.executable_path
            args = ["--no-sandbox"]
            if extension:
                path = ROOT / "browser-extension/dist"
                args += [f"--disable-extensions-except={path}", f"--load-extension={path}"]
            context = p.chromium.launch_persistent_context(profile, executable_path=binary, headless=True, ignore_default_args=["--disable-extensions"] if extension else [], args=args)
            context.set_default_timeout(15000)
            errors = []
            page = context.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.add_init_script(f"sessionStorage.setItem('serialbridge.port',{json.dumps(str(a.port))});sessionStorage.setItem('serialbridge.token',{json.dumps(a.token)});")
            page.goto("http://127.0.0.1:4173/")
            expect(page.locator("#side-name")).to_have_text("Node A")
            expect(page.locator("#demo-banner")).not_to_be_visible()
            passed("live UI reads actual Core state without simulated data")
            page.locator('[data-page="logs"]').click()
            expect(page.locator("#log-rows tr").first).to_be_visible()
            page.locator("#log-rows tr").first.click()
            expect(page.locator("#dialog")).to_be_visible()
            page.locator("#close-dialog").click()
            passed("live Core logs open details in UI")
            page.locator('[data-page="settings"]').click()
            page.locator('input[name="node_name"]').fill("Live Test A")
            page.locator('#settings-form button[type="submit"]').click()
            expect(page.locator("#toast")).to_contain_text("设置已保存")
            assert a.api("GET", "/v1/settings")["node_name"] == "Live Test A"
            page.locator("#new-pair").click()
            expect(page.locator(".pair-code")).to_have_text(__import__("re").compile("[A-F0-9]{12}"))
            passed("UI settings persist to Core and request a real pairing code")
            page.locator('[data-page="transfers"]').click()
            page.locator("#new-download").click()
            page.locator('select[name="executor"]').select_option("http")
            page.locator('input[name="url"]').fill(origin + "/file")
            page.locator('input[name="filename"]').fill("ui-download.bin")
            page.locator("#download-form button").click()
            row = page.locator(".transfer-live").filter(has_text="ui-download.bin")
            expect(row).to_contain_text("已完成", timeout=15000)
            job = next(t for t in a.api("GET", "/v1/downloads")["transfers"] if t["filename"] == "ui-download.bin")
            a.assert_file(job)
            passed("UI creates a real peer download and displays verified completion")

            if extension:
                worker = context.service_workers[0] if context.service_workers else context.wait_for_event("serviceworker")
                ext_id = worker.url.split("/")[2]
                popup = context.new_page()
                popup.goto(f"chrome-extension://{ext_id}/popup.html")
                code = b.api("POST", "/v1/extension/pairing", {})["code"]
                popup.locator("#port").fill(str(b.port))
                popup.locator("#code").fill(code)
                popup.locator('#pair-form button[type="submit"]').click()
                expect(popup.locator("#state")).to_have_text("已连接")
                passed("actual Chromium MV3 extension pairs with actual Core")
                value = a.api("POST", "/v1/rpc", {"method":"browser.fetch","params":{"url":origin + "/api"}})
                assert value["result"]["body"] == "fixture-api"
                passed("peer browser.fetch executes in the real extension")
                job = a.api("POST", "/v1/downloads", {"target":"peer","executor":"browser","url":origin + "/file","filename":"chrome-download.bin"}, expected=202)
                a.assert_file(job)
                passed("real browser download traverses Core and peer with matching SHA-256")
                b.api("DELETE", "/v1/extension/pairing")
                wait_for(lambda: not b.api("GET", "/v1/health")["browser"]["connected"])
                error = a.api("POST", "/v1/rpc", {"method":"browser.fetch","params":{"url":origin + "/api"}}, expected=503)
                assert error["error"]["code"] == "BROWSER_DISCONNECTED"
                passed("extension revocation fails browser RPC without executor fallback")

            a.stop()
            page.locator('[data-page="overview"]').click()
            page.locator("#refresh").click()
            expect(page.locator("#connection-pill")).to_have_text("本机服务未连接")
            expect(page.locator("#demo-banner")).not_to_be_visible()
            assert not errors, errors
            passed("actual Core shutdown clears UI online state without demo fallback")
            context.close()
    finally:
        ui.shutdown()
        ui.server_close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", action="store_true")
    parser.add_argument("--extension", action="store_true")
    parser.add_argument("--output", default=str(ROOT / ".superpowers/e2e-results.json"))
    options = parser.parse_args()
    fixture = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    fixture.daemon_threads = True
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    origin = f"http://127.0.0.1:{fixture.server_port}"
    nodes = []
    try:
        with tempfile.TemporaryDirectory(prefix="serialbridge-e2e-") as root:
            try:
                link_port = free_port()
                nodes.append(NodeProcess(Path(root), "Node A", origin, link_port, True))
                nodes.append(NodeProcess(Path(root), "Node B", origin, link_port, False))
                a, b = nodes
                core_flows(a, b, origin)
                if options.browser or options.extension:
                    browser_flows(a, b, origin, options.extension)
            finally:
                # Windows cannot delete a directory containing open process logs.
                # Stop the processes and close handles before TempDir cleanup.
                for node in nodes:
                    node.stop()
                    node.log.close()
    finally:
        fixture.shutdown()
        fixture.server_close()
        output = Path(options.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps({"scope":"real headless Core over loopback TCP; UI/extension only when requested; no physical serial hardware", "extension_requested": options.extension, "passed":RESULTS,"count":len(RESULTS)}, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"{len(RESULTS)} real integration checks passed")


if __name__ == "__main__":
    main()
