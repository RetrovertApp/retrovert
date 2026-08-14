# Retrovert Player

Shared playback engine and Flowi-based renderers for Retrovert hosts.

The Cargo workspace contains two crates:

- `retrovert-player`: the headless playback engine
- `retrovert-player-ui`: the Flowi renderer layer

`retrovert-player` owns headless decode coordination. `retrovert-player-ui` owns renderer
selection, the seven views, their Flowi resources, and their golden fixtures.

## Checkout layout

The workspace resolves `retrovert-host` from the sibling Retrovert checkout. The expected
layout is:

```text
projects/
├── retrovert/
│   └── retrovert_api/
│       └── rust/
│           └── retrovert-host/
└── retrovert-player/
```

Replay Frontend is not a build dependency and need not be checked out. From the repository
root, run:

```sh
./scripts/check.sh
```

Original code is MIT licensed. Third-party asset terms and provenance records live in
[`LICENSES`](LICENSES/README.md).
