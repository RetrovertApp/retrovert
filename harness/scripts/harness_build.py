"""Build one plugin repo into a validated, deterministic release artifact.

This is the single entry point the reusable workflow (and local runs) use:
configure/build through the harness floor, run every per-artifact check,
package, and smoke through rv_host. Any failure exits non-zero — an
artifact only exists if every check of this harness version passed.

The Linux playback smoke runs inside a sealed jail (chroot + unshared
network) when --sandbox jail is given: the only readable trees are the
extracted generation, the fixture, and the loader/libc needed to run the
host. That requires root (CI runs it in a privileged container); local
unprivileged runs use --sandbox none, CI is the gate.
"""

import argparse
import hashlib
import os
import platform
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

from rvcommon import TARGETS, fail, info, load_harness_toml, run
import check_abi_headers
import check_artifact
import check_binary
import package_artifact

PLAY_SECONDS = 10


def native_target():
    machine = platform.machine().lower()
    if os.name == "nt":
        return "windows-x86_64" if machine in ("amd64", "x86_64") else None
    if sys.platform == "linux":
        if machine == "x86_64":
            return "linux-x86_64"
        if machine in ("aarch64", "arm64"):
            return "linux-arm64"
    return None


def cmake_configure_args(target, harness):
    args = [
        "-G", "Ninja",
        "-DCMAKE_BUILD_TYPE=Release",
        f"-DCMAKE_PROJECT_INCLUDE={harness / 'cmake' / 'harness.cmake'}",
    ]
    if TARGETS[target]["os"] == "windows":
        args += ["-DCMAKE_C_COMPILER=clang-cl", "-DCMAKE_CXX_COMPILER=clang-cl"]
    return args


def build_cmake(src, build_dir, extra_args):
    run(["cmake", "-S", src, "-B", build_dir] + extra_args)
    run(["cmake", "--build", build_dir])


def find_lib(build_dir, name, target):
    lib_name = f"{name}_playback{TARGETS[target]['lib_suffix']}"
    hits = [p for p in build_dir.rglob(lib_name) if p.is_file()]
    if len(hits) != 1:
        fail(f"expected exactly one {lib_name} under {build_dir}, found {hits}")
    return hits[0]


def extract_generation(archive, dest):
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    raw = package_artifact.zstd_decompress(archive.read_bytes())
    import io

    with tarfile.open(fileobj=io.BytesIO(raw)) as tar:
        try:
            tar.extractall(dest, filter="data")
        except TypeError:
            tar.extractall(dest)
    return dest


def fixture_files(repo, cfg):
    # Fixtures are declared by hash in the plugin's own harness.toml, so
    # fixture identity travels with the plugin revision and the harness
    # holds no per-plugin knowledge.
    files = []
    for f in cfg["fixtures"]:
        path = repo / f["file"]
        if not path.is_file():
            fail(f"declared fixture {f['file']} is missing from the plugin repo")
        if hashlib.sha256(path.read_bytes()).hexdigest() != f["sha256"]:
            fail(f"fixture {f['file']} does not match its declared sha256")
        files.append(path)
    return files


def build_jail(jail, host_bin, generation, fixtures):
    if jail.exists():
        shutil.rmtree(jail)
    jail.mkdir(parents=True)

    shutil.copyfile(host_bin, jail / "rv_host")
    os.chmod(jail / "rv_host", 0o755)

    ldd = subprocess.run(["ldd", str(host_bin)], check=True, capture_output=True, text=True).stdout
    libs = set()
    for line in ldd.splitlines():
        parts = line.split()
        if "=>" in parts and len(parts) >= 3 and parts[2].startswith("/"):
            libs.add(parts[2])
        elif parts and parts[0].startswith("/"):
            libs.add(parts[0])
    for lib in libs:
        dest = jail / lib.lstrip("/")
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(lib, dest)
        os.chmod(dest, 0o755)

    shutil.copytree(generation, jail / "generation")
    fdir = jail / "fixtures"
    fdir.mkdir()
    for f in fixtures:
        shutil.copyfile(f, fdir / f.name)
    return jail


def run_smoke(host_bin, lib_rel, fixtures, generation, jail_root, sandbox):
    if sandbox == "jail":
        jail = build_jail(jail_root, host_bin, generation, fixtures)
        base = ["unshare", "--net", "--pid", "--fork", "chroot", str(jail)]
        run(base + ["/rv_host", "load", f"/generation/{lib_rel}"])
        for f in fixtures:
            run(base + ["/rv_host", "play", f"/generation/{lib_rel}", f"/fixtures/{f.name}", str(PLAY_SECONDS)])
    else:
        run([host_bin, "load", generation / lib_rel])
        for f in fixtures:
            run([host_bin, "play", generation / lib_rel, f, str(PLAY_SECONDS)])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True, type=Path)
    ap.add_argument("--harness", type=Path, default=Path(__file__).resolve().parent.parent)
    ap.add_argument("--target", choices=TARGETS)
    ap.add_argument("--work", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--sandbox", choices=("auto", "jail", "none"), default="auto")
    args = ap.parse_args()

    target = args.target or native_target()
    if target is None:
        fail("cannot infer a build target for this machine; pass --target")
    if target != native_target():
        fail(f"target {target} cannot be built on this {platform.machine()} host")

    repo = args.repo.resolve()
    harness = args.harness.resolve()
    work = args.work.resolve()
    is_linux = TARGETS[target]["os"] == "linux"

    sandbox = args.sandbox
    if sandbox == "auto":
        sandbox = "jail" if is_linux and os.geteuid() == 0 else "none"
    if sandbox == "none" and is_linux:
        info("WARNING: running the Linux playback smoke unsandboxed (dev mode)")

    cfg = load_harness_toml(repo)
    name = cfg["name"]
    info(f"building {name} for {target} (harness at {harness})")

    check_abi_headers.check(harness, repo / "include" / "retrovert")

    plugin_build = work / "plugin-build"
    build_cmake(repo, plugin_build, cmake_configure_args(target, harness))
    lib = find_lib(plugin_build, name, target)

    check_binary.check_linux(str(lib)) if is_linux else check_binary.check_windows(str(lib))

    host_build = work / "host-build"
    build_cmake(harness / "host", host_build, ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release"])
    host_bin = host_build / ("rv_host.exe" if os.name == "nt" else "rv_host")

    artifact = args.out / f"{name}-{target}.tar.zst"
    package_artifact.make_tar_zst(
        package_artifact.stage(repo, lib, name, cfg["data_entries"], work / "staging"),
        artifact,
        package_artifact.commit_timestamp(repo),
    )
    check_artifact.check(artifact, name, target, cfg["data_entries"])

    generation = extract_generation(artifact, work / "generation")
    lib_rel = f"{name}_playback{TARGETS[target]['lib_suffix']}"
    fixtures = fixture_files(repo, cfg)
    run_smoke(host_bin, lib_rel, fixtures, generation, work / "jail", sandbox)

    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    (args.out / f"{name}-{target}.tar.zst.sha256").write_text(f"{digest}  {artifact.name}\n")
    info(f"artifact ready: {artifact} sha256={digest}")
    print(digest)


if __name__ == "__main__":
    main()
