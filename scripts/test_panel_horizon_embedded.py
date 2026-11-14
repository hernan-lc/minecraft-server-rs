"""Opt-in smoke test for first-start installation from a standalone binary."""
import http.cookiejar
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.request


@unittest.skipUnless(os.environ.get("MCPANEL_EMBED_SMOKE_BIN"), "requires a panel built with an embedded Horizon fixture")
class EmbeddedHorizonSmoke(unittest.TestCase):
    def test_first_start_extracts_the_runtime_without_companion_files(self):
        with tempfile.TemporaryDirectory(prefix="panel-embedded-smoke-") as work:
            root = Path(work)
            suffix = ".exe" if os.name == "nt" else ""
            binary = root / f"mcpanel{suffix}"
            shutil.copy2(os.environ["MCPANEL_EMBED_SMOKE_BIN"], binary)
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            base = f"http://127.0.0.1:{port}"
            cookies = http.cookiejar.CookieJar()
            client = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPCookieProcessor(cookies))
            with (root / "panel.log").open("w+") as log:
                panel = subprocess.Popen([str(binary), "--data-dir", str(root / "data"), "--bind", f"127.0.0.1:{port}", "--playit-mode", "external", "--allow-unsandboxed-servers"], stdout=log, stderr=subprocess.STDOUT, env=dict(os.environ, MCPANEL_NO_TRAY="1"))
                try:
                    def request(path, body=None, method=None):
                        headers = {"Origin": base}
                        csrf = next((cookie.value for cookie in cookies if cookie.name == "mcpanel_csrf"), None)
                        if csrf:
                            headers["x-csrf-token"] = csrf
                        if body is not None:
                            headers["Content-Type"] = "application/json"
                        req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers=headers, method=method)
                        with client.open(req, timeout=60) as response:
                            return json.loads(response.read())
                    for _ in range(100):
                        if panel.poll() is not None:
                            self.fail("panel exited before startup")
                        try:
                            request("/api/setup/status")
                            break
                        except OSError:
                            time.sleep(0.1)
                    else:
                        self.fail("panel did not start")
                    password = secrets.token_urlsafe(24)
                    credentials = {"username": "smoke-admin", "password": password}
                    request("/api/setup", {**credentials, "confirm": password})
                    request("/api/auth/login", credentials)
                    provider = next(item for item in request("/api/catalog/providers") if item["id"] == "horizon")
                    self.assertTrue(provider["programs_bundled"])
                    self.assertFalse(provider["programs_present"])
                    server = request("/api/servers", {"name": "Native smoke", "core": "horizon", "version": "26.3", "memory": {"min_mb": 256, "max_mb": 1024}, "port": 25565, "eula_accepted": False})
                    request("/api/servers/" + server["id"] + "/power", {"action": "start"})
                    installed = root / "data/horizon"
                    self.assertTrue((installed / f"horizon-server{suffix}").is_file())
                    self.assertTrue((installed / f"horizon-engine{suffix}").is_file())
                    self.assertTrue((installed / f"prepare/horizon-prepare{suffix}").is_file())
                    self.assertFalse((root / "runtime").exists())
                finally:
                    if panel.poll() is None:
                        panel.terminate()
                        try:
                            panel.wait(timeout=30)
                        except subprocess.TimeoutExpired:
                            panel.kill()
                            panel.wait()


if __name__ == "__main__":
    unittest.main()
