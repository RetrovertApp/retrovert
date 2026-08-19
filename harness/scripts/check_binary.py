"""Binary audits on the built plugin shared library.

Linux: DT_NEEDED allowlist, versioned-symbol floor (every GLIBC_* need
<= 2.28, nothing but GLIBC_* needs), exact-export audit via readelf.
Windows: import-table System32 allowlist (regular + delay-load) and
exact-export audit via pefile.
"""

import argparse
import re
import sys

from rvcommon import (
    GLIBC_MAX_VERSION,
    LINUX_NEEDED_ALLOWLIST,
    LINUX_NEEDED_PREFIX_ALLOWLIST,
    REQUIRED_EXPORTS,
    TARGETS,
    WINDOWS_IMPORT_ALLOWLIST,
    fail,
    info,
    run_capture,
)


def check_linux(lib):
    dyn = run_capture(["readelf", "-dW", lib])
    needed = re.findall(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]", dyn)
    bad = [
        n
        for n in needed
        if n not in LINUX_NEEDED_ALLOWLIST
        and not n.startswith(LINUX_NEEDED_PREFIX_ALLOWLIST)
    ]
    if bad:
        fail(f"{lib}: DT_NEEDED outside the allowlist: {bad}")

    syms = run_capture(["readelf", "--dyn-syms", "-W", lib])
    exports = set()
    version_offenders = []
    for line in syms.splitlines():
        cols = line.split()
        if len(cols) < 8 or not cols[0].endswith(":"):
            continue
        bind, ndx, name = cols[4], cols[6], cols[7]
        if "@" in name:
            sym, _, version = name.partition("@")
            version = version.lstrip("@")
            m = re.fullmatch(r"GLIBC_(\d+)\.(\d+)(?:\.\d+)?", version)
            if not m:
                version_offenders.append(f"{sym} needs non-glibc version {version}")
            elif (int(m.group(1)), int(m.group(2))) > GLIBC_MAX_VERSION:
                version_offenders.append(f"{sym} needs {version}")
            name = sym
        if ndx != "UND" and bind in ("GLOBAL", "WEAK"):
            exports.add(name)
    if version_offenders:
        fail(
            f"{lib}: symbol versions above the glibc "
            f"{'.'.join(map(str, GLIBC_MAX_VERSION))} floor: {version_offenders}"
        )
    if exports != REQUIRED_EXPORTS:
        fail(
            f"{lib}: dynamic exports must be exactly {sorted(REQUIRED_EXPORTS)}, "
            f"got {sorted(exports)}"
        )
    info(f"linux binary audit passed: NEEDED={needed}, exports={sorted(exports)}")


def check_windows(lib):
    import pefile

    pe = pefile.PE(lib)

    imports = set()
    for attr in ("DIRECTORY_ENTRY_IMPORT", "DIRECTORY_ENTRY_DELAY_IMPORT"):
        for entry in getattr(pe, attr, []):
            imports.add(entry.dll.decode("ascii").lower())
    bad = sorted(imports - WINDOWS_IMPORT_ALLOWLIST)
    if bad:
        fail(f"{lib}: imports outside the System32 allowlist: {bad}")

    exports = set()
    for sym in getattr(getattr(pe, "DIRECTORY_ENTRY_EXPORT", None), "symbols", []):
        if sym.name:
            exports.add(sym.name.decode("ascii"))
    if exports != REQUIRED_EXPORTS:
        fail(
            f"{lib}: export table must be exactly {sorted(REQUIRED_EXPORTS)}, "
            f"got {sorted(exports)}"
        )
    info(f"windows binary audit passed: imports={sorted(imports)}, exports={sorted(exports)}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lib", required=True)
    ap.add_argument("--target", required=True, choices=TARGETS)
    args = ap.parse_args()
    if TARGETS[args.target]["os"] == "linux":
        check_linux(args.lib)
    else:
        check_windows(args.lib)


if __name__ == "__main__":
    sys.exit(main())
