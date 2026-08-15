# Channels

A channel is a self-contained TUF repository hosted on GitHub releases. Two
kinds of release make it up:

- `<channel>/channel-metadata` — one rolling release whose flat asset namespace
  *is* the channel's base URL. It carries the TUF metadata and the release-set
  manifest each generation is named by. `timestamp.json` is replaced on every
  publish and is the commit point; everything else is named by version or
  digest and never changes.
- `<channel>/vN` — one release per generation, carrying that release set's
  immutable assets: the manifest and, once the gather workflow lands, the
  plugin artifacts it lists.

## `dev`

Disposable test root, hosted in `RetrovertApp/playback_plugins`.

| | |
| --- | --- |
| Base URL | `https://github.com/RetrovertApp/playback_plugins/releases/download/dev/channel-metadata/` |
| Root | [`dev/root.json`](dev/root.json) |
| Root key id | `b56c9549951284ca51285a1ea866510dfa1d3251a8e4847c8a0118cb95cf8081` |

`dev/root.json` is public key material and the only thing a client needs to
trust beyond the base URL. Its private keys are disposable and are *not* the
production root: `stable` is born under the real root at Gate P and never
migrates roots.

Verify what the channel is serving right now:

```console
$ retrovert-publish verify \
    https://github.com/RetrovertApp/playback_plugins/releases/download/dev/channel-metadata/ \
    channels/dev/root.json
```

## Publishing

The signing keys live outside this repository, in the publisher's workspace
(`keys/` beside the `repository/` tree that gets uploaded). The workspace is the
channel's source of truth — the host is a mirror of it, and a publish that fails
part-way is recovered by publishing again, never by deleting what landed.

```console
$ retrovert-publish publish <workspace> <manifest> \
    --repo RetrovertApp/playback_plugins --channel dev
```

`GH_TOKEN` or `GITHUB_TOKEN` supplies the credential. `--stop-after N` ends a
run short of its commit point, which is how the failed-publish drill is run
against the real channel.
