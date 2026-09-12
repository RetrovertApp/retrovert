"""Stage and package one plugin artifact as a deterministic tar.zst.

Layout (flat root): <name>_playback.<ext>, optional <name>_data/,
mandatory non-empty <name>_licenses/. Determinism: entries sorted by
path, uid/gid 0, modes 0755 (dirs) / 0644 (files), mtime = plugin
commit timestamp, ustar format, pinned zstd level, single thread.
"""

import argparse
import io
import shutil
import sys
import tarfile
from pathlib import Path

from rvcommon import TARGETS, ZSTD_LEVEL, fail, info, run_capture

LICENSE_GLOBS = ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*")


def zstd_compress(data):
    try:
        import zstandard

        cctx = zstandard.ZstdCompressor(level=ZSTD_LEVEL, write_checksum=False)
        return cctx.compress(data)
    except ImportError:
        import subprocess

        info("zstandard module missing, falling back to zstd CLI (dev only)")
        proc = subprocess.run(
            ["zstd", f"-{ZSTD_LEVEL}", "--single-thread", "--no-check", "-c"],
            input=data,
            capture_output=True,
            check=True,
        )
        return proc.stdout


def zstd_decompress(data):
    try:
        import zstandard

        return zstandard.ZstdDecompressor().decompress(
            data, max_output_size=1 << 31
        )
    except ImportError:
        import subprocess

        proc = subprocess.run(
            ["zstd", "-d", "-c"], input=data, capture_output=True, check=True
        )
        return proc.stdout


def stage(repo, lib_path, name, data_entries, staging):
    if staging.exists():
        shutil.rmtree(staging)
    staging.mkdir(parents=True)

    shutil.copyfile(lib_path, staging / lib_path.name)

    licenses = staging / f"{name}_licenses"
    licenses.mkdir()
    for pattern in LICENSE_GLOBS:
        for src in sorted(repo.glob(pattern)):
            if src.is_file():
                shutil.copyfile(src, licenses / src.name)
    licenses_dir = repo / "licenses"
    if licenses_dir.is_dir():
        for src in sorted(licenses_dir.rglob("*")):
            if src.is_file():
                shutil.copyfile(src, licenses / src.name)
    if not any(licenses.iterdir()):
        fail(f"no license texts found in {repo}: <name>_licenses/ must not be empty")

    if data_entries:
        data_src = repo / f"{name}_data"
        if not data_src.is_dir():
            fail(f"harness.toml declares data entries but {data_src} does not exist")
        shutil.copytree(data_src, staging / f"{name}_data")

    return staging


def commit_timestamp(repo):
    return int(run_capture(["git", "-C", repo, "log", "-1", "--format=%ct"]).strip())


def make_tar_zst(staging, out_path, mtime):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        entries = sorted(staging.rglob("*"), key=lambda p: str(p.relative_to(staging)))
        for path in entries:
            arcname = str(path.relative_to(staging)).replace("\\", "/")
            ti = tarfile.TarInfo(arcname)
            ti.uid = 0
            ti.gid = 0
            ti.uname = ""
            ti.gname = ""
            ti.mtime = mtime
            if path.is_dir():
                ti.type = tarfile.DIRTYPE
                ti.mode = 0o755
                tar.addfile(ti)
            elif path.is_file():
                ti.type = tarfile.REGTYPE
                ti.mode = 0o644
                ti.size = path.stat().st_size
                with open(path, "rb") as f:
                    tar.addfile(ti, f)
            else:
                fail(f"unsupported file type in staging tree: {path}")

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(zstd_compress(buf.getvalue()))
    info(f"packaged {out_path} ({out_path.stat().st_size} bytes, mtime {mtime})")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True, type=Path)
    ap.add_argument("--lib", required=True, type=Path)
    ap.add_argument("--name", required=True)
    ap.add_argument("--target", required=True, choices=TARGETS)
    ap.add_argument("--data-entries", nargs="*", default=[])
    ap.add_argument("--staging", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    staging = stage(args.repo, args.lib, args.name, args.data_entries, args.staging)
    make_tar_zst(staging, args.out, commit_timestamp(args.repo))


if __name__ == "__main__":
    sys.exit(main())
