# Retrovert

Host-side Rust for Retrovert: the playback engine, the plugin catalogue and the
desktop player, together with the update machinery that keeps a plugin set
current and the publisher that signs it.

| Crate | What it is |
| --- | --- |
| [`retrovert-player`](crates/player) | the headless playback engine |
| [`retrovert-plugin-catalog`](crates/catalog) | keeps the engine's decoder set current against an update channel |
| [`retrovert-player-desktop`](crates/desktop) | the desktop player binary, `retrovert`, following the `dev` channel |
| [`retrovert-tuf`](crates/tuf) | TUF metadata model and signing primitives, shared by publisher and client |
| [`retrovert-updater`](crates/updater) | resolves a signed channel into a verified installed generation |
| [`retrovert-publish`](crates/publish) | the publisher CLI that signs a channel, with each channel's trust anchor under [`channels/`](channels) |

Plugins live in [`playback_plugins`](https://github.com/RetrovertApp/playback_plugins),
which also hosts the release channels; the plugin ABI lives in
[`retrovert_api`](https://github.com/RetrovertApp/retrovert_api).

## Channels

| Channel | Host | Base URL | Anchor |
| --- | --- | --- | --- |
| `dev` | `RetrovertApp/playback_plugins` | `https://github.com/RetrovertApp/playback_plugins/releases/download/dev/channel-metadata/` | [`channels/dev/root.json`](channels/dev/root.json) |

Operating a channel — the `retrovert-publish` commands, the key layout, the
expiry schedule and the secrets the signing workflows need — is documented in
[`crates/publish/README.md`](crates/publish/README.md).

`retrovert-player` owns headless decode coordination. Renderers are not part of
this workspace yet: the player UI draws with flowi and lives with the host that
embeds it. `retrovert-plugin-catalog` owns its own `retrovert-updater` instance
and drives check → apply → activate → retain from one `tick()` on the audio
worker's loop; generations activate all-or-nothing at the mount/stop boundary
with a reload-previous fallback. `retrovert-player-desktop` wires the engine and
the catalog to a cpal output and a stdin command loop.

## Building

All dependencies resolve from a plain clone; no sibling checkouts are needed.
The desktop binary needs ALSA headers on Linux (`libasound2-dev`). From the
repository root, run:

```sh
./scripts/check.sh
```

## Licensing

Original code is MIT licensed; `retrovert-tuf`, `retrovert-updater` and
`retrovert-publish` are `MIT OR Apache-2.0`. Third-party asset notices travel
with the crate that ships them.
