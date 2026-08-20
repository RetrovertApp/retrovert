"""Shared definitions for the Retrovert build harness scripts.

The allowlists here are contract enforcement, versioned with the harness:
widening either one is a harness version bump.
"""

import re
import subprocess
import sys

TARGETS = {
    "linux-x86_64": {"os": "linux", "lib_suffix": ".so"},
    "linux-arm64": {"os": "linux", "lib_suffix": ".so"},
    "windows-x86_64": {"os": "windows", "lib_suffix": ".dll"},
}

# Linux payloads may depend on the base runtime only; C++/compiler runtimes
# are statically linked, so libstdc++/libgcc_s are deliberately absent.
LINUX_NEEDED_ALLOWLIST = {
    "libc.so.6",
    "libm.so.6",
    "libpthread.so.0",
    "libdl.so.2",
    "librt.so.1",
}
LINUX_NEEDED_PREFIX_ALLOWLIST = ("ld-linux",)

GLIBC_MAX_VERSION = (2, 28)

# Core-OS System32 allowlist (case-insensitive). Compiler-runtime DLLs
# (vcruntime*/msvcp*/ucrt*) are forbidden by omission: /MT is mandatory.
WINDOWS_IMPORT_ALLOWLIST = {
    "kernel32.dll",
    "ntdll.dll",
    "advapi32.dll",
    # Core-OS since Windows 95; admitted for loopback socketpair IPC
    # (uade frontend <-> core), not for network use.
    "ws2_32.dll",
}

REQUIRED_EXPORTS = {"rv_playback_plugin"}

ZSTD_LEVEL = 19

MANDATORY_ABI_HEADERS = {"playback.h", "rv_types.h"}


def fail(msg):
    print(f"HARNESS CHECK FAILED: {msg}", file=sys.stderr)
    sys.exit(1)


def info(msg):
    print(f"harness: {msg}", file=sys.stderr)


def run(cmd, **kwargs):
    info("run: " + " ".join(str(c) for c in cmd))
    return subprocess.run([str(c) for c in cmd], check=True, **kwargs)


def run_capture(cmd):
    return subprocess.run(
        [str(c) for c in cmd], check=True, capture_output=True, text=True
    ).stdout


def load_harness_toml(repo):
    import tomllib

    path = repo / "harness.toml"
    if not path.is_file():
        fail(f"{path} is missing: every rostered plugin declares its contract there")
    with open(path, "rb") as f:
        cfg = tomllib.load(f)
    name = cfg.get("plugin", {}).get("name")
    if not name or not name.replace("_", "").isalnum():
        fail("harness.toml: [plugin] name is missing or not a plain short name")
    data_entries = cfg.get("data", {}).get("entries", [])
    if not isinstance(data_entries, list) or not all(
        isinstance(e, str) and e and "/" not in e and e != ".." for e in data_entries
    ):
        fail("harness.toml: [data] entries must be a list of plain top-level names")
    version = cfg.get("plugin", {}).get("version")
    fixtures = cfg.get("fixtures", [])
    if not isinstance(fixtures, list) or not fixtures:
        fail(
            "harness.toml: at least one [[fixtures]] entry (file + sha256) is "
            "required; a plugin without a playback fixture is a release blocker"
        )
    for f in fixtures:
        file = f.get("file") if isinstance(f, dict) else None
        if (
            not isinstance(file, str)
            or file.startswith(("/", "\\"))
            or ".." in file.split("/")
        ):
            fail("harness.toml: [[fixtures]] file must be a relative repo path")
        if not re.fullmatch(r"[0-9a-f]{64}", str(f.get("sha256", ""))):
            fail(f"harness.toml: [[fixtures]] {file} needs a lowercase sha256")
    return {
        "name": name,
        "data_entries": data_entries,
        "version": version,
        "fixtures": fixtures,
    }
