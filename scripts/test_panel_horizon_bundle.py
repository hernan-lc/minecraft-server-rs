"""Opt-in HTTP smoke test for a panel and its locally built Horizon bundle."""

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
import urllib.error
import urllib.request


@unittest.skipUnless(
    all(os.environ.get(name) for name in ("MCPANEL_SMOKE_BIN", "HORIZON_SMOKE_SERVER", "HORIZON_SMOKE_ENGINE")),
    "requires a built panel and Horizon programs",
)
class PanelHorizonBundleSmokeTest(unittest.TestCase):
    def test_administrator_installs_bundle_without_game_files(self):
        with tempfile.TemporaryDirectory(prefix="panel-bundle-smoke-") as work:
            root = Path(work)
            package = root / "package"
            bundle = package / "runtime/horizon"
            bundle.mkdir(parents=True)
            binary = package / "mcpanel"
            for variable, target in [("MCPANEL_SMOKE_BIN", binary),
                                     ("HORIZON_SMOKE_SERVER", bundle / "horizon-server"),
                                     ("HORIZON_SMOKE_ENGINE", bundle / "horizon-engine")]:
                shutil.copy2(os.environ[variable], target)
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            base = f"http://127.0.0.1:{port}"
            cookies = http.cookiejar.CookieJar()
            client = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPCookieProcessor(cookies))
            environment = dict(os.environ, MCPANEL_NO_TRAY="1")
            with (root / "panel.log").open("w+") as log:
                panel = subprocess.Popen([str(binary), "--data-dir", str(root / "data"), "--bind", f"127.0.0.1:{port}",
                                          "--playit-mode", "external"], stdout=log, stderr=subprocess.STDOUT, env=environment)
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
                    server = request("/api/servers", {"name": "Native smoke", "core": "horizon", "version": "26.3",
                                                      "memory": {"min_mb": 256, "max_mb": 1024}, "port": 25565,
                                                      "eula_accepted": False})
                    self.assertTrue(server["horizon_programs_bundled"])
                    self.assertFalse(server["horizon_programs_present"])
                    self.assertTrue(request("/api/catalog/horizon/install", method="POST")["ok"])
                    provider = next(item for item in request("/api/catalog/providers") if item["id"] == "horizon")
                    self.assertTrue(provider["programs_present"])
                    server = request("/api/servers/" + server["id"])
                    self.assertTrue(server["horizon_programs_present"])
                    self.assertFalse((root / "data/horizon/.local").exists())
                    with self.assertRaises(urllib.error.HTTPError) as conflict:
                        request("/api/catalog/horizon/install", method="POST")
                    self.assertEqual(conflict.exception.code, 409)
                    conflict.exception.close()
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
