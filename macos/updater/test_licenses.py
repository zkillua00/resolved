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
        self.helper = self.root / "macos" / "updater"
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
                          else "unexpected local dependency" if field == "source"
                          else "dependency " + field + " mismatch")
                self.reject(reason, metadata)

    def add_first_party(self):
        manifest = self.root / "crates" / "resolved-release" / "Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text('[package]\nname = "resolved-release"\n')
        package = {
            "id": "release", "name": "resolved-release", "version": "0.0.0",
            "source": None, "license": "Apache-2.0", "license_file": None,
            "manifest_path": str(manifest),
        }
        self.metadata["packages"].append(package)
        self.metadata["resolve"]["nodes"].append({"id": package["id"]})
        return package

    def test_canonical_first_party_allowed(self):
        self.add_first_party()
        licenses.audit(self.metadata, self.output)
        self.assertTrue((self.output / "inventory.json").is_file())

    def test_first_party_identity_and_license_rejected(self):
        self.add_first_party()
        for field, value in [("version", "0.0.1"), ("source", "registry+unexpected"),
                             ("license", "MIT"), ("license_file", "LICENSE")]:
            with self.subTest(field=field):
                metadata = copy.deepcopy(self.metadata)
                metadata["packages"][-1][field] = value
                self.reject("wrong first-party resolved-release identity or license", metadata)

    def test_unexpected_local_package_rejected(self):
        self.add_first_party()["name"] = "another-local-package"
        self.reject("unexpected local dependency: another-local-package")

    def test_first_party_path_substitution_rejected(self):
        package = self.add_first_party()
        package["manifest_path"] = str(self.source / "Cargo.toml")
        self.reject("first-party resolved-release manifest path mismatch")

    def test_first_party_duplicate_rejected(self):
        package = dict(self.add_first_party(), id="duplicate-release")
        self.metadata["packages"].append(package)
        self.metadata["resolve"]["nodes"].append({"id": package["id"]})
        self.reject("duplicate first-party resolved-release dependency")

    def test_first_party_symlink_substitution_rejected(self):
        package = self.add_first_party()
        manifest = Path(package["manifest_path"])
        manifest.unlink()
        manifest.symlink_to(self.source / "Cargo.toml")
        self.reject("first-party resolved-release manifest path mismatch")

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

    def configure_supplemental(self):
        data = b"Original embedded upstream copyright\r\nBSD terms"
        (self.reviewed / "embedded.txt").write_bytes(data)
        path = self.reviewed / "inventory.json"
        inventory = json.loads(path.read_text())
        inventory["packages"][0]["supplemental_texts"] = [{
            "reviewed": "embedded.txt", "sha256": licenses.digest(data),
            "upstream_url": "https://example.invalid/revision/LICENSE",
        }]
        path.write_text(json.dumps(inventory))
        return data, inventory, path

    def test_supplemental_notice_ships_exact_bytes(self):
        data, _, _ = self.configure_supplemental()
        licenses.audit(self.metadata, self.output)
        self.assertEqual((self.output / "example-1.0.0" / "embedded.txt").read_bytes(), data)

    def test_changed_supplemental_notice_rejected(self):
        self.configure_supplemental()
        (self.reviewed / "embedded.txt").write_bytes(b"changed")
        self.reject("supplemental notice mismatch")

    def test_supplemental_notice_cannot_replace_source_validation(self):
        self.configure_supplemental()
        (self.source / "Cargo.toml").write_text("changed source")
        self.reject("upstream source text mismatch")

    def test_supplemental_notice_invalid_record_rejected(self):
        _, inventory, path = self.configure_supplemental()
        for field, value in [("reviewed", "../embedded.txt"), ("upstream_url", "file:///LICENSE"),
                             ("reviewed_lf_sha256", "not-supported")]:
            with self.subTest(field=field):
                invalid = copy.deepcopy(inventory)
                invalid["packages"][0]["supplemental_texts"][0][field] = value
                path.write_text(json.dumps(invalid))
                self.reject("invalid supplemental notice")

    def test_supplemental_notice_symlink_rejected(self):
        self.configure_supplemental()
        notice = self.reviewed / "embedded.txt"
        notice.unlink()
        notice.symlink_to(self.reviewed / "MIT.txt")
        self.reject("symlink supplemental notice")

    def configure_excerpt(self):
        upstream = b"not a notice\r\nCopyright test\r\nMIT terms"
        (self.source / "LICENSE-MIT").write_bytes(upstream)
        snapshot = b"Copyright test\nMIT terms\n"
        (self.reviewed / "MIT.txt").write_bytes(snapshot)
        path = self.reviewed / "inventory.json"
        inventory = json.loads(path.read_text())
        review = inventory["packages"][0]
        review["source_tree_sha256"] = licenses.source_digest(self.source)
        review["texts"][0].update(
            start_line=2, prefix_lines=2,
            sha256=licenses.digest(b"Copyright test\r\nMIT terms"),
            reviewed_lf_sha256=licenses.digest(snapshot),
        )
        path.write_text(json.dumps(inventory))
        return inventory, path

    def test_excerpt_and_lf_snapshot_ship_exact_upstream_bytes(self):
        self.configure_excerpt()
        licenses.audit(self.metadata, self.output)
        self.assertEqual((self.output / "example-1.0.0" / "MIT.txt").read_bytes(),
                         b"Copyright test\r\nMIT terms")

    def test_excerpt_wrong_range_rejected(self):
        inventory, path = self.configure_excerpt()
        inventory["packages"][0]["texts"][0]["start_line"] = 1
        path.write_text(json.dumps(inventory))
        self.reject("license/attribution text mismatch")

    def test_lf_snapshot_does_not_allow_upstream_byte_changes(self):
        inventory, path = self.configure_excerpt()
        (self.source / "LICENSE-MIT").write_bytes(b"not a notice\nCopyright test\nMIT terms")
        # Even if a source hash were re-reviewed, the exact text hash must match.
        inventory["packages"][0]["source_tree_sha256"] = licenses.source_digest(self.source)
        path.write_text(json.dumps(inventory))
        self.reject("license/attribution text mismatch")

    def test_lf_snapshot_hash_rejected(self):
        inventory, path = self.configure_excerpt()
        inventory["packages"][0]["texts"][0]["reviewed_lf_sha256"] = "wrong"
        path.write_text(json.dumps(inventory))
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
