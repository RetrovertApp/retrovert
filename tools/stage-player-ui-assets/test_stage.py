import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("stage_assets", Path(__file__).with_name("stage.py"))
STAGE_ASSETS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STAGE_ASSETS)


class StageAssetsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.source.mkdir()
        (self.source / "asset.txt").write_bytes(b"verified")
        (self.source / "LICENSE.txt").write_bytes(b"MIT notice")
        manifest = {
            "version": 1,
            "licenses": {"MIT": "LICENSE.txt"},
            "assets": [
                {
                    "path": "asset.txt",
                    "size": 8,
                    "sha256": STAGE_ASSETS.file_sha256(self.source / "asset.txt"),
                    "license": "MIT",
                },
                {
                    "path": "LICENSE.txt",
                    "size": 10,
                    "sha256": STAGE_ASSETS.file_sha256(self.source / "LICENSE.txt"),
                    "license": "MIT",
                },
            ],
        }
        (self.source / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def test_stages_verified_tree_and_manifest(self):
        destination = self.root / "staged"
        STAGE_ASSETS.stage(self.source, destination)
        self.assertEqual((destination / "asset.txt").read_bytes(), b"verified")
        self.assertEqual((destination / "LICENSE.txt").read_bytes(), b"MIT notice")
        self.assertTrue((destination / "manifest.json").is_file())

    def test_hash_mismatch_fails_before_replacing_destination(self):
        destination = self.root / "staged"
        destination.mkdir()
        (destination / "keep.txt").write_text("old", encoding="utf-8")
        (self.source / "asset.txt").write_bytes(b"tampered")
        with self.assertRaisesRegex(STAGE_ASSETS.StageError, "SHA-256 mismatch"):
            STAGE_ASSETS.stage(self.source, destination)
        self.assertEqual((destination / "keep.txt").read_text(encoding="utf-8"), "old")

    def test_unlisted_file_fails(self):
        (self.source / "extra.txt").write_text("extra", encoding="utf-8")
        with self.assertRaisesRegex(STAGE_ASSETS.StageError, "manifest/tree mismatch"):
            STAGE_ASSETS.stage(self.source, self.root / "staged")


if __name__ == "__main__":
    unittest.main()
