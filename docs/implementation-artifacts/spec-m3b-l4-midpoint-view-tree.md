---
title: 'M3b L4 — midpoint view-tree encoding'
type: 'feature'
created: '2026-10-11'
status: 'done'
baseline_commit: '9f8c4b86f39cf3aa4560c21ed834e3c3a9a56849'
route: 'dispatch'
review_loop_iteration: 1
context:
  - '{project-root}/docs/implementation-artifacts/spec-m3b-sparse-voxel-grid.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** At default `Params` the parity tome is 3,765,856 B against Umbra's 1,114,672 B. Of that, 1,954,472 B is view-tree split floats: Tenebrae stores a 4-byte f32 for every view-tree node, 488,618 in all. Umbra's view trees store none; every one of their splits is an implicit midpoint (stock tome and Unity rebake: 0 view splits, max depth 23).

**Approach:** Add bake lever L4, `Params::midpoint_views: bool`. It is switchable like L1–L3, on by default and off in `Params::levers_off()`. With L4 on, every view-tree split is an implicit midpoint and each tile stores no split floats, as Umbra does.

## Boundaries & Constraints

**Always:**
- Library rules carry over from the M3b spec: no dependencies, no `unsafe`, deterministic output, and 1 thread spawns nothing.
- Conservative leaves. A leaf may only be uniform over the voxels it actually covers, with SOLID as don't-care. No leaf may assign an open voxel to SOLID or to the wrong cell.
- For every implicit split, the baker's split plane must equal the runtime's f32 `(min+max)*0.5` over the node's float box, starting from the tile AABB. Assert this in debug builds.
- With `levers_off()`, output is byte-identical to the pre-L4 levers-off bake, so the 0.25 m gate against the M3 dense baker still holds. With L1–L3 on and L4 off, output is byte-identical to the M3b baker's default bake.
- CLI positionals are append-only, and the default equals `Params::default()` (AD-12).

- **Decision (user, 2026-10-11): pure midpoint.** Every view-tree split is implicit, and `splits` is empty for view trees. The axis is still chosen per node. There is no depth-D explicit prefix.
- **Decision (user, 2026-10-11): L4 only.** The acceptance target is a saving of at least 1.5 MB with status 0 and 0 wrongly culled. The 1,114,672 B target stays open on the M3b spec. Portal, cell and object-run cuts are a separate follow-up in `deferred-work.md`.

- **Decision (user, 2026-10-12): explicit fallback for collapsed tiles.** When collapse leaves a tile whose view tree is not a midpoint tree (a union whose shared plane is not its midpoint), that tile stores its splits explicitly, one per node, as with L4 off. It is never re-encoded in midpoint form, so it is bounded and exactly as conservative as the assembled tree. Every other tile stays pure midpoint. Collapse itself is unchanged. Priorities: correct culling first, then Umbra parity.

**Never:**
- No change to cells, portals, merging, reachability, collapse or the top tree. L4 touches only how view trees are split and how they are emitted.
- No change to the tome reader/writer format semantics.
- No explicit view-tree splits under L4, except as the 2026-10-12 decision below allows.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Levers off | `levers_off()` bake | bytes == pre-L4 levers-off bake | N/A |
| L4 off | defaults with L4 off | bytes == the M3b baker default bake | N/A |
| Threads | L4 on, threads 1 vs 4 | byte-identical | N/A |
| Non-dyadic box | a tile or L1 region whose voxel span is not a power of two | conservative leaves, no open voxel misassigned; tome valid | N/A |
| Round trip | L4 tome | write∘read byte-identical; every leaf value is a tile cell or the solid value | N/A |

</frozen-after-approval>

## Code Map

- `src/bake/tile.rs:1530` `view_tree` -- breadth-first over integer voxel ranges. A region is a leaf when `region_uniform` (`:1417`) finds one cell, with SOLID as don't-care.
- `src/bake/tile.rs:1459` `choose_split` -- splits on voxel planes at cell-extent faces, scored by (cells cut, imbalance), with a midpoint fallback. L4 keeps choosing the axis, since the 2-bit code is still explicit, but forces the midpoint plane.
- `src/bake/tile.rs:1325` `view_of` -- solid ownership near open space. Do not change.
- `src/bake/global.rs:763` `collapse_leaves` (called at `:951`, `:1122`) -- re-orders the tree level by level. Compute the prefix after it.
- `src/bake/global.rs:364` `emit_tree` (called at `:651`) -- today it pushes a split for every node and `0.0` for leaves (`:376`), so `splits.len() == codes.len()`. Under L4 it emits none.
- `src/tome/query.rs:26-31,55-60` -- runtime-equivalent descent. **The prefix indexes nodes, not inner nodes**: leaves inside the prefix use a slot too. The midpoint is f32 `(min+max)*0.5` on the float box from the tile AABB, and a point goes above when `s <= p`. Use this for the equality assert.
- `src/tome/write.rs:162-169` -- the split count at tile header +36 and the offset at +40, both 0 when there are no splits. Format is unchanged.
- `src/bake.rs` `Params` / `levers_off()` -- add the L4 field next to `empty_leaves`, `cell_merge` and `global_groups`.
- `src/bake/reference.rs` -- the M3 dense baker, kept as a test oracle. Do not change.
- `tools/src/bin/bake.rs` -- append positional 22 for L4 and update the `//!` header.
- the evaluation harness -- passes extra positionals through. To evaluate: `cargo build --release -p tenebrae-tools`, then `TENEBRAE_NAME=am_L4 <harness> 32 0.125 merged0.95b6 flood <parity box> [15..22]`. It reports status pairs (the evaluation harness) and wrongly culled (the evaluation harness).

## Tasks & Acceptance

**Execution:**
- [x] `src/bake.rs` -- add the L4 `Params` field (default on, off in `levers_off()`), its validation and its docs.
- [x] `src/bake/tile.rs` -- under L4, `view_tree` splits at the f32 float-box midpoint of the chosen axis. Leaves stay conservative, and the plane-equality assert is added. Where a box's voxel span is odd or not a power of two, the midpoint falls inside a voxel. Then uniformity is tested over every voxel the box overlaps, even partly. Recursion ends at the latest when a box overlaps a single voxel per axis.
- [x] `src/bake/global.rs` -- under L4, `emit_tree` emits no splits for view trees, so the split count and offset are 0. This happens after `collapse_leaves`.
- [x] `src/bake/tests.rs` -- one test per matrix row, plus a test that a point query through `tome::query` finds the same cell as the voxel's `view_of` for sampled open voxels.
- [x] `tools/src/bin/bake.rs` -- positional 22 and the header.
- [x] Spec Implementation Notes -- the parity bake at defaults: tiles, nodes, view-tree bytes, tome bytes, engine status, wrongly culled, objects per view, 1 vs 16 thread identity, memory.

**Acceptance Criteria:**
- Given the parity setup and default `Params`, when it is baked, then external research reports status 0 on all 586 queries and 0 wrongly culled, and the tome is smaller than 3,765,856 B by at least 1.5 MB.
- Given default `Params`, when it is baked at threads 1 and 16, then tome, samples and object map are identical.

## Implementation Notes

Code: `Params::midpoint_views` (`src/bake.rs`); `midpoint_view_tree`, `midpoint_axis`,
`region_cells`, `overlapped`, `split_node`, `tile_frame`, `midpoint`, `is_midpoint_tree` and
`midpoint_retree` (`src/bake/tile.rs`); `emit_tree` / `emit` / `collapse_node`
(`src/bake/global.rs`); CLI positional 22 `midpoint-views` (`tools/src/bin/bake.rs`); usage text of
the evaluation harness scripts (both pass positionals
through unchanged).

**How it works.** Each node carries a float box starting from the tile's box (the same
`origin + range * tile` the tome stores). The box's voxels are those it overlaps with positive length
on every axis (`overlapped`, a binary search over `Grid::coord`), partly covered ones included; it is
a leaf when `region_uniform` finds one cell over them (SOLID don't-care). Otherwise it splits at f32
`(min + max) * 0.5` on an axis chosen per node. A non-leaf overlaps two or more voxels on the chosen
axis, so a voxel plane lies strictly inside the box, and then the f32 midpoint does too (asserted):
every split shrinks the box and the build ends. Emission writes no split floats (tile words +36/+40
stay 0). `emit` debug-asserts `is_midpoint_tree` (bitwise equality with the runtime's descent from
the tile AABB) on every tile.

**Axis choice (deviation from the Code Map's "keep `choose_split`'s axis").** Keeping
`choose_split`'s axis gave 1,101,866 view nodes and 2,284,176 B, a saving of 1,481,680 B: short of the
1.5 MB AC. `midpoint_axis` instead tries each axis spanning two or more voxels at its midpoint and
takes the one leaving the fewest mixed children, then the fewest cells over both children, then
`choose_split`'s axis, then the lowest axis. That gives 925,812 nodes and 2,147,600 B. The intent
("the axis is still chosen per node") is unchanged.

**Collapse.** A collapsed tile's tree splits at the shared plane, which is the union's midpoint only
for equal halves. When the assembled tree is not a midpoint tree, `midpoint_retree` re-encodes it over
the union's box: a midpoint box is a leaf when the assembled tree's leaves it overlaps hold at most one
cell (solid don't-care), else it splits on the axis of the first assembled split straddling it. This is
exactly as conservative as the assembled tree. Collapse itself (gates, seam merge, cells, portals) is
unchanged. At default `Params` collapse is a no-op (32 m tiles, CS 32), so this path never runs there.

**Parity setup at default `Params`** (the parity scene, domain and VV
(-192,-160,-672)..(160,192,608), 0.125 m, 32 m tiles, L1-L4 on, 16 threads):

| bake | tiles | cells | global cells | view nodes | view splits | max view depth | tome | engine status | wrongly culled | objects per view |
|---|---|---|---|---|---|---|---|---|---|---|
| the M3b baker default (= L4 off) | 344 | 8,367 | 838 | 488,618 | 488,618 | 124 | 3,765,856 B | 0 x586 | 0 | 384 |
| L4, `choose_split` axis (rejected) | 344 | 8,367 | 838 | 1,101,866 | 0 | n/m | 2,284,176 B | 0 x586 | 0 | 384 |
| **L4 default** | 344 | 8,367 | 838 | **925,812** | **0** | **24** | **2,147,600 B** | **0 x586** | **0** | **384** |

Saving: **1,618,256 B** (43 %), above the 1.5 MB AC in either reading (1,500,000 / 1,572,864). Cells,
portals, global cells, samples and object map are unchanged; the whole saving is view trees. View-tree
bytes: the 488,618 split floats were 1,954,472 B; the midpoint trees have 1.89x the nodes (463,078
leaf values vs 244,481), which costs back ~336 KB in node codes and leaf values. Umbra: 1,114,672 B,
max view depth 23; the remaining gap (cells ~0.29 MB, tile portals ~0.77 MB, object runs ~0.11 MB) is
the deferred portal/cell/object-run follow-up.

**Identity and memory** (the evaluation harness, Q4 method, baseline at `scene loaded` = WS 429 /
private 556 MB):

| check | result |
|---|---|
| 1 vs 16 threads, default `Params` | tome, samples and object map byte-identical (also identical to the evaluated 16-thread bake) |
| L4 off (`... 1 6 1 0`) vs the M3b baker default | tome, samples, object map byte-identical |
| levers off (`... 0 0 0 0`), 0.25 m small box (-160,-160,-96)..(160,192,96) vs the M3b baker levers off | tome, samples, object map byte-identical (3,862,720 B; the M3b baker levers off is identical to the M3 dense baker) |
| memory, 16 threads | +216 / +234 MB WS / private, 65 s (pre-L4: +194 / +212 MB, 72 s) |
| memory, 1 thread | +118 / +135 MB WS / private, 578 s (pre-L4: +109 / +125 MB, 587 s) |

Both memory rows stay far inside the Umbra Q4 rows (+322/+334 MB 1 thread, +1,577/+1,723 MB 16 threads).

**Tests** (`src/bake/tests.rs`): `l4_off_bakes_are_unchanged` pins the M3b baker bytes (FNV-1a and
length, taken from a M3b baker build) of the multi-tile scene for default-with-L4-off and for
`levers_off()`, and checks those trees still store a split per node; `l4_is_thread_independent_and_round_trips`
(threads 1 vs 4, round trip, leaf values, no stored splits, cells equal to the L4-off bake);
`point_queries_find_each_open_voxels_cell` (for L4 on/off x levers on/off, every sampled open voxel's
centre and eight near-corner points, +-0.45 voxel, find the voxel's own global cell through
`query::find_cell`); `l4_non_dyadic_tiles_stay_conservative` (6 m tiles of 12 voxels);
`l4_collapse_of_unequal_tiles_is_re_encoded` (three 16 m tiles collapse into a 48 m tile whose shared
plane, x = 16, is not its midpoint, x = 24; asserted on the L4-off tree). The last two also check every
open voxel exhaustively: its centre and near-corner points resolve through `query::find_cell` in the
L4 tome to the cell the voxel-exact L4-off tome gives. `l4_off_bakes_are_unchanged` also pins two L4-on
bakes (default, 1,188,784 B, and collapse size 8, 40,000 B; replacing `midpoint_axis` by
`choose_split`'s axis fails the second) and asserts L4 shrinks the uncollapsed tome. The levers-off dense-reference tests still pass. The debug build runs the
midpoint-equality assert in the L4 tests.

**Risk: non-dyadic collapsed tiles.** A collapsed union whose cell planes sit at non-dyadic fractions
of its box (e.g. a 3-tile union, planes at 1/3) cannot be reached by midpoints, so each such plane needs
a ~24-level float-bisection chain, and chains multiply where planes on two or three axes meet. On the
test scene at 6 m tiles of 12 voxels with collapse size 18 the largest tile reaches 435,511 nodes; with
collapse size 32 the bake did not finish within minutes. Defaults never reach this (collapse is a
no-op there, and L1 region tiles are single leaves), and plain non-dyadic tiles without collapse stay
small (at most a few hundred nodes per tile in the tests). A fix needs a decision beyond this spec:
for example, under L4, collapse only into unions whose shared plane is the midpoint, or a node budget
that refuses the bake.

**Explicit fallback for uneven collapsed tiles (user decision 2026-10-12).** `midpoint_retree` and
the re-encode in `collapse_node` are removed. `collapse_node` now marks a collapsed tile
`GTile::explicit` when, under L4, its assembled tree is not a midpoint tree over the union's box
(`is_midpoint_tree`); `emit` writes such a tile's splits explicitly, one per node, exactly as with L4
off, and every other tile split-free (the debug plane-equality assert still covers those). Collapse
itself is untouched: cells, portals, merges and the assembled tree are the same as before. The tile
cache holds tile-stage results only, so `cache::VERSION` stays 1 (the cache pin test passes).

| bake | tome | time (16 threads) | engine |
|---|---|---|---|
| parity setup, default `Params` | 2,147,600 B, tome, samples and map byte-identical to the pre-M3c baker build | 71 s | status 0 x586, 0 wrongly culled, 384 per view |
| uneven collapse via the CLI: `8 0.25 merged0.95b6 flood` on (-160,-160,-96)..(160,192,96), default collapse size 32 (8 m tiles collapse into 16 m and 24 m unions), L4 on | 2,316,624 B (425 tiles, 8,836 cells) | 14.2 s | status 0 x586, 0 wrongly culled, 353 per view |
| same, L4 off (`... 1 6 1 0`) | 2,081,248 B | 14.9 s | status 0 x586, 0 wrongly culled, 353 per view |

Bounded now (before the fallback the same kind of bake ran unbounded). Where uneven unions dominate,
L4 costs more than it saves (+11 % here): the split-free trees of the even tiles have more nodes than
their explicit trees would, and the uneven tiles save nothing. On the unit-test scene (8 m tiles,
collapse 32) the L4-on tome is 11,104 B against 10,320 B off (was 1,188,784 B with the re-encode).

Tests: `l4_collapse_of_unequal_tiles_is_re_encoded` is now `l4_uneven_collapsed_tiles_keep_explicit_splits`:
the 48 m union stores one split per node and its tome equals the L4-off tome; on the multi-tile scene
only tiles whose extent is not a power-of-two number of 8 m tiles store splits, the others none, with
sampled point queries and the exhaustive open-voxel check against L4 off. `l4_off_bakes_are_unchanged`:
the default L4-on pin is 11,104 B (regenerated); the collapse-size-8 pin (40,000 B, catches a
`midpoint_axis` regression) is unchanged; "L4 shrinks the tome" is asserted without collapse only.
`l4_non_dyadic_tiles_stay_conservative` runs with the default collapse size again (6 m tiles collapse
into uneven unions) and passes in well under a second in release. The thread test accepts explicit
tiles (`assert_l4_view_splits`: no split, or one per node, and at least one split-free tile).

## Spec Change Log

- 2026-10-12, triage row 1 (intent_gap, high): under L4, `midpoint_retree` grew collapsed tiles of uneven width without bound (8 m-tile test scene: 1,188,784 B against 10,320 B with L4 off; some collapse sizes never finished). Amended: frozen Always gains the user's explicit-fallback decision, and the Never line now carves that exception out. Known-bad state avoided: unbounded re-encoding of non-dyadic unions. KEEP: `midpoint_view_tree`, `midpoint_axis`, `overlapped`, `is_midpoint_tree`, emission without splits for midpoint tiles, the five L4 tests and their review patches (pins, exhaustive voxel checks), the parity numbers at defaults (collapse never runs there).

## Review Triage Log

| # | Source | Finding | Verdict | Route | Evidence |
|---|---|---|---|---|---|
| 1 | blind, edge-case, verification-gap | L4 + collapse into a union with non-dyadic cell planes: `midpoint_retree` grows without bound (435,511 nodes measured; CS 32 at 6 m tiles never finished); no node cap, bake hangs | high | intent_gap | Reachable through public `Params` (e.g. 8 m tiles, CS 24: a 1+2 tile union). Every fix either changes collapse (frozen Never) or refuses the bake (new error behaviour); the frozen intent does not settle it. Defaults unaffected (32 m tiles, CS 32, no collapse). Measured after the patches: the 8 m-tile test scene at default CS 32 bakes 1,188,784 B with L4 on against 10,320 B off. **Resolved 2026-10-12 by the user's decision (explicit fallback)** (options: refuse such bakes with an error / skip non-midpoint collapses under L4 / turn L4 off when collapse unions are non-dyadic). |
| 2 | edge-case | `cell_pass` memory estimate omits the view-tree nodes | low | grouped with 1 | Pre-existing for L4-off trees; material only in the row-1 blowup. |
| 3 | verification-gap | No test pins the L4-on size or that it is smaller than L4-off; a `midpoint_axis` regression would pass every test | medium | patch | Confirmed: the five L4 tests check validity, conservativeness and identity only. |
| 4 | verification-gap | Partial-voxel conservativeness is probed only at up to 64 sampled voxels per cell | medium | patch | `CELL_SAMPLES = 64`, strided sampling (`tile.rs:1160-1169`); an off-by-one in `overlapped` can slip past. |
| 5 | blind | Collapse test does not show the re-encode ran (release builds skip the debug assert) | low | patch | Assertions hold whether or not the re-encode ran, except via the sampled point queries. |
| 6 | blind | README M3b lever list omits L4 | low | patch | The row's parenthetical lists L1-L3 only. |
| 7 | blind | Pinned FNV hashes give no regeneration recipe | low | patch | Constants lack provenance. |
| 8 | verification-gap | CLI positional 22 untested | low | defer | Same gap as positionals 15-21 (existing deferred entry); the tools crate has no harness. |
| 9 | blind, edge-case | Plane-equality check is debug-only | false | reject | The frozen intent says "Assert this in debug builds". |
| 10 | blind, edge-case | `split_node` `assert!` / `unreachable!` panic in the library | false | reject | A non-leaf overlaps two or more voxels on the axis, so a float plane lies strictly inside and the f32 midpoint does too; the overflow case needs coordinates near 1.7e38. No reachable input shown. |
| 11 | blind, edge-case | Task ticks "its validation" but nothing is validated | low | reject | The fix is a spec edit, or it depends on the row-1 decision. |
| 12 | blind | Tile frame and voxel planes may differ (tile only a multiple of voxel within 1e-3) | maybe-false | reject | A box lying only in the sliver needs a split inside it; bisection only chases planes between voxels in the tile. Would be low at most. |
| 13 | blind | No direct unit tests for `is_midpoint_tree` / `midpoint_retree` | low | reject | Covered by the row-4/5 patches and the debug assert; adds tests only. |
| 14 | blind | Spec Change Log empty, risk not in deferred work | low | reject | The fix edits this spec; the risk is now row 1. |
| 15 | blind | `midpoint_axis` scans more than needed | low | reject | Bake time fell (72 s to 65 s); no named harm. |
| 16 | blind, edge-case, verification-gap (loop 1) | Mixed-tiles test judges "uneven" by width and checks one direction; no-collapse pin row relaxed; non-dyadic test lost its storage check; tile size hardcoded | medium | patch | Explicit is set by `!is_midpoint_tree`, not by width; the deleted `assert_no_view_splits` left nothing checking storage there. |
| 17 | blind (loop 1) | Where collapse dominates, L4 is larger than L4 off (2,316,624 B against 2,081,248 B; unit scene 11,104 B against 10,320 B) | medium | defer | Defaults never collapse. A per-tile choice of the smaller form is a format-legal follow-up. |
| 18 | blind, edge-case (loop 1) | Spec Intent / task text and the decision's "below" pointer and fallback condition are out of date | low | reject | The fix edits this spec; the recorded decision governs. |
| 19 | blind (loop 1) | No counter of explicit tiles; whole-vector tome asserts give no diagnostics | low | reject | No named harm. |
| 20 | blind (loop 1) | Cache `VERSION` claim not backed by a cached re-bake with uneven collapse | false | reject | The M3c cache tests use the 8 m scene at default collapse size, which produces explicit tiles (see the mixed-tiles test), and match a fresh bake. |

## Verification

**Commands:**
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` -- expected: clean.
- `cargo test --release --workspace` -- expected: all pass.
- `cargo build --release -p tenebrae-tools`, then the parity evaluation above -- expected: the ACs.
