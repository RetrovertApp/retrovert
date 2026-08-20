# retrovert-build-harness

The shared, versioned build harness for Retrovert playback plugins. Every
rostered plugin repo builds its own release artifacts through the reusable
workflow here; the harness owns the release floor and the full per-artifact
check set, so plugin repos carry only their source, a `harness.toml`, and a
three-line caller workflow.

Artifact identity is `(plugin, revision, target, harness version)`. Any
change to flags, checks, allowlists, pinned headers, or the toolchain image
is a harness version bump (a new `v<N>` tag), which correctly invalidates
every cached build. An artifact built by harness vN can only exist if every
vN check passed — the build-provenance attestation is the evidence.

## Targets

`linux-x86_64`, `linux-arm64` (glibc 2.28 floor, armv8-a baseline, static
C++/compiler runtimes), `windows-x86_64` (Windows 10 floor, static `/MT`
CRT, clang-cl).

## What runs per artifact

1. ABI header-digest check against the pinned `retrovert_api` set (`abi/`),
   plus the reported `api_version` cross-check in the load smoke.
2. Build through `cmake/harness.cmake`: hidden visibility, version-script /
   dllexport-controlled exports, static runtimes, reproducibility flags.
3. Binary audit — Linux: DT_NEEDED allowlist + glibc ≤ 2.28 versioned-symbol
   audit + exact-export audit; Windows: System32 import allowlist (regular +
   delay-load) + exact-export audit.
4. Deterministic packaging: flat `tar.zst` (`<name>_playback.*`, optional
   `<name>_data/`, mandatory `<name>_licenses/`), sorted entries, uid/gid 0,
   0755/0644 modes, mtime = plugin commit timestamp, pinned zstd level.
5. Archive lint + both-ways `harness.toml` data declaration check.
6. Load/unload smoke and ≥10 s non-silent playback smoke against the
   fixture(s) the plugin declares by sha256 in its `harness.toml`,
   through `host/rv_host` — on Linux inside the
   glibc-2.28 toolchain container, playback sealed in a network-less chroot
   jail where only the extracted generation and the fixture are readable
   (the injected-RVIo compliance gate).

Determinism is enforced by this repo's own CI: the full pipeline runs twice
against a pinned `playback-spu` revision from different build paths and the
artifacts must be byte-identical.

## Rolling a plugin repo on

1. Add `harness.toml`: plugin name, payload-data declaration, and at
   least one playback fixture (a redistribution-clean file in the plugin
   repo, declared with its sha256). Fixture identity rides in the plugin
   revision, so nothing here changes when a plugin joins. A fixture whose
   redistribution grant requires the unmodified upstream archive may be
   declared as that zip plus `members = ["<path in zip>"]`; the members
   are extracted and smoked in the archive's place.
2. Vendor headers in `include/retrovert/` matching the pinned set.
3. Ship license texts at the repo root (`LICENSE*`, `COPYING*`, `NOTICE*`
   or a `licenses/` directory).
4. Add the caller workflow:

```yaml
name: Release artifacts
on:
  push:
    branches: [master]
  workflow_dispatch: {}
permissions:
  contents: write
  id-token: write
  attestations: write
  packages: read
jobs:
  build:
    uses: RetrovertApp/retrovert-build-harness/.github/workflows/build-plugin.yml@v5
```

Artifacts land on the repo's rolling `builds` release as
`<name>-<target>-<shortsha>-h<harness>.tar.zst` with provenance
attestations; the `playback_plugins` gather consumes them from there.
