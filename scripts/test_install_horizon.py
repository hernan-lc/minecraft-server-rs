#!/usr/bin/env python3
"""Tests for the host-owned Horizon installer using small stand-in binaries."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().with_name("install-horizon.sh")


class InstallHorizonTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="horizon install test ")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.build = self.root / "built programs"
        self.data = self.root / "panel data"
        self.resources = self.root / "prepared files"
        self.build.mkdir()
        self.resources.mkdir()
        (self.resources / "horizon-resources.json").write_text('{"minecraft":"26.3"}')
        (self.resources / "marker").write_text("personal game data")
        self.make_program(
            "horizon-server",
            'if [ "$1" = "--help" ]; then echo "Horizon Minecraft 26.3"; exit 0; fi\n'
            'if [ "$1" = "--dir" ] && [ "$3" = "import-resources" ] '
            '&& [ -f "$4/horizon-resources.json" ]; then\n'
            '  cp -R "$4/." "$2/resources/"; exit $?\nfi\nexit 1\n',
        )
        self.make_program("horizon-client", 'test "$1" = "--help"\n')

    def make_program(self, name, body):
        path = self.build / name
        shell = shutil.which("sh")
        path.write_text(f"#!{shell}\n{body}")
        path.chmod(0o700)

    def run_installer(self, *extra):
        return subprocess.run(
            ["bash", str(SCRIPT), "--data-dir", str(self.data),
             "--from-build-dir", str(self.build), *map(str, extra)],
            text=True, capture_output=True, check=False,
        )

    def assert_no_staging(self):
        self.assertEqual(list(self.data.glob(".horizon-install.*")), [])

    def test_installs_both_programs_and_validated_shared_resources(self):
        result = self.run_installer("--resources", self.resources)
        self.assertEqual(result.returncode, 0, result.stderr)
        destination = self.data / "horizon"
        self.assertTrue(os.access(destination / "horizon-server", os.X_OK))
        self.assertTrue(os.access(destination / "horizon-engine", os.X_OK))
        installed = destination / ".local/horizon-client/26.3/marker"
        self.assertEqual(installed.read_text(), "personal game data")
        self.assertEqual((self.resources / "marker").read_text(), "personal game data")
        self.assert_no_staging()

    def test_failed_program_validation_never_publishes_installation(self):
        self.make_program("horizon-client", "exit 1\n")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.data / "horizon").exists())
        self.assert_no_staging()

    def test_wrong_minecraft_version_is_not_installed(self):
        self.make_program("horizon-server", 'echo "Horizon Minecraft 1.21"\n')
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.data / "horizon").exists())
        self.assertIn("26.3", result.stderr)
        self.assert_no_staging()

    def test_failed_resource_import_never_publishes_installation(self):
        (self.resources / "horizon-resources.json").unlink()
        result = self.run_installer("--resources", self.resources)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.data / "horizon").exists())
        self.assert_no_staging()

    def test_resource_symlink_is_rejected_without_touching_its_target(self):
        target = self.root / "outside"
        target.write_text("keep")
        (self.resources / "link").symlink_to(target)
        result = self.run_installer("--resources", self.resources)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(target.read_text(), "keep")
        self.assertFalse((self.data / "horizon").exists())
        self.assert_no_staging()

    def test_existing_installation_is_not_modified(self):
        existing = self.data / "horizon"
        existing.mkdir(parents=True)
        (existing / "keep").write_text("existing")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((existing / "keep").read_text(), "existing")
        self.assertEqual([path.name for path in existing.iterdir()], ["keep"])
        self.assert_no_staging()

    def test_builds_the_pinned_submodule_with_headless_engine_options(self):
        repo = SCRIPT.parent.parent
        source = repo / "vendor/horizon"
        pinned = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD:vendor/horizon"], capture_output=True, text=True)
        current = subprocess.run(["git", "-C", str(source), "rev-parse", "HEAD"], capture_output=True, text=True)
        clean = subprocess.run(["git", "-C", str(source), "status", "--porcelain", "--untracked-files=no"], capture_output=True, text=True)
        if pinned.returncode or current.returncode or clean.returncode or pinned.stdout != current.stdout or clean.stdout:
            self.skipTest("pinned clean Horizon submodule is unavailable")
        tools = self.root / "tools"
        tools.mkdir()
        cargo = tools / "cargo"
        shell = shutil.which("sh")
        cargo.write_text(
            f"#!{shell}\nset -eu\n"
            'printf "%s\\n" "$*" >> "$HORIZON_TEST_CARGO_LOG"\n'
            'mkdir -p "$CARGO_TARGET_DIR/debug"\n'
            'case "$*" in *"-p horizon-client"*) name=horizon-client;; '
            '*"-p horizon-server"*) name=horizon-server;; *) exit 1;; esac\n'
            'printf "#!%s\\necho Horizon Minecraft 26.3\\nexit 0\\n" "$HORIZON_TEST_SHELL" > "$CARGO_TARGET_DIR/debug/$name"\n'
            'chmod 700 "$CARGO_TARGET_DIR/debug/$name"\n'
        )
        cargo.chmod(0o700)
        log = self.root / "cargo log"
        environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"],
                           CARGO_TARGET_DIR=str(self.root / "cargo output"),
                           HORIZON_TEST_CARGO_LOG=str(log), HORIZON_TEST_SHELL=shell)
        result = subprocess.run(["bash", str(SCRIPT), "--data-dir", str(self.data), "--debug"],
                                env=environment, text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = log.read_text().splitlines()
        self.assertEqual(len(calls), 2)
        self.assertIn("--locked -p horizon-client --no-default-features --bin horizon-client", calls[0])
        self.assertIn("--locked -p horizon-server --bin horizon-server", calls[1])
        self.assertTrue((self.data / "horizon/horizon-engine").is_file())
        self.assert_no_staging()

    def test_packaged_installer_accepts_binaries_without_a_source_checkout(self):
        package_scripts = self.root / "release/scripts"
        package_scripts.mkdir(parents=True)
        packaged = package_scripts / "install-horizon.sh"
        shutil.copyfile(SCRIPT, packaged)
        no_source = subprocess.run(["bash", str(packaged), "--data-dir", str(self.data)],
                                   text=True, capture_output=True, check=False)
        self.assertNotEqual(no_source.returncode, 0)
        self.assertIn("--from-build-dir", no_source.stderr)
        result = subprocess.run(["bash", str(packaged), "--data-dir", str(self.data),
                                 "--from-build-dir", str(self.build)],
                                text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.data / "horizon/horizon-server").is_file())

    @unittest.skipUnless(
        os.environ.get("HORIZON_SMOKE_BUILD") and os.environ.get("HORIZON_SMOKE_RESOURCES"),
        "requires real Horizon binaries and prepared game files",
    )
    def test_real_binaries_import_the_prepared_game_files(self):
        self.build = Path(os.environ["HORIZON_SMOKE_BUILD"])
        source = Path(os.environ["HORIZON_SMOKE_RESOURCES"])
        result = self.run_installer("--resources", source)
        self.assertEqual(result.returncode, 0, result.stderr)
        installed = self.data / "horizon/.local/horizon-client/26.3"
        self.assertTrue((installed / "registries.json").is_file())
        self.assertTrue((installed / "commands.json").is_file())
        self.assertTrue(os.access(self.data / "horizon/horizon-engine", os.X_OK))
        self.assert_no_staging()


if __name__ == "__main__":
    unittest.main()
