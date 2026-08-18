# retrovert-updater

Client-side update acquisition for Retrovert: fetching release metadata,
authenticating it, and handing over a verified generation. Consumers are the
Retrovert player and Replay's database updater; the machinery is generic over
what is being updated, and playback policy lives above it.

| Crate | What it is |
| --- | --- |
| [`retrovert-tuf`](crates/retrovert-tuf) | TUF metadata model and signing primitives, shared with the publisher |
| `retrovert-updater` | the `Updater` facade — not yet extracted, see issues #2 and #6 |

Nothing here signs a channel or holds a key. The publisher side — the
`retrovert-publish` CLI, the channel trust anchors, and the scheduled re-signing
jobs — lives in
[`retrovert-channels`](https://github.com/RetrovertApp/retrovert-channels),
which depends on `retrovert-tuf` from here so that signer and verifier agree on
the metadata format.
