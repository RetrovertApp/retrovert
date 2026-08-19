# Retrovert Player

Shared playback engine and Flowi-based renderers for Retrovert hosts.

The Cargo workspace contains four crates:

- `retrovert-player`: the headless playback engine
- `retrovert-player-ui`: the Flowi renderer layer
- `retrovert-plugin-catalog`: keeps the engine's decoder set current against an update channel
- `retrovert-player-desktop`: the desktop player binary, following the `dev` channel

`retrovert-player` owns headless decode coordination. `retrovert-player-ui` owns renderer
selection, the seven views, their Flowi resources, and their golden fixtures.
`retrovert-plugin-catalog` owns its own `retrovert-updater` instance and drives
check → apply → activate → retain from one `tick()` on the audio worker's loop;
generations activate all-or-nothing at the mount/stop boundary with a
reload-previous fallback. `retrovert-player-desktop` wires the engine and the
catalog to a cpal output and a stdin command loop.

## Checkout layout

The workspace resolves `retrovert-host` and `retrovert-updater` from the sibling Retrovert
checkout. The expected layout is:

```text
projects/
├── retrovert/
│   ├── retrovert_api/
│   │   └── rust/
│   │       └── retrovert-host/
│   └── retrovert-updater/
└── retrovert-player/
```

Replay Frontend is not a build dependency and need not be checked out. From the repository
root, run:

```sh
./scripts/check.sh
```

Original code is MIT licensed. Third-party asset terms and provenance records live in
[`LICENSES`](LICENSES/README.md).
