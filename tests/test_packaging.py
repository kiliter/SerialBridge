"""Check the installable ZIP structure and deterministic output."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("package_extension", ROOT / "scripts/package-extension.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ExtensionPackaging(unittest.TestCase):
    def test_zip_contains_loadable_manifest_and_all_referenced_assets(self):
        with tempfile.TemporaryDirectory() as output:
            path = module.package(output_dir=Path(output))
            before = path.read_bytes()
            with zipfile.ZipFile(path) as archive:
                self.assertIsNone(archive.testzip())
                manifest = json.loads(archive.read("manifest.json"))
                self.assertEqual(manifest["manifest_version"], 3)
                self.assertIn(manifest["background"]["service_worker"], archive.namelist())
                self.assertIn(manifest["action"]["default_popup"], archive.namelist())
                self.assertFalse(any(name.startswith("dist/") for name in archive.namelist()))
            self.assertEqual(module.package(output_dir=Path(output)).read_bytes(), before)

    def test_missing_built_assets_fail_without_creating_a_package(self):
        with tempfile.TemporaryDirectory() as root:
            source = Path(root) / "source"
            source.mkdir()
            (source / "manifest.json").write_text(json.dumps({"background":{"service_worker":"missing.js"},"action":{"default_popup":"popup.html"}}))
            with self.assertRaises(FileNotFoundError):
                module.package(source=source, output_dir=Path(root) / "output")
            self.assertFalse((Path(root) / "output").exists())


if __name__ == "__main__":
    unittest.main()
