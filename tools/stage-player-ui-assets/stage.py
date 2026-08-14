#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile


class StageError(Exception):
    pass


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validated_entries(source):
    manifest_path = source / "manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise StageError(f"cannot read {manifest_path}: {error}") from error

    entries = manifest.get("assets")
    licenses = manifest.get("licenses")
    if (
        manifest.get("version") != 1
        or not isinstance(entries, list)
        or not isinstance(licenses, dict)
    ):
        raise StageError(f"invalid asset manifest: {manifest_path}")

    expected_paths = set()
    for entry in entries:
        if not isinstance(entry, dict):
            raise StageError("asset manifest entries must be objects")
        logical_path = entry.get("path")
        license_key = entry.get("license")
        if not isinstance(logical_path, str) or not logical_path:
            raise StageError("asset manifest entry has no logical path")
        relative = Path(logical_path)
        if relative.is_absolute() or ".." in relative.parts:
            raise StageError(f"unsafe logical path: {logical_path}")
        if not isinstance(license_key, str) or not license_key:
            raise StageError(f"asset has no license key: {logical_path}")
        if license_key not in licenses:
            raise StageError(f"asset has unknown license key: {logical_path}")
        if logical_path in expected_paths:
            raise StageError(f"duplicate logical path: {logical_path}")
        expected_paths.add(logical_path)

        asset_path = source / relative
        try:
            size = asset_path.stat().st_size
        except OSError as error:
            raise StageError(f"cannot stat {logical_path}: {error}") from error
        if size != entry.get("size"):
            raise StageError(
                f"size mismatch for {logical_path}: expected {entry.get('size')}, got {size}"
            )
        actual_hash = file_sha256(asset_path)
        if actual_hash != entry.get("sha256"):
            raise StageError(
                f"SHA-256 mismatch for {logical_path}: "
                f"expected {entry.get('sha256')}, got {actual_hash}"
            )

    actual_paths = {
        path.relative_to(source).as_posix()
        for path in source.rglob("*")
        if path.is_file() and path != manifest_path
    }
    if actual_paths != expected_paths:
        missing = sorted(expected_paths - actual_paths)
        unlisted = sorted(actual_paths - expected_paths)
        raise StageError(f"manifest/tree mismatch: missing={missing}, unlisted={unlisted}")
    for license_key, notice_path in licenses.items():
        if not isinstance(notice_path, str) or notice_path not in expected_paths:
            raise StageError(f"license notice is not manifested: {license_key}")
    return manifest_path, entries


def stage(source, destination):
    manifest_path, entries = validated_entries(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=f".{destination.name}-", dir=destination.parent))
    try:
        shutil.copy2(manifest_path, temporary / manifest_path.name)
        for entry in entries:
            relative = Path(entry["path"])
            target = temporary / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / relative, target)
        validated_entries(temporary)
        if destination.exists():
            shutil.rmtree(destination)
        os.replace(temporary, destination)
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("destination", type=Path)
    parser.add_argument(
        "--source",
        type=Path,
        default=Path(__file__).resolve().parents[2]
        / "crates"
        / "retrovert-player-ui"
        / "assets",
    )
    args = parser.parse_args()
    try:
        stage(args.source.resolve(), args.destination.resolve())
    except StageError as error:
        print(f"stage-player-ui-assets: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
