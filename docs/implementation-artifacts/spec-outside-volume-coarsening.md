---
title: 'Distance coarsening of tiles outside the view volumes'
type: 'feature'
created: '2026-10-14'
status: 'done'
baseline_commit: '87b4f956dd8a6cc8b7700120b6f4215f67f1e334'
route: 'dispatch'
review_loop_iteration: 0
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** With `Params::domain_margin` > 0 every domain tile is voxelised at the full `voxel` (0.125 m), although the camera never enters tiles outside the view volume; on a large bubble that costs most of the bake (a margin-1000 bake is barely faster than baking the whole scene box as view volume). The existing outside-volume rule (`tile::level_shift`) also takes its shift from the volume's thinnest extent, which is wrong: the shift should grow with the tile's distance from the nearest view volume.

**Approach:** Give every tile a level shift `k` from its distance `d` to the nearest view volume and bake a tile with `k > 0` on a coarse voxel grid (`voxel · 2^k`) with feature `cell_merge · 2^k`, presenting its results to the rest of the bake at the fine resolution so seams, the flood join, the global stage and the cache stay unchanged. Leaf tiles outside the volumes may collapse up to `tile · 2^k`. Several view volumes are accepted.

Rule (`SO` = `cell_merge`, `SH` = `voxel`, `T` = `tile`):
- `pad = max(1.6·SO, 2·SH)`. A tile overlapping any view volume grown by `pad` has `k = 0`.
- Otherwise `d` = least Euclidean box-to-box distance (per-axis gap; 0 on overlap) from the tile's box to a view volume; `q = d / (4·SO) − 1`; `k = 1 + floor(log2 q)` when `q ≥ 1`, else `k = 1`.
- `k` is capped so that `T / (SH · 2^k)` stays a whole number of voxels ≥ 1. `k = 0` everywhere when `cell_merge` is 0 or `Params::coarsen_outside` is off.
- At SO 6, SH 0.125, T 32: d < 72 m → k 1 (0.25 m voxels), < 120 → 2, < 216 → 3, < 408 → 4, < 792 → 5.

## Boundaries & Constraints

**Always:** bakes with every tile at `k = 0` (margin 0, or all tiles within `pad`) stay byte-identical to today, so the pinned hashes, the dense-reference tests and the benchmark do not move; bakes stay byte-identical across thread counts and with or without a tile cache; the cache key covers `k`; library crates stay pure.

**Never:** no per-volume SO/SH parameters; no change to the runtime or tome format; no change to tiles inside the volumes; no top-tree grid tiles larger than `T` (leaf growth only through collapse).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Margin 0 | domain = volume snapped to tiles | every tile `k = 0`, bytes unchanged | N/A |
| Margin > 0 | tiles 10–800 m from the volume | tiles baked at `0.125·2^k`, fewer voxels and cells; seams to fine tiles keep portals and the flood | N/A |
| Seed in a coarse tile | seed point outside the volume | seeds its coarse voxel | N/A |
| Fine voxel count not divisible | `T / SH` = 6 | `k` capped at 1 (`6 / 2 = 3`) | N/A |
| Several volumes | two disjoint boxes | domain covers both; `k` from the nearer; view mask and reachability take their union | empty or invalid volume → `Error::Params` |

**Decisions:**
- Several view volumes enter as a new `Params::extra_view_volumes: Vec<Aabb>` (default empty) beside `Input::view_volume`; the view volumes are `view_volume` plus these. Not breaking under contract v2 (new `Params` fields are not).
- The full spec is kept above the token target (one cohesive feature).

</frozen-after-approval>

## Code Map

- `crates/tenebrae-bake/src/tile.rs` -- `level_shift` (wrong rule, replace), `Ctx::{level_shift, feature, block_of, lo}`; per-tile passes read `ctx.grid` / `ctx.layout` / `ctx.lo(t)` (`voxelize_tile`, `label`, `starts`, `flood_pass`, `flood_solid`, `parity_solid`, `cell_pass`, `faces`, `samples`, `target_range`, `objects`, `view_range`, `mask_view`, view trees). Grid helpers (`tri_ranges`, `may_touch`, `touched_bricks`) already take a `&Grid` and work on any voxel size.
- `crates/tenebrae-bake/src/grid.rs` -- `Grid`, `Bricks`, `Layer` (run-length face values, positions row-major over `size²`), `face_layer`.
- `crates/tenebrae-bake/src/global.rs` -- consumes `TileOut` in fine voxel units: `assemble` zips neighbour layers, `empty_leaves` compares `cells[0].voxels` with the full fine count, `reachability` reads `input.view_volume`, `collapse_node` limits union extent by `size`, `emit` writes `tile_level(p.voxel, …)`.
- `crates/tenebrae-bake/src/cache.rs` -- tile key already hashes `ctx.level_shift(t)` (line ~348); decoders validate against the fine layout.
- `crates/tenebrae-bake/src/lib.rs` -- `Params` (docs of `domain_margin`), `validate`, `prepare` (domain from volume + margin, brick shift, touched-brick estimate per tile).
- `crates/tenebrae-bake/src/tests.rs` -- `domain_margin_takes_in_targets` (~line 2987) is the template for margin tests.

## Tasks & Acceptance

**Execution:**
- [x] `crates/tenebrae-bake/src/tile.rs` -- replace `level_shift` with the distance rule over all volumes; add a per-tile space (coarse `Grid`, `Bricks`, `lo`, scale `s = 2^k`) and run every tile pass in it; scale `CellOut` boxes, `Face` planes/rects and open-voxel counts (`s³`) to fine units; upsample output and flood layers to the fine face; a coarse tile's flood reads a fine neighbour's layer as "any of its `s²` positions"; seed block stays the same voxel count (capped at the tile) -- seams, join, global stage and cache see fine tiles.
- [x] `crates/tenebrae-bake/src/grid.rs` -- `Layer` upsampling by an integer factor.
- [x] `crates/tenebrae-bake/src/lib.rs` -- `Params::coarsen_outside` (default true) and `Params::extra_view_volumes` (validated like `view_volume`); domain over all volumes; brick layout and memory estimate per tile at its own resolution; docs.
- [x] `crates/tenebrae-bake/src/global.rs` -- collapse limit `max(size, T·2^min(ka, kb))` for leaf pairs both outside the volumes; `tile_level` with the tile's own voxel; reachability over all volumes.
- [x] `crates/tenebrae-bake/src/tests.rs` -- unit tests for the `k` table, layer upsampling, margin bake determinism (threads 1 vs 4, cached vs fresh), coarse tiles present, every in-volume seed cell reachable, and margin-0 unchanged.
- [x] `README.md`, `crates/tenebrae-tools/src/bin/bake.rs` -- document the rule; tools flag for `coarsen_outside`.

**Acceptance Criteria:**
- Given the existing suite, when `cargo test --release` and `cargo test --release -p tenebrae-bench` run, then all pass with pinned hashes and benchmark numbers unchanged.
- Given a scene with `domain_margin` > 0, when baked with `coarsen_outside` on and off, then the coarse bake has fewer voxels and is faster, both tomes load and `find_cell` resolves every in-volume sample to the same set of reachable cells' tiles.
- Given that scene, when baked at 1 and 4 threads and with a warm cache, then the tome bytes are identical.

## Implementation Notes

- Per-tile `tile::Space` (coarse `Grid`, `Bricks`, `lo`, `s`, seed block): every tile pass runs in it; at `k = 0` it is exactly the grid's space, so margin-0 bakes, the pinned tome hashes, the cache pin (no `cache::VERSION` bump) and the benchmark are unchanged.
- Output scaling: cell boxes and face rects/planes ×`s` (faces coalesced at the grid's resolution), open/surface/solid counts ×`s³`, output and flood layers `Layer::upsample`d; a coarse tile's flood reads a neighbour layer at any of its `s²` positions.
- Cache: pass-A key gains `k` only when `k > 0`; pass-B key hashes each volume's view range (a count prefix only with several volumes). Old caches cannot hit a coarse tile.
- Prepare buckets and estimates each tile in its own space; layer bound charged ×`s` for upsampled rows.
- Several volumes: domain over their bounding box (margin past that box); reach box = box around the cell's overlaps with each volume; view mask = union of per-volume ranges.
- Measured on the far-scene test (8 m tiles, 0.5 m voxels, margin 100, 2 m blocks): 1,643,520 against 3,112,960 baked voxels, tile-stage cells fewer, bake ~108 ms against ~176 ms.

## Spec Change Log

## Review Triage Log

Pass 1 (2026-10-14; blind B, edge-case E, verification-gap V):

| # | Finding | Verdict | Route | Evidence |
|---|---------|---------|-------|----------|
| B1/E2 | Docs say coarsening "only matters with a domain margin"; with several volumes, gap tiles coarsen at margin 0 | low | patch | `prepare` takes the hull of all volumes; `several_view_volumes` asserts `level_shift(at(64)) > 0` at margin 0. Wording fix in README, `coarsen_outside` doc, `bake.rs`, test comment. |
| B2 | Margin bakes change even with `coarsen_outside` off (old raised feature / block gone) | low | patch | True by design (spec: `k = 0` when off); harm is an undocumented output change for margin users. Note it in the `domain_margin` / `coarsen_outside` docs. |
| B3 | Warm-cache tome identity not asserted | false | — | `rebake` (tests.rs:1827) asserts the cached outcome equals a fresh bake. |
| B4 | Coarse voxels may close gaps and hide far targets; unchecked | false | — | Intended: tiles away from the volumes are built at `SH·2^k` (Intent); the camera never enters them. |
| B5/V3 | Collapse growth past `collapse_size` never exercised | medium | patch | Verification layer reverted the limit with all tests passing. Add a test with a small `collapse_size` on the far scene. |
| B6/E5/V6 | Public `Voxels::surface/solid` answer for the covering coarse voxel; undocumented and untested | low | patch | `locate` divides by the tile scale; public docs promise per-voxel answers. Doc on the public methods plus one `voxelize` check in a coarse tile. |
| B7/E3 | Reach box is the hull of a cell's volume overlaps, not a union | low | patch | Only a cell meeting two disjoint volumes is affected, and the hull only keeps more cells (conservative). Correct the wording ("box around"), no code change. |
| B8 | New rule lacks provenance / AD entry | false | — | Out of what this change records; the rule is stated in the spec Intent and README. |
| B9 | Motivating large-bubble timing not measured | maybe-false (medium if true) | defer | Needs an external large-scene bake; no game data in the repo. |
| B10 | CLI cannot pass extra view volumes | low | reject | Library callers set it; adding CLI parsing is new surface for an unlikely dev path. |
| B11 | One distant extra volume changes every tile's pass-B key | medium | patch | `Keys::b` hashes every volume's (mostly empty) range plus a count; hash only non-empty ranges with their volume index (single volume unchanged). |
| B12/E4 | `region_tile` shift starts at `u32::MAX` | false | — | Its only caller (global.rs:1588) passes a region of ≥ 2 tiles (`single` takes the other branch), so `members` is never empty. |
| B13 | Pad test per-axis, `d` Euclidean | false | — | That is the spec's rule (Intent: "overlapping any view volume grown by `pad`", `d` Euclidean). |
| B14 | Spec logs empty | false | — | Filled by this step. |
| E1 | Seed falling in a coarse surface voxel is dropped | low | reject | Pre-existing behaviour for fine surface voxels; seeds are camera points inside the volumes, and coarse tiles lie ≥ `pad` away. Fix needs a new search branch. |
| V1 | Pass-A key's level shift untested; dropping it breaks cached re-bakes silently | medium | patch | Demonstrated by the layer's probe. Add a test: cache from `coarsen_outside: false`, `rebake` with true. |
| V2 | Coarse `+0x48` level assertion passes with the old formula | low | patch | Fine tome also has levels > 0.5. Assert a computed level for a known coarse leaf. |
| V4 | Coarse tile reading all `s²` neighbour positions unverified | medium | patch | Mutating to one position passes every test. Add a seam test. |
| V5 | `several_view_volumes` does not observe the union in reachability or mask | medium | patch | Restricting either to the first volume passes every test. Compare against a bake without the extra volume. |

## Design Notes

Coarse tiles report everything at the fine resolution, so the global stage never learns about per-tile voxel sizes: a coarse face voxel becomes `s × s` fine face positions with the same cell or label. Coarse voxels align with the fine grid because tile origins are multiples of `T` and `s` divides `T / SH`.

## Verification

**Commands:**
- `cargo fmt --all -- --check` -- clean
- `cargo clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo test --release` -- all pass
- `cargo test --release -p tenebrae-bench` -- all pass, no baseline change
