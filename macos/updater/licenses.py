#!/usr/bin/env python3
"""Offline, fail-closed comparison to a reviewed Cargo dependency inventory."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys

HERE = Path(__file__).resolve().parent


def digest(data):
    return hashlib.sha256(data).hexdigest()


def source_digest(root):
    """Hash sorted relative paths and file digests, excluding Cargo cache markers."""
    entries = []
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError("symlink in dependency source: " + str(path))
        if path.is_file() and path.name not in (".cargo-ok", ".cargo-checksum.json"):
            entries.append([path.relative_to(root).as_posix(), digest(path.read_bytes())])
    return digest(json.dumps(entries, ensure_ascii=True, separators=(",", ":")).encode())


def audit(metadata, output):
    inventory_path = HERE / "licenses" / "inventory.json"
    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
    expected = {(p["name"], p["version"]): p for p in inventory["packages"]}
    packages = metadata["packages"]
    by_id = {p["id"]: p for p in packages}
    if len(by_id) != len(packages):
        raise ValueError("duplicate Cargo package identity")
    resolve = metadata["resolve"]
    nodes = {n["id"] for n in resolve["nodes"]}
    if nodes != set(by_id):
        raise ValueError("metadata must include exactly the complete resolved graph")
    own = by_id[resolve["root"]]
    if (own["name"], own["version"], own["source"]) != ("resolved-updater", "0.0.0", None):
        raise ValueError("wrong metadata root")
    if Path(own["manifest_path"]).resolve() != HERE / "Cargo.toml":
        raise ValueError("metadata root is not this helper")
    seen = set()
    copies = []
    for package in packages:
        if package["id"] == own["id"]:
            continue
        key = (package["name"], package["version"])
        if key not in expected or key in seen:
            raise ValueError("unreviewed or duplicate dependency: " + str(key))
        seen.add(key)
        review = expected[key]
        for field in ("license", "source", "repository"):
            if package.get(field) != review[field]:
                raise ValueError("dependency " + field + " mismatch: " + str(key))
        if package.get("license_file") is not None:
            raise ValueError("unexpected license_file: " + str(key))
        root = Path(package["manifest_path"]).parent
        if source_digest(root) != review["source_tree_sha256"]:
            raise ValueError("upstream source text mismatch: " + str(key))
        for text in review["texts"]:
            reviewed = HERE / "licenses" / text["reviewed"]
            data = reviewed.read_bytes()
            upstream = (root / text["upstream"]).read_bytes()
            if text.get("prefix_lines"):
                upstream = b"".join(upstream.splitlines(keepends=True)[:text["prefix_lines"]])
            if digest(data) != text["sha256"] or upstream != data:
                raise ValueError("license/attribution text mismatch: " + str(key))
            copies.append((reviewed, output / (package["name"] + "-" + package["version"]) / text["reviewed"]))
    if seen != set(expected):
        raise ValueError("resolved graph differs from reviewed inventory")
    # Validate everything before touching the fresh output directory.
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        raise ValueError("notices output must be absent or empty")
    output.mkdir(parents=True, exist_ok=True)
    for source, destination in copies:
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
    shutil.copyfile(inventory_path, output / "inventory.json")
    shutil.copyfile(HERE / "licenses" / "README.md", output / "README.md")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        audit(json.loads(args.metadata.read_text(encoding="utf-8")), args.output)
    except (ValueError, KeyError, TypeError, OSError) as error:
        print("updater license audit failed: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
