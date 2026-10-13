# Deferred Work

- source_spec: `docs/implementation-artifacts/spec-m3b-sparse-voxel-grid.md`
  summary: No test pins reachability's plane-in-reach-box rule (`inside(&rc, axis, plane)` in `src/bake/global.rs`).
  evidence: Making `inside` always true passes every test. A fixture needs a view volume narrower than the domain, which belongs with the M4 view-volume work.
- source_spec: `docs/implementation-artifacts/spec-m3b-sparse-voxel-grid.md`
  summary: The bake CLI's positionals 15-21 and the `OUT.tome.map.bin` layout have no in-repo test.
  evidence: The tools crate has no test harness. An off-by-one in an argument index or in the word order would only show in an external harness run (the evaluation harness asserts the magic, version and count).
- source_spec: `docs/implementation-artifacts/spec-m3b-sparse-voxel-grid.md`
  summary: the evaluation harness indexes `.samples.bin` by cell, and the file is now per global cell (L3 group) by default.
  evidence: Unverified (would be medium). Check whether `find_cells` returns global-cell ids or tile-cell ids; if it returns tile cells, the script's own-cell assertion fails on L3 bakes.
  resolved: 2026-10-12 (M4), not a bug. `find_cells` runs `cells.exe` → `query::find_cell`, which returns the global cell (`global_of`, words 4/5 of the tile cell), and `.samples.bin` is per global cell, so both sides are global. `assert_samples_find_their_cells` checks it on L3/L4 bakes.
- source_spec: `docs/implementation-artifacts/spec-m3b-sparse-voxel-grid.md`
  summary: A tile can exceed `max_cells_per_tile` when the merge heap empties with disconnected cells left, or in a non-Merged cell mode.
  evidence: The stop rule is the same as the M3 dense baker merge (`src/bake/reference.rs:801`), so it is pre-existing. The doc says the runtime crashes on too many cells; Umbra fails the bake above 2,047.
- source_spec: `docs/implementation-artifacts/spec-m3b-l4-midpoint-view-tree.md`
  summary: Cut the tile-portal, cell and object-run bytes so the parity tome reaches 1,114,672 B.
  evidence: L4 alone leaves about 1.81 MB. Tenebrae has tile portals of 771 KB, cells of 294 KB and object runs of 108 KB, against Umbra's 445 KB, 124 KB and 21 KB. The user split this from L4 on 2026-10-11.
  resolved: 2026-10-12, tome-size research handover (`plan-bench-gaps.md` item 7). The parity setup at defaults: 667,168 B, status 0 on 586/586, 0 truly visible objects culled, 423 kept per view (Unity rebake 419).
- source_spec: `docs/implementation-artifacts/spec-m3b-l4-midpoint-view-tree.md`
  summary: CLI positional 22 (`midpoint-views`) in `tools/src/bin/bake.rs` has no test.
  evidence: An off-by-one (`lever(21)` or a `>= 22` guard) would tie L4 to `global-groups` or ignore it, and the external research's "L4 off" runs would bake with L4 on. Same gap as positionals 15-21; fix together once the tools crate has a harness.
- source_spec: `docs/implementation-artifacts/spec-m3c-incremental-rebake.md`
  summary: No test isolates each cross-tile pass-B key term (`live[t]`, a neighbour's A key, a neighbour's liveness, the target box bits); each can be deleted from `Keys::b` with every test passing.
  evidence: Mutation survival from the M3c verification-gap review. No wrong-output scenario was found, because the test scenes change these terms together. A target moved within a tile into another cell with the same overlap count is the first scenario to try.
- source_spec: `docs/implementation-artifacts/spec-m3c-incremental-rebake.md`
  summary: The bake CLI's positional 23 `cache-file` (read, write, reuse across runs) has no automated test.
  evidence: The tools crate has no harness. A wrong index or a skipped write would silently turn every run into a full bake with correct output.
- source_spec: `docs/implementation-artifacts/spec-m3b-l4-midpoint-view-tree.md`
  summary: Under L4, emit each tile in whichever form is smaller (midpoint or explicit splits), so that L4 never costs bytes.
  evidence: With the explicit fallback, bakes where collapse dominates are larger with L4 on: 8 m tiles at collapse size 32, 2,316,624 B on against 2,081,248 B off; the unit scene 11,104 B against 10,320 B. The format allows a per-tile split count. Defaults never collapse, so parity is unaffected.
  update: 2026-10-12, the top-tree-pairing research answers (external) (reading 2b). The format's split list is a level-order prefix, implicit midpoints after it, and 412 stock view trees use exactly that (every stored value a midpoint). A collapsed tile's assembled tree is midpoint below the seam, since A's and B's trees were midpoint over their own boxes, so the fallback needs splits only up to the last non-midpoint node in level order (the seam, or a few seams after cascaded collapse) instead of one per node. That makes the fallback cost a handful of floats per collapsed tile, which likely retires this item without a two-form choice. Changes the bytes of collapsed L4 tiles only (the pinned `Params::default()` row of `l4_off_bakes_are_unchanged`, Stored::Fallback); the parity scene at defaults is unaffected. Alternative (2a), a dyadic guard on collapse, would be a rule of Tenebrae's own (Umbra has none).
  resolved: 2026-10-12 (user: "implement the prefix splits"). `global::trim_splits` stores the level-order prefix up to the last off-midpoint split. The uneven 48 m unit tile stores 1 split (the seam) instead of one per node. Unit scene at defaults: 9,872 B, against 10,320 B with L4 off (was 11,104 B). The parity scene, `8 0.25 merged0.95b6 flood` on (-160,-160,-96)..(160,192,96), collapse 32: 1,670,768 B with L4 on, against 2,081,248 B off (was 2,316,624 B); samples and map identical to the earlier L4-on bake; status 0 on 586/586, 0 wrongly culled, 353 kept per view. The parity scene at defaults is byte-identical (2,147,600 B), as it never collapses. L4 is now smaller than L4 off in every measured case, so no two-form choice is needed.
- source_spec: `docs/implementation-artifacts/spec-m3b-l4-midpoint-view-tree.md`
  summary: Check whether Umbra's top tree only ever pairs equal-width halves (stock tomes, external research); if so, match it so that collapsed unions are always midpoint-compatible.
  evidence: Tenebrae's `top_tree` splits odd tile counts unevenly (3 tiles become 1 + 2), and that is the only way uneven unions form. Matching Umbra would remove the L4 explicit fallback case and its size cost.
  answered: 2026-10-12, the top-tree-pairing research answers (external). Umbra also splits 3 tiles 1 + 2 (cubeness score, then tile balance) and its collapse has no dyadic guard, so matching it does not remove uneven unions.
  resolved: 2026-10-12, dropped. external research measured the rule on 26 stock tomes (the parity scene 264/264 and 269/269 splits rebuilt). Stock tomes have no uneven view-tree tiles only because the engine's CS equals T (32 m), so collapse never fires; Tenebrae's defaults never collapse either, so parity is unaffected. The fallback-size cost moves to the "whichever form is smaller" item above.
- source_spec: the top-tree-pairing research answers (external)
  summary: Optional fidelity lever: build the top tree with Umbra's rule (worst-child cubeness score, then tile-count balance, then the larger scene-box axis; planes must touch a tile face; empty child boxes become one filler leaf), for byte parity with stock top trees.
  evidence: The rule rebuilds 97-100 % of the splits of 26 stock tomes; residuals are equal-score ties that need the builder's scene box rather than the domain. It changes no culling and does not remove uneven unions; worth it only if top-tree bytes or shape parity become a goal.

## Deferred from: code review of spec-m4-bake-api.md (2026-10-12)

- **[resolved 2026-10-13, spine update after validation]** **Spine left stale in three places** (`ARCHITECTURE-SPINE.md`): AD-6 still says
  "`Target::user`" (now `Target::users`, line 95); AD-10's heading still reads "Public API is not
  yet a contract" over a body declaring contract v1 (line 131); Deferred "Visibility record
  ownership" says the README M4 row "stays as written", but M4 rewrote it (line 205).
- **Parity-mode progress order untested** (`src/bake/tests.rs` `progress_is_monotone_and_complete`):
  no progress test sets `SolidMode::Parity`, so the documented `Prepare, Cells, Global` sequence
  (no Flood/Join) is unchecked. The consumer presets are flood mode.
- **Instanced-parity soup copy in the memory estimate untested** (`src/bake.rs` `prepare`,
  `parity_bytes`): dropping the `copy` term fails no test. Test with a `memory_limit` between the
  estimates with and without it, expecting `Error::TooLarge`.
- source_spec: `bench/README.md` (benchmark, generator 1)
  summary: At 8 m tiles the runtime's portal query fails with status 2 (traversal budget) on Tenebrae's `city.t8` (150/200 views) and `terrain.t8` (166/200); Umbra's bakes of the same cases pass every view.
  evidence: Both tomes have over 1,000 global cells (1,764 and 1,131) but at most 7 cells per tile, so the documented per-tile limit is not it; Umbra's `terrain.t8` has 3,294 global cells and passes. 8 m is not an engine setting, but the consumer may expose the tile size. Reproduce with `bench export` and the external research on `city`.
- source_spec: `bench/README.md` (benchmark, generator 1)
  summary: At 32 m Tenebrae culls nothing on `rooms` (6,250 kept = frustum; Umbra 3,703) and little on `seams` and `thin_walls`, with 0 false culls; at 16 m it culls again (`rooms.t16` 3,748).
  evidence: `rooms.t32` has 18 tile cells over 16 tiles (at most 2 per tile), so the 6 m room walls are no cell boundaries. Unverified cause: the merge's fill test (`CellMode::Merged`, fill 0.95) or the L2 cell merge accepting unions across walls that are thin relative to a 32 m tall tile. parity is unaffected (its tiles hold far more geometry).
  resolved: 2026-10-12 (`plan-bench-gaps.md` item 1). The cause was lever L2 (`umbra_merge`), not the fill test: the best-first merge kept the wall, then L2's box growth joined one side's floor cell to the air above it, that to the air above the other side across the plane x = 16, and the other side was then inside the box. Its cost (solid in the box over the opening) never sees a 0.5 m wall in a 32 m tall tile. L2 now also refuses a union whose box wraps more than `r³` of other cells' open space (`l2_keeps_a_wall_in_a_tall_tile`). Engine evaluation: `rooms.t32` keeps 3,065 (was 6,250 = frustum; Umbra 3,703), `rooms.t64` 2,920, `seams.t32` 3,565, `maze.t32` 2,700, no new false culls on generated scenes or Sponza. parity: status 0 on 586/586, 0 truly visible culled, 376 kept per view (was 384), 2,497,472 B (was 2,147,600 B), 9,705 cells. Costs bytes on every case at 16 m and up; `bistro.t8` hole misses 68 to 92 (`bistro.t16` 84 to 73), tracked with the Bistro item. A looser cap `(2r)³` lost most of the culling (the parity scene 384, `rooms.t32` 4,739) for +5 % bytes.
- source_spec: `bench/README.md` (benchmark, generator 1, Bistro asset)
  summary: On `bistro` Tenebrae culls objects visible through openings of at least the smallest hole: 84 / 90 / 48 misses at 16 / 32 / 64 m against Umbra's 25 (68 against 190 at 8 m), and its tome is 2.5-3.5x Umbra's (1,001,520 B against 286,512 B at 32 m).
  evidence: The first benchmark errors under the hole truth (`bench/src/truth.rs`, a 0.125 m beam); every generated scene has none. Shared misses with Umbra (e.g. the eye at (8.39, 2.97, 2.39) inside the bistro) are centimetre-sized bottles and glasses, so part of the residual may be the beam model rather than either baker. Reproduce with `bench export DIR bistro` and the external research on `DIR bistro`, which prints the first misses per case.
  update: 2026-10-12 (`plan-bench-gaps.md` item 3 and research C). Misses: objects are now also listed in every cell whose box overlaps their grown box; through a hole 34 / 36 / 40 / 0 at 8 / 16 / 32 / 64 m (Umbra 190 / 25 / 25 / 25), none shared with Umbra; the rest are voxel closure (research B). Size remains open (1,168,832 B against 286,512 B at 32 m).

## Deferred from: tome-size research handover (2026-10-12)

- source_spec: `docs/implementation-artifacts/plan-bench-gaps.md` item 7
  summary: Sponza at 32 m keeps 10,637 objects against Umbra's 9,549 since the defaults moved to 8 m blocks (6 m blocks keep 9,611).
  evidence: Without merging, 8 m components already keep 10,434. Umbra's grid spans the scene bounds, so its 8 m block planes fall elsewhere relative to the galleries; untested. A block offset or a scene-bounds grid origin would test it.
- source_spec: `docs/implementation-artifacts/plan-bench-gaps.md` item 7
  summary: In open `seams` tiles Umbra keeps 2 to 4 cells where Tenebrae's mode 1 merges a tile's 64 blocks into one at cost 0.
  evidence: Every sampled segment between openings sees through the group (hidden 0 of about 160). Culling is close anyway (3,232 against 3,087), so the cause (a sampling detail, or Umbra's 16 m empty octants) is not pinned down.
- source_spec: `docs/implementation-artifacts/plan-bench-gaps.md` item 7
  summary: the parity scene keeps 423 objects per view at the new defaults, against 377 before (stock 343, Unity rebake 419).
  evidence: The defaults are Umbra's own parameters, so this is Umbra parity; `cell_merge` 3 m gives 394 at 893,072 B and 6 m blocks 393 at 872,672 B (0 truly visible culled in each). A preset choice for the consumer.
- source_spec: `docs/implementation-artifacts/plan-bench-gaps.md` item 7
  summary: Through the engine's native runtime, re-score the benchmark's exported tomes (with `--results`) and re-run the evaluation harness.
  evidence: The tomes now use coalesced portals (several per neighbour and plane), shared object windows and cells with no view leaf.
  resolved: 2026-10-12. The evaluation harness on a fresh `bench export`: 0 portal and 0 frustum mismatches over 16,000 views (40 cases, Tenebrae and Umbra tomes), so the in-repo visibility numbers are the engine's runtime's.
- source_spec: `docs/implementation-artifacts/spec-m6-native-runtime.md`
  summary: The native runtime's portal query is 2.1× the engine's native call on the stock parity tome (272 against 129 µs median over 400 captured cameras); the spec's acceptance is 1.5×. Accepted for V1 by the user (2026-10-13).
  evidence: Counters per query: 24 tiles, 154 cells reached, 136 visits, 1,654 portals considered (808 rejected by the exact back-face test), 445 rasterised over 4,400 pixel rows, 1,108 object candidates; the engine processes about 500 portals and 133 cells. Deferred cross-tile feedback changed no count, so there are no revisits left to remove; what remains is the rasterisation and portal-check volume. Untried: a cheaper portal test than projection for tiles entirely inside the frustum, a coarser coverage (32×32) for far tiles, and reusing the previous frame's selection and footprints under small camera motion.
- source_spec: `docs/implementation-artifacts/spec-m6-native-runtime.md`
  summary: The collision radius is matched to the engine by tuned knobs (portal rectangles grown by 1.75 radii, an eighth-pixel dilation, grown frustum and object boxes, box-based start cells), not derived.
  evidence: With a geometric `r` the runtime culled 8.6 % of what the engine keeps at the captured radii (median 0.29 m); the knobs bring it to 1.8-3.1 % with kept +3 to +7 %. What the radius adds has the same distance profile in both engines. A derivation would need the engine's own rule, which only the oracle knows.
- source_spec: `docs/implementation-artifacts/spec-m6-native-runtime.md`
  summary: Flip the facade's `tenebrae::query` camera queries to the native runtime and turn the `oracle` feature off by default (AD-13, step 2).
  evidence: Waits for the consumer's confirmation of what it links; `tenebrae-bench` and `tenebrae-tools` keep the oracle as the reference either way.
  resolved: 2026-10-13 (publication). `tenebrae-oracle` deleted; `tenebrae::query` re-exports `tenebrae-runtime`; the bench scores visibility with the runtime and its oracle comparison tests are gone. Validation against the engine's runtime now needs the external harness.
- source_spec: `docs/implementation-artifacts/spec-m6-native-runtime.md`
  summary: Gate portals, dynamic occluders and a configurable coverage resolution are not exercised or not implemented (plane-BSP leaves: lookups follow them since 2026-10-13, see the amendment).
  evidence: No tome in the workspace has gates or BSP leaves (the parity scene's tome A has none); a gate portal is opened over the whole cell face on its plane when its gates are open. Dynamic occluders and the 64×64 resolution are design choices of V1.
  amended: 2026-10-13 (`spec-fix-bsp-lookup-deep-keys.md`). Point lookups (`locate::find_cell`, `Locator`, the runtime's camera cell) now follow plane-BSP leaves to their local cell; 26,991 of 366,141 light-probe lookups on the installed game had come back empty. The baker still never emits a BSP, and `cell_regions` still skips BSP leaves.
- source_spec: none
  summary: Bring CI up to AD-15: `windows-latest` and `ubuntu-latest` both run fmt, clippy, tests and the benchmark; a separate MSRV 1.95 job; `cargo-semver-checks` on the published crates; workflow actions pinned by full commit SHA.
  evidence: Split from the crates.io release work (`spec-release-crates-io.md`) on 2026-10-13 by the user. Until it lands, the release reruns the gates `.github/workflows/ci.yml` has today (fmt on Ubuntu; clippy, test and bench on Windows).
- source_spec: `docs/implementation-artifacts/spec-release-crates-io.md`
  summary: Before the first publish, drop `Counters::stage_ns` from `tenebrae-runtime` and the clock that fills it (`traverse.rs` `Traversal::run` measures `clock.elapsed()`), and update `tenebrae-bench/examples/rtcount.rs`.
  evidence: AD-1 forbids clock access in library crates and durations in library counters; AD-10 allows dropping the field only before the first publish. Found 2026-10-13 while planning the release; it does not block the workflow, only the first `cargo publish`.
  resolved: 2026-10-13, in `spec-release-crates-io.md` (user brought it into scope): `stage_ns` and the runtime's clock removed, `rtcount` no longer prints stage times.
- source_spec: `docs/implementation-artifacts/spec-fix-bsp-lookup-deep-keys.md`
  summary: No committed stock-tome test detects a regression in the plane-BSP walk.
  evidence: `stock_tomes_answer_queries` compares `Locator` with `find_cell`, which share `locate::local_cell`, and its region-centre check skips BSP leaves; engine agreement (9,994,752 / 9,994,752 grid points on 723 tomes, 2026-10-13) comes from an uncommitted reference harness. A count of lattice points in BSP leaves that resolve, asserted non-zero under `TENEBRAE_TOMES`, would partly close it.
