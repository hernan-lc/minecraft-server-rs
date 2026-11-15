"""Opt-in real Horizon first-start smoke test using an isolated temporary world.

Requires MCPANEL_REAL_SMOKE_BIN, HORIZON_REAL_RUNTIME (installed runtime),
HORIZON_REAL_SOURCE (a checkout with previously imported game files and a built
horizon-prepare). It copies game inputs into temporary panel data; none are
committed or modified. Missing official libraries may be downloaded.
"""
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


@unittest.skipUnless(all(os.environ.get(k) for k in ("MCPANEL_REAL_SMOKE_BIN", "HORIZON_REAL_RUNTIME", "HORIZON_REAL_SOURCE")), "requires local Horizon runtime and imported Minecraft files")
class RealAutomaticHorizonSmoke(unittest.TestCase):
    def test_prepares_and_starts_a_fresh_server(self):
        source = Path(os.environ["HORIZON_REAL_SOURCE"])
        runtime = Path(os.environ["HORIZON_REAL_RUNTIME"])
        imported = source / ".local/minecraft/26.3"
        with tempfile.TemporaryDirectory(prefix="horizon-auto-smoke-") as work:
            root = Path(work)
            data = root / "data"
            horizon = data / "horizon"
            prep = horizon / "prepare"
            prep.mkdir(parents=True)
            for name in ("horizon-server", "horizon-engine"):
                shutil.copy2(runtime / name, horizon / name)
            shutil.copy2(source / "target/debug/horizon-prepare", prep / "horizon-prepare")
            shutil.copy2(source / "research-lock.json", prep / "research-lock.json")
            shutil.copytree(source / "mods/horizon/src/test/java", prep / "mods/horizon/src/test/java")
            # Keep the preparer's private cache empty so it must import the
            # verified launcher-style input files, as on a true first start.
            if (source / ".local/exporters").is_dir():
                shutil.copytree(source / ".local/exporters", prep / ".local/exporters")
            inputs = data / "horizon-downloads/26.3"
            (inputs / "versions/26.3").mkdir(parents=True)
            shutil.copy2(imported / "manifest.json", inputs / "versions/26.3/26.3.json")
            shutil.copy2(imported / "jars/client.jar", inputs / "versions/26.3/26.3.jar")
            shutil.copy2(imported / "jars/server.jar", inputs / "server.jar")
            shutil.copytree(imported / "libraries/client", inputs / "libraries")
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                game_port = listener.getsockname()[1]
            base = f"http://127.0.0.1:{port}"
            cookies = http.cookiejar.CookieJar()
            client = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPCookieProcessor(cookies))
            with (root / "panel.log").open("w+") as log:
                panel = subprocess.Popen([os.environ["MCPANEL_REAL_SMOKE_BIN"], "--data-dir", str(data), "--bind", f"127.0.0.1:{port}", "--playit-mode", "external", "--allow-unsandboxed-servers"], stdout=log, stderr=subprocess.STDOUT, env=dict(os.environ, MCPANEL_NO_TRAY="1"))
                server_id = None
                try:
                    def request(path, body=None):
                        headers = {"Origin": base}
                        csrf = next((cookie.value for cookie in cookies if cookie.name == "mcpanel_csrf"), None)
                        if csrf:
                            headers["x-csrf-token"] = csrf
                        if body is not None:
                            headers["Content-Type"] = "application/json"
                        req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers=headers)
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
                    password = secrets.token_urlsafe(24)
                    credentials = {"username": "smoke-admin", "password": password}
                    request("/api/setup", {**credentials, "confirm": password})
                    request("/api/auth/login", credentials)
                    server = request("/api/servers", {"name": "Disposable Horizon smoke", "core": "horizon", "version": "26.3", "memory": {"min_mb": 256, "max_mb": 512}, "port": game_port, "eula_accepted": True})
                    server_id = server["id"]
                    (data / "servers" / server_id / "server.toml").write_text("[server]\nbind = '127.0.0.1'\nonline_mode = false\nwhitelist = false\nview_distance = 2\nsimulation_distance = 2\n[engine]\nlow_memory = true\n")
                    request(f"/api/servers/{server_id}/power", {"action": "start"})
                    deadline = time.monotonic() + 1800
                    previous = None
                    while time.monotonic() < deadline:
                        state = request(f"/api/servers/{server_id}")
                        phase = (state["status"], (state.get("progress") or {}).get("stage"))
                        if phase != previous:
                            print("Horizon smoke:", phase, flush=True)
                            previous = phase
                        if state["status"] == "online":
                            break
                        if state["status"] in ("offline", "crashed"):
                            self.fail(f"Horizon stopped during setup: {request(f'/api/servers/{server_id}/logs')[-15:]}")
                        time.sleep(2)
                    else:
                        self.fail(f"Horizon setup timed out: {request(f'/api/servers/{server_id}/logs')[-15:]}")
                    self.assertTrue((horizon / ".local/horizon-client/26.3/horizon-resources.json").is_file())
                    request(f"/api/servers/{server_id}/power", {"action": "stop"})
                finally:
                    if panel.poll() is None:
                        panel.terminate()
                        try:
                            panel.wait(timeout=60)
                        except subprocess.TimeoutExpired:
                            panel.kill()
                            panel.wait()


if __name__ == "__main__":
    unittest.main()
