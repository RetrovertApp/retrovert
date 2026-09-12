# Pinned Retrovert ABI headers

Canonical source: RetrovertApp/retrovert_api @ 5e3127b45fb5e5f204f27176511e0991630de846
Playback plugin ABI version: 2 (RV_PLAYBACK_PLUGIN_API_VERSION in retrovert/playback.h)

The harness fails a plugin build if any header the plugin vendors under
`include/retrovert/` differs from the copy pinned here (`HEADERS.sha256`).
A plugin need not vendor every header, but `retrovert/playback.h` and
`retrovert/rv_types.h` are mandatory. Re-pinning to a newer retrovert_api
revision is a harness version bump.
