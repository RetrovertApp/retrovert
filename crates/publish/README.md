# Retrovert channels

Publisher side of Retrovert's update channels: each channel's trust anchor
(`channels/<channel>/root.json`), the `retrovert-publish` CLI that signs it, and
the scheduled job that keeps its metadata from expiring. Clients use
[`retrovert-updater`](../updater); both sides share
[`retrovert-tuf`](../tuf), so signer and verifier agree on the metadata format.

## Channels

| Channel | Host | Base URL | Anchor |
| --- | --- | --- | --- |
| `dev` | `RetrovertApp/playback_plugins` | `https://github.com/RetrovertApp/playback_plugins/releases/download/dev/channel-metadata/` | [`channels/dev/root.json`](../../channels/dev/root.json) |

A channel is a TUF repository spread over two kinds of GitHub release on its
host:

| Release tag | Contents | Mutability |
| --- | --- | --- |
| `<channel>/channel-metadata` | TUF metadata and release-set manifests; its flat asset namespace is the base URL | `timestamp.json` replaced on every publish; everything else immutable |
| `<channel>/vN` | one release set: manifest and the artifacts it lists | immutable |

## Commands

Create a channel. `init` generates one root key into `keys/offline/` and the
three online keys into `keys/online/`, and signs the first, empty generation:

```console
$ retrovert-publish init <workspace>
$ retrovert-publish check <workspace>       # offline: refreshes the chain against its own root
```

Keep `keys/offline/root.pem` somewhere safe and offline. Whoever has it owns the
channel. It is never given to CI.

Publish a release set. Always pull first: the re-sign job also writes this
channel's version sequence, and publishing over stale versions would put
different bytes under names the channel already serves:

```console
$ retrovert-publish pull    <workspace> --repo <host> --channel <channel>
$ retrovert-publish publish <workspace> <manifest> --repo <host> --channel <channel>
```

`GH_TOKEN` supplies the credential. `--stop-after N` stops after `N` uploads,
for drilling a failed publish. A publish that fails part-way is recovered by
publishing again, never by deleting what landed.

Re-sign without publishing anything new:

```console
$ retrovert-publish pull   <workspace> --repo <host> --channel <channel>
$ retrovert-publish resign <workspace> --repo <host> --channel <channel>
```

Read a channel as a client does:

```console
$ retrovert-publish verify <base-url> <channel>/root.json
```

Optional: a root shared between several holders. `keygen` on each holder's
machine, then `init --root-key <pub>... --root-threshold N --root-sign-with
<pem>...`. Not used by any channel today.

## Workflows

| Workflow | Trigger | Does |
| --- | --- | --- |
| [`publish.yml`](../../.github/workflows/publish.yml) | dispatched by the host's gather | pulls the live chain, publishes the metadata with `timestamp.json` last, reads it back |
| [`resign-dev.yml`](../../.github/workflows/resign-dev.yml) | 1st of each month, or dispatch | re-signs every online role and reads the channel back |
| [`ci.yml`](../../.github/workflows/ci.yml) | push, PR | builds and tests the signer |

Both signing workflows build `retrovert-publish` from this repository at the
running revision with `--locked`, and share the concurrency group
`sign-<channel>`.

## Secrets

The `channel-signing` environment holds:

| Secret | What it is |
| --- | --- |
| `CHANNEL_TARGETS_KEY` | `targets` private key, PKCS#8 PEM |
| `CHANNEL_SNAPSHOT_KEY` | `snapshot` private key, PKCS#8 PEM |
| `CHANNEL_TIMESTAMP_KEY` | `timestamp` private key, PKCS#8 PEM |
| `CHANNEL_TOKEN` | write access to the channel's host repository |

These are the three PEMs `init` wrote to `keys/online/`. An unset secret arrives
as an empty string; the assemble step fails on that before touching the channel.

## Expiry

| Role | Lifetime after signing | Renewed by |
| --- | --- | --- |
| `timestamp` | 90 days | `resign-dev.yml` |
| `snapshot`, `targets` | 1 year | `resign-dev.yml` |
| `root` | 10 years | the root key holder; not implemented in the signer yet |

A lapsed channel cannot self-recover: `pull` refuses expired metadata, so once
`timestamp` is 90 days old the job fails and the publisher re-signs from a
workspace of their own. GitHub also disables scheduled workflows after 60 days
without a commit to this repository.
