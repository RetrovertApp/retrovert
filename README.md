# Retrovert

Host-side Rust for Retrovert: a headless playback engine for retro music
formats, the plugin catalogue that feeds it decoders, and a player binary that
wires the two to an audio device.

| Crate | What it is |
| --- | --- |
| [`retrovert-player`](crates/player) | the headless playback engine |
| [`playlist-engine`](crates/playlist) | host-agnostic playlist storage, persistence and playback ordering |
| [`retrovert-plugin-catalog`](crates/catalog) | keeps the engine's decoder set current |
| [`retrovert-player-desktop`](crates/desktop) | the `retrovert` binary: engine and catalogue on a cpal output, driven by a stdin command loop |

Decoders are plugins and live in
[`playback_plugins`](https://github.com/RetrovertApp/playback_plugins); the
plugin ABI lives in
[`retrovert_api`](https://github.com/RetrovertApp/retrovert_api).
[`harness/`](harness) is the shared build harness those plugins build through,
versioned on its own `harness/v<N>` tags.

`retrovert-player` owns headless decode coordination. Renderers are not part of
this workspace: the player UI draws with flowi and lives with the host that
embeds it.

`crates/tuf`, `crates/updater` and `crates/publish` are the machinery that keeps
an installed plugin set current and verified. Nothing above asks you to think
about it; [`crates/publish/README.md`](crates/publish/README.md) covers it if you
ever have to operate it.

## Building

All dependencies resolve from a plain clone; no sibling checkouts are needed.
The player binary needs ALSA headers on Linux (`libasound2-dev`). From the
repository root, run:

```sh
./scripts/check.sh
```

## Running

```sh
cargo run -p retrovert-player-desktop -- FILE...
```

It reads commands on stdin: `play <path|index>`, `stop`, `seek <ms>`, `status`,
`quit`.

## Licensing

Original code is MIT licensed; `retrovert-tuf`, `retrovert-updater` and
`retrovert-publish` are `MIT OR Apache-2.0`. Third-party asset notices travel
with the crate that ships them.
