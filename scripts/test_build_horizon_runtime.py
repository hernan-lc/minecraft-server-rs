"""Exercise release packaging without compiling Horizon or Minecraft game data."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class RuntimePackageTest(unittest.TestCase):
    def test_stages_all_runtime_programs_and_preparer_sources(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            cargo = fake_bin / "cargo"
            cargo.write_text("#!/bin/sh\nexit 0\n")
            cargo.chmod(0o755)
            target = root / "target" / "test-target" / "release"
            target.mkdir(parents=True)
            for name in ("horizon-server", "horizon-client", "horizon-prepare"):
                (target / name).write_text(name)
            output = root / "runtime" / "horizon"
            env = dict(os.environ, PATH=f"{fake_bin}{os.pathsep}{os.environ['PATH']}", CARGO_TARGET_DIR=str(root / "target"))
            subprocess.run([str(ROOT / "scripts/build-horizon-runtime.sh"), "test-target", str(output)], cwd=ROOT, env=env, check=True)
            self.assertEqual((output / "horizon-engine").read_text(), "horizon-client")
            self.assertEqual((output / "horizon-server").read_text(), "horizon-server")
            self.assertTrue((output / "prepare/horizon-prepare").is_file())
            self.assertTrue((output / "prepare/research-lock.json").is_file())
            self.assertTrue((output / "prepare/mods/horizon/src/test/java/dev/horizonlab/parity/PrepareWorker.java").is_file())
            self.assertTrue((output / "REVISION").read_text().strip())
            self.assertNotEqual(subprocess.run([str(ROOT / "scripts/build-horizon-runtime.sh"), "test-target", str(output)], cwd=ROOT, env=env, capture_output=True).returncode, 0)


if __name__ == "__main__":
    unittest.main()
