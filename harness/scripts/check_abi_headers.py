"""ABI identity check: the plugin's vendored retrovert/ headers must be
byte-identical to the harness-pinned canonical set (abi/HEADERS.sha256).

A plugin need not vendor every canonical header, but the mandatory core
set must be present, and a vendored header that isn't in the canonical
set at all means the plugin is speaking an ABI this harness doesn't pin.
The runtime half of this check (the plugin's reported api_version) runs
in the load smoke.
"""

import argparse
import hashlib
import sys
from pathlib import Path

from rvcommon import MANDATORY_ABI_HEADERS, fail, info


def load_pinned(harness):
    pinned = {}
    for line in (harness / "abi" / "HEADERS.sha256").read_text().splitlines():
        digest, name = line.split()
        pinned[Path(name).name] = digest
    return pinned


def check(harness, plugin_include):
    pinned = load_pinned(harness)
    vendored = sorted(plugin_include.glob("*.h"))
    if not vendored:
        fail(f"no vendored headers found in {plugin_include}")

    names = {p.name for p in vendored}
    missing = MANDATORY_ABI_HEADERS - names
    if missing:
        fail(f"mandatory ABI headers missing from {plugin_include}: {sorted(missing)}")

    drifted = []
    for header in vendored:
        if header.name not in pinned:
            fail(f"{header.name} is not part of the pinned canonical header set")
        # CRLF checkouts on Windows are not ABI drift; hash LF-normalized.
        digest = hashlib.sha256(header.read_bytes().replace(b"\r\n", b"\n")).hexdigest()
        if digest != pinned[header.name]:
            drifted.append(header.name)
    if drifted:
        fail(
            f"vendored headers drifted from the pinned retrovert_api revision: "
            f"{drifted} (re-sync the plugin's include/retrovert/)"
        )
    info(f"ABI header check passed: {len(vendored)} headers match the pinned set")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--harness", required=True, type=Path)
    ap.add_argument("--plugin-include", required=True, type=Path)
    args = ap.parse_args()
    check(args.harness, args.plugin_include)


if __name__ == "__main__":
    sys.exit(main())
