"""Archive lint + both-ways harness.toml data check on a produced tar.zst.

Structural safety rules mirror what the updater's extractor enforces:
regular files and directories only, relative paths, no links, no `..`.
Layout rules pin the flat-root contract, and the data declaration is
checked both ways (declared-but-missing and present-but-undeclared fail).
"""

import argparse
import io
import sys
import tarfile
from pathlib import Path

from rvcommon import TARGETS, fail, info
from package_artifact import zstd_decompress


def check(archive, name, target, data_entries):
    lib_name = f"{name}_playback{TARGETS[target]['lib_suffix']}"
    data_dir = f"{name}_data"
    licenses_dir = f"{name}_licenses"

    raw = zstd_decompress(Path(archive).read_bytes())
    seen = set()
    top_level = set()
    data_top_level = set()
    licenses_count = 0
    mtimes = set()

    with tarfile.open(fileobj=io.BytesIO(raw)) as tar:
        for m in tar:
            if not (m.isreg() or m.isdir()):
                fail(f"{m.name}: only regular files and directories are allowed")
            n = m.name
            if n.startswith("/") or n.startswith("\\") or ".." in n.split("/"):
                fail(f"{n}: paths must be relative with no '..'")
            if n in seen:
                fail(f"{n}: duplicate entry")
            seen.add(n)
            if m.uid != 0 or m.gid != 0 or m.uname or m.gname:
                fail(f"{n}: owner not normalized to 0/0")
            expected_mode = 0o755 if m.isdir() else 0o644
            if m.mode != expected_mode:
                fail(f"{n}: mode {oct(m.mode)}, expected {oct(expected_mode)}")
            mtimes.add(m.mtime)

            parts = n.split("/")
            top_level.add(parts[0])
            if parts[0] == data_dir and len(parts) > 1:
                data_top_level.add(parts[1])
            if parts[0] == licenses_dir and m.isreg():
                licenses_count += 1

    if len(mtimes) > 1:
        fail(f"mtimes are not uniform: {sorted(mtimes)}")

    allowed = {lib_name, licenses_dir} | ({data_dir} if data_entries else set())
    unexpected = top_level - allowed
    if unexpected:
        fail(f"unexpected top-level entries: {sorted(unexpected)}")
    if lib_name not in top_level:
        fail(f"{lib_name} missing from archive root")
    if licenses_count == 0:
        fail(f"{licenses_dir}/ is missing or empty")

    declared = set(data_entries)
    if declared != data_top_level:
        missing = declared - data_top_level
        undeclared = data_top_level - declared
        if missing:
            fail(f"harness.toml declares data entries missing from archive: {sorted(missing)}")
        fail(f"archive ships undeclared data entries: {sorted(undeclared)}")

    info(f"archive lint passed: {archive} ({len(seen)} entries)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--archive", required=True)
    ap.add_argument("--name", required=True)
    ap.add_argument("--target", required=True, choices=TARGETS)
    ap.add_argument("--data-entries", nargs="*", default=[])
    args = ap.parse_args()
    check(args.archive, args.name, args.target, args.data_entries)


if __name__ == "__main__":
    sys.exit(main())
