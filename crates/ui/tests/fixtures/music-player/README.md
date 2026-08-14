# Music-player render oracles

`ui_tree/*.json` records the seven views' render structure. Pixel baselines under
`pixels/` are exact RGB captures owned by this crate.

Inspect renderer changes before blessing pixel baselines from the repository root:

```sh
REPLAY_BLESS_PIXEL_ORACLES=1 cargo test -p retrovert-player-ui -- --ignored --test-threads=1
```

Captures must stay inside a layout scope and pass an ink guard. The software
backend is authoritative only for rectangles, horizontal and vertical lines,
and image subregions. Rust ports preserve the four asymmetric padding sites
with `Layout::configure` and an explicit `LayoutPadding`: the header blocks in
`sid_view`, `v2m_view`, `vgm_view`, and `general_view`.

The two full-view captures also require Replay's CMake-built OpenMPT decoder via
`BUILD_DIR`. The remaining five pixel fixtures run with Flowi's embedded backend.

The UI-tree fixtures are the frozen C parity contract. The Rust views consume only
`VizSnapshot`, which does not carry the legacy header strings used to create those trees,
so module tests check their structural and palette contracts while
`ui_tree_contract::all_seven_ui_tree_fixtures_are_byte_exact` locks every complete fixture.
Run the gate with:

```sh
cargo test -p retrovert-player-ui ui_tree
```

If the shared UI contract intentionally changes, inspect all seven tree diffs, replace the
fixtures here, and update the seven expected SHA-256 digests in `src/lib.rs`. Replay does not
own or bless copies.
