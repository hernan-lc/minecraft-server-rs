import os
from pathlib import Path
import subprocess
import sys
import stat
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("migrate-paper-world.py")


class MigratePaperWorldTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        self.target = self.root / "target"
        self.write("adventure/level.dat", b"world")
        self.write("server.properties", b"level-name=adventure\n")

    def write(self, path, contents):
        path = self.source / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(contents)

    def run_migration(self):
        return subprocess.run([sys.executable, str(SCRIPT), "--source", str(self.source),
                               "--destination", str(self.target), "--world", "adventure"],
                              text=True, capture_output=True)

    def test_converts_both_layouts_in_copy_and_preserves_original(self):
        self.write("adventure_nether/DIM-1/region/r.0.0.mca", b"nether")
        self.write("adventure_the_end/DIM1/region/r.0.0.mca", b"end")
        self.write("adventure_nether/uid.dat", b"paper-data")
        self.write("adventure/dimensions/minecraft/overworld/data/minecraft/weather.dat", b"weather")
        self.target.mkdir()  # The panel creates an empty server directory.
        if os.name == "posix":
            self.source.chmod(0o755)
            self.target.chmod(0o700)
        result = self.run_migration()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.target / "adventure/DIM-1/region/r.0.0.mca").read_bytes(), b"nether")
        self.assertEqual((self.target / "adventure/DIM1/region/r.0.0.mca").read_bytes(), b"end")
        self.assertEqual((self.target / "adventure/data/minecraft/weather.dat").read_bytes(), b"weather")
        self.assertEqual((self.target / ".mcpanel-paper-originals/adventure_nether/uid.dat").read_bytes(), b"paper-data")
        self.assertTrue((self.source / "adventure_nether/DIM-1/region/r.0.0.mca").is_file())
        self.assertTrue((self.source / "adventure/dimensions/minecraft/overworld/data/minecraft/weather.dat").is_file())
        self.assertFalse((self.target / ".mcpanel-migration-in-progress").exists())
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(self.target.stat().st_mode), 0o700)

    def test_conflicts_leave_destination_blocked_and_source_unchanged(self):
        self.write("adventure_nether/DIM-1/region/old.mca", b"old")
        self.write("adventure/DIM-1/region/new.mca", b"new")
        result = self.run_migration()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("conflicting destination", result.stderr)
        self.assertEqual((self.target / "adventure/DIM-1/region/new.mca").read_bytes(), b"new")
        self.assertEqual((self.source / "adventure_nether/DIM-1/region/old.mca").read_bytes(), b"old")
        self.assertTrue((self.target / ".mcpanel-migration-in-progress").is_file())

    @unittest.skipUnless(hasattr(os, "symlink"), "requires symbolic links")
    def test_symlink_is_not_followed(self):
        outside = self.root / "outside"
        outside.write_text("outside")
        os.symlink(outside, self.source / "link")
        result = self.run_migration()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symbolic links", result.stderr)
        self.assertEqual(outside.read_text(), "outside")
        self.assertTrue((self.target / ".mcpanel-migration-in-progress").exists())

    def test_nonempty_destination_is_not_changed(self):
        self.target.mkdir()
        (self.target / "keep").write_text("untouched")
        result = self.run_migration()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.target / "keep").read_text(), "untouched")


if __name__ == "__main__":
    unittest.main()
