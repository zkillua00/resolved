"""Offline audit tests; run through the macOS bundler's verification mode."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import licenses


class LicenseAuditTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        # macOS exposes /var as a symlink to /private/var; match the audit's
        # canonical manifest paths instead of failing every test at the root check.
        self.root = Path(self.temporary.name).resolve()
        self.helper = self.root / "helper"
        self.reviewed = self.helper / "licenses"
        self.reviewed.mkdir(parents=True)
        self.source = self.root / "upstream"
        self.source.mkdir()
        self.text = b"Reviewed test license text\n"
        (self.source / "LICENSE-MIT").write_bytes(self.text)
        (self.source / "Cargo.toml").write_text('name = "example"\n')
        (self.reviewed / "MIT.txt").write_bytes(self.text)
        (self.reviewed / "README.md").write_text("Test notices\n")
        review = {
            "name": "example", "version": "1.0.0", "license": "MIT",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "repository": "https://example.invalid/example",
            "source_tree_sha256": licenses.source_digest(self.source),
            "texts": [{"upstream": "LICENSE-MIT", "reviewed": "MIT.txt",
                       "sha256": licenses.digest(self.text)}],
        }
        (self.reviewed / "inventory.json").write_text(json.dumps({"packages": [review]}))
        own = {"id": "helper", "name": "resolved-updater", "version": "0.0.0",
               "source": None, "manifest_path": str(self.helper / "Cargo.toml")}
        dependency = {key: review[key] for key in
                      ("name", "version", "license", "source", "repository")}
        dependency.update(id="dependency", manifest_path=str(self.source / "Cargo.toml"),
                          license_file=None)
        self.metadata = {"packages": [own, dependency], "resolve": {
            "root": "helper", "nodes": [{"id": "helper"}, {"id": "dependency"}]}}
        self.output = self.root / "notices"
        self.patcher = patch.object(licenses, "HERE", self.helper)
        self.patcher.start()
        self.addCleanup(self.patcher.stop)

    def reject(self, reason, metadata=None):
        with self.assertRaisesRegex(ValueError, reason):
            licenses.audit(metadata or self.metadata, self.output)
        self.assertFalse(self.output.exists())

    def test_success_preserves_exact_bytes_and_inventory(self):
        licenses.audit(self.metadata, self.output)
        self.assertEqual((self.output / "example-1.0.0" / "MIT.txt").read_bytes(), self.text)
        self.assertEqual((self.output / "inventory.json").read_bytes(),
                         (self.reviewed / "inventory.json").read_bytes())
        self.assertTrue((self.output / "README.md").is_file())

    def test_unreviewed_identity_version_license_source_repository(self):
        for field, value in [("name", "unreviewed"), ("version", "1.0.1"),
                             ("license", "GPL-3.0"), ("source", None),
                             ("repository", "https://other.invalid")]:
            with self.subTest(field=field):
                metadata = copy.deepcopy(self.metadata)
                metadata["packages"][1][field] = value
                reason = ("unreviewed or duplicate dependency" if field in ("name", "version")
                          else "dependency " + field + " mismatch")
                self.reject(reason, metadata)

    def test_extra_package(self):
        metadata = copy.deepcopy(self.metadata)
        extra = dict(metadata["packages"][1], id="extra", name="extra")
        metadata["packages"].append(extra)
        metadata["resolve"]["nodes"].append({"id": "extra"})
        self.reject("unreviewed or duplicate dependency", metadata)

    def test_missing_package(self):
        self.metadata["packages"].pop()
        self.metadata["resolve"]["nodes"].pop()
        self.reject("resolved graph differs from reviewed inventory")

    def test_partial_metadata(self):
        self.metadata["resolve"]["nodes"].pop()
        self.reject("metadata must include exactly the complete resolved graph")

    def test_changed_upstream_license(self):
        (self.source / "LICENSE-MIT").write_bytes(b"different license\n")
        self.reject("upstream source text mismatch")

    def test_changed_reviewed_license(self):
        (self.reviewed / "MIT.txt").write_bytes(b"different license\n")
        self.reject("license/attribution text mismatch")

    def test_changed_source_with_unchanged_label_and_license(self):
        (self.source / "Cargo.toml").write_text("changed source\n")
        self.reject("upstream source text mismatch")

    def test_symlink_source_rejected(self):
        (self.source / "link").symlink_to(self.source / "LICENSE-MIT")
        self.reject("symlink in dependency source")

    def test_nonempty_output_rejected(self):
        self.output.mkdir()
        (self.output / "existing").write_text("preserve")
        with self.assertRaisesRegex(ValueError, "notices output must be absent or empty"):
            licenses.audit(self.metadata, self.output)
        self.assertEqual(list(self.output.iterdir()), [self.output / "existing"])


if __name__ == "__main__":
    unittest.main()
