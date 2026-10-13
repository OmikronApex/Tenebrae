---
title: 'M3c — incremental re-bake (tile cache)'
type: 'feature'
created: '2026-10-12'
status: 'done'
baseline_commit: '0490a2c6ec5ae552d060e6591374ec6b389f58e9'
route: 'dispatch'
review_loop_iteration: 0
context:
  - '{project-root}/docs/planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** After a map edit, every bake recomputes all tiles, although the tile stage is about 98 % of the cost and most edits touch a few tiles (research handover, "Recommended follow-up: M3c"). Umbra has a tile cache for this; Tenebrae has none.

**Approach:** Add a per-tile cache to the library, with two levels: pass A (flood) and pass B (`TileOut`). Each entry is keyed by a 128-bit hash of every input that the tile's result depends on. A re-bake recomputes only the tiles whose key changed, then always runs the full global stage. The cache is plain bytes in and out; `tools/` owns the file.

## Boundaries & Constraints

**Always:**
- **Byte identity, required.** A cached re-bake gives exactly what a fresh bake of the edited input gives: the tome bytes, `cell_samples`, `object_map`, `voxels`, and every `Report` field except the new cache counters. This is the same oracle as the 1-vs-N-thread check.
- Library rules hold: no dependencies, no `unsafe`, no I/O, deterministic output, and 1 thread spawns nothing. `bake()` output is unchanged.
- Keys must be sound. A key misses no input, including cross-tile ones:
  - pass A depends on the tile's own triangles, its seeds, and the global "no seeds" flag;
  - pass B depends on the tile's post-join liveness, and on each neighbour's flood data and liveness;
  - Parity mode depends on the whole mesh;
  - object lists depend on the targets that overlap the tile.
- The cache bytes carry a magic and a version. A cache that is malformed, of another version or from another grid is ignored, never an error, and the bake is then a full one.
- CLI positionals are append-only (AD-12).
- **Decision (agent, delegated by the user 2026-10-12 for a headless run):**
  - A change of grid (origin, dims, tile, voxel, brick layout) invalidates every entry. Growing the view volume is therefore correct but not incremental.
  - Parity tiles are keyed on the whole mesh, so any occluder edit recomputes them; target-only edits still reuse them.
  - A target-only edit recomputes pass B, and only for tiles whose one-voxel-grown box the target overlaps.

**Never:**
- No change to bake algorithms, the tome format or the engine tables.
- No unbounded cache growth: the returned cache holds only the entries of this bake.
- No new crate dependencies. Hashing is in the library.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| No edit | re-bake with own cache | identical; every tile hits both levels | N/A |
| Add mesh | small box added in one tile | identical to fresh; only tiles it reaches miss | N/A |
| Remove mesh | a wall removed (opens a region) | identical to fresh (liveness changes reach far tiles) | N/A |
| Move mesh | box moved across a tile boundary | identical to fresh | N/A |
| Target only | target added | identical; pass A all hits; pass B misses only overlapped tiles | N/A |
| Grow volume | view volume grown | identical; all miss | N/A |
| Parity | occluder edit, Parity mode | identical; all pass-B miss | N/A |
| Bad cache | truncated, wrong magic or version | full bake, identical to fresh | ignored, counted |
| Threads | cached re-bake at 1 vs 4 threads | identical, including the cache bytes | N/A |

</frozen-after-approval>

## Code Map

- `src/bake.rs:734` `bake`. Pipeline: `prepare` (:426; origin is already `floor(vv.min/T)*T`, :437; triangles are bucketed per tile at :527-560 with a one-voxel low margin) → pass A `flood()` (:722, `run_tiles(tile::flood_pass)` then `global::join`) → pass B (:740, `solid_pass` + `cell_pass`) → `assemble`, `empty_leaves`, `reachability`, `collapse`, `emit` (:771-783). `run_tiles` (:689) uses ordered per-tile slots; reuse it and run only the misses.
- `src/bake/tile.rs:396` `Flood` (pass A value) and `:678` `TileOut` (pass B value). Both are plain data: global voxel indices, world floats, tile-local cell ids. Fine to cache as-is, since the grid is part of every key.
- `src/bake/tile.rs:486` `flood_solid`. It reads `live[t]` and the neighbours' `Flood` layers and `live[n]` (`touches_live`, :507-537), so these go into the pass-B key.
- `src/bake/tile.rs:585` `parity_solid`. It casts rays through the whole-mesh BVH, normalised by the global `reach` extent (`src/bake.rs:638-649`).
- `src/bake/tile.rs:1269` `objects`. It stores `(cell, target index in input order)`. Cache it as `(cell, position in the tile's overlapping-target list)` and remap on load. The overlap test is the one `objects` uses (target box grown by one voxel, clipped to the domain and the tile).
- `src/bake/tile.rs:381` `starts`. It reads the tile's seeds plus the global `input.seeds.is_empty()`.
- Tile-stage `Params`: `solid`, `cells`, `tile`, `voxel`, `surface_occludes`, `max_cells_per_tile`, `cell_merge`, `midpoint_views`. Global-only fields (`base_distance`, `reachability_threshold`, `collapse_size`, `empty_leaves`, `global_groups`) do not enter keys. `threads` and `memory_limit` do not either.
- `src/bake/tests.rs`: `scene()` (:46), `SEEDS` (:140), `lever_input`, `fnv`, and the oracle pattern of `thread_count_does_not_change_the_bake` (:385).
- `tools/src/bin/bake.rs`: positionals 1-22; outputs are written at :214-235.
- the evaluation harness: they pass positionals through; update the usage text only.

## Tasks & Acceptance

**Execution:**
- [x] `src/bake/cache.rs` (new) -- `TileCache` (entries by key, both levels), `to_bytes` / `from_bytes` (magic `TBTC`, version word, tolerant parse returning an empty cache), and a dependency-free 128-bit hasher over little-endian bit patterns. Key material:
  - **A:** grid signature, tile-stage `Params`, tile coordinate, the tile's bucketed triangles as a sorted set of vertex bits, its snapped seeds, the no-seeds flag.
  - **B:** A(t), `live[t]`, and for each of the six faces the neighbour's A key and `live[n]` (or absent), the overlapping targets' box bits in input order, and in Parity mode a whole-mesh hash.
- [x] `src/bake.rs` -- `pub fn bake_cached(input, cache: &TileCache) -> Result<(Tome, Report, TileCache), Error>`. `bake` calls it with an empty cache and drops the result. Add `Report::cache_hits: [usize; 2]` (pass A, pass B) and a flag for a rejected cache. Run only misses through `run_tiles`.
- [x] `src/bake/tile.rs` -- split `objects` so that pass B stores local positions and the remap to input indices happens on use.
- [x] `src/bake/tests.rs` -- one test per matrix row, each comparing against a fresh bake of the edited input and asserting the expected hit and miss counts.
- [x] `tools/src/bin/bake.rs` -- positional 23 `cache-file`: read it if it exists (a rejected cache is noted on stderr), write it after the bake, print the hit counts. Update the `//!` header.
- [x] `README.md` -- M3c row. Spec Implementation Notes: on the parity setup, a no-edit re-bake and a one-prop edit, with time, memory, hits, cache size and identity to a fresh bake.

**Acceptance Criteria:**
- Given the parity setup and a cache from its full bake, when it is re-baked unchanged, then the tome is byte-identical, every tile hits, and the time is far below the full bake's.
- Given the default `Params` and no cache, when it is baked, then the output is byte-identical to the pre-M3c bake.

## Implementation Notes

- **Shape.** `src/bake/cache.rs` holds `TileCache` (two `BTreeMap<u128, Vec<u8>>`, pass A and pass B, plus a `rejected` flag), the FNV-1a 128 hasher, `Keys` (a per-bake base signature for the grid and tile-stage `Params`, plus a whole-mesh hash in parity mode) and payload encode/decode. Each entry stores its encoded result, so a hit copies bytes into the next cache and a miss encodes the fresh result. Both happen inside `run_tiles`, so a hit never runs a pass.
- **File layout.** `TBTC`, u32 version 1, u128 checksum (FNV-1a 128 of everything after it), two u32 entry counts, then per entry a u128 key, a u64 length and the payload, keys ascending. The parse rejects truncated input, trailing bytes, a wrong magic or version, a checksum mismatch, and unsorted keys. Payload decoders also bounds-check every label, cell, node, face and object index against this bake (tile face size, label and cell counts, overlap-list length, tree children after parents). A payload that does not fit is a miss, never a panic (test `bad_caches_are_ignored` truncates every payload).
- **`grid_bytes` identity.** `Layer::bytes` counts `runs.capacity()`, which a decoded layer does not reproduce. `Flood` and `TileOut` therefore carry `layer_bytes`, computed when the pass builds them, and `bake_cached` sums those. Same value as before, and identical cached or fresh.
- **Objects.** `tile::overlaps(ctx, t)` lists the input indices of targets whose grown box reaches the tile, using the same test as before, now factored into `target_range`. `objects` emits `(cell, position in that list)`. `remap_objects` maps positions to input indices after the payload is encoded (fresh) or decoded (hit). The output order is unchanged.
- **Tests** (`src/bake/tests.rs`, 8 m tiles, 0.5 m voxels, levers on, 4 threads, 8×8×5 = 320 tiles): one test per matrix row. Each re-bake is compared with a fresh bake on the tome bytes, every `Report` field except the cache counters, and the returned cache bytes. Measured hits:
  - no edit: [320, 320];
  - add a box: [319, 313], i.e. the tile plus its six neighbours miss pass B;
  - prepend a target: [320, 319];
  - grow the volume: [0, 0] over 360 tiles;
  - parity: [0, 320] unchanged, [0, 0] after an occluder edit, [0, 319] after a target edit;
  - remove a wall, move a box: identical, with partial hits.

  The threads row compares 1 and 4 threads, including the cache bytes. It excludes `grid_bytes`, which counts the `threads` largest tiles by definition. The pinned `l4_off_bakes_are_unchanged` hashes still pass, so default `Params` without a cache are unchanged.
- **The parity setup** (9.2 M triangles; 3,385 boxes; 586 seeds; (-192,-160,-672)..(160,192,608); defaults, 16 threads; 4,840 grid tiles; via the evaluation harness with positional 23):

  | Run | Time | Hits A / B | Peak − baseline (WS / private) | Output |
  |---|---|---|---|---|
  | plain bake (`bake`, no cache path), two runs | 66.4 / 66.5 s | — | +208 / +227 and +203 / +225 MB | 2,147,600 B, byte-identical to the pre-M3c L4 bakes (`l4_axis`, `l4_mem16`, `l4_mem1` tomes, samples, maps) |
  | baseline the pre-M3c baker (pre-M3c), two runs | 68.8 / 65.2 s | — | +201 / +220 and +199 / +218 MB | identical |
  | `bake_cached` with an empty cache (first version of the tool) | 71.9 s | 0 / 0 | +249 / +269 MB | identical |
  | full bake writing the cache | 74.6 s | 0 / 0 | — | identical |
  | re-bake, no edit | 2.4–2.5 s | 4,840 / 4,840 | +262 / +249 MB | identical tome, samples and map; status 0 on 586/586, 0 truly visible culled |
  | re-bake, one prop moved 1 m (owner 32, 604 triangles, edit at x ≈ -51, z ≈ 107) | 4.9 s | 4,839 / 4,833 | +230 / +269 MB | identical to a fresh bake of the edited scene (70.2 s, +261 / +282 MB) |

  The cache is 37,967,518 B (36.2 MiB). A re-bake's peak memory stays near a full bake's: the global stage runs in full and still holds every tile's flood and cells, and the decoded results plus both caches are held during the passes. The first measurement of a plain bake (72 s, +249 MB WS) went through `bake_cached` with an empty cache, which hashes every key and encodes every tile for a cache it drops: about +50 MB and a few seconds. Review fix: `bake()` now runs an internal no-cache path (`bake_with(input, None)`: no keys, no encoding, no returned cache), and the tool calls `bake` when no cache file is given. Remeasured, the plain bake is back at the pre-M3c cost (66 s, +203 to +208 MB WS, against +199 to +201 MB at the pre-M3c baker in the same session). The +216 MB in the README's M3b row was an earlier session's figure.

## Spec Change Log

## Review Triage Log

| # | Source | Finding | Verdict | Route | Evidence |
|---|---|---|---|---|---|
| 1 | blind | Parity keys miss the view-volume extent that `reach` grows from | false | reject | `domain` is built from the grid (`origin`, `dims * voxel`, `src/bake.rs:516`), which is in every key; sub-voxel view-volume moves leave it unchanged. |
| 2 | blind | Nothing ties a cache to the code that wrote it; a tile-pass change without a `VERSION` bump reuses stale entries silently | medium | patch | `VERSION` is bumped by hand only. A test that pins a small scene's cache bytes forces the bump. |
| 3 | blind, edge-case | `bake()` hashes and encodes every tile for a cache it drops (72 s / +249 MB against M3b's 65 s / +216 MB, not separated from noise) | medium | patch | An internal no-cache path adds no public surface. |
| 4 | blind | README plain-bake numbers inconsistent between M3b and M3c rows | low | patch | Grouped with 3: re-measure after the fix. |
| 5 | blind, edge-case | Hit payloads are copied, so a re-bake holds two caches | low | reject | About 2 x 38 MB against a +250 MB peak; the fix changes the public signature. |
| 6 | blind | Cache memory not counted by `memory_limit` / `grid_bytes` | low | patch | Doc-only correction: state the exclusion. |
| 7 | blind, edge-case | `decode_*` do not range-check face `k`/`rect`, cell boxes within the tile, `Split::at`, tree shape, label count, seeded order | low | reject | Only a checksum-valid payload under a matching 128-bit key reaches them, which needs a crafted file; the fix is a set of guards. |
| 8 | blind | Bounds-check paths untested | low | reject | Follows 7. |
| 9 | blind, verification-gap | Each cross-tile pass-B key term (`live[t]`, neighbour A key, neighbour liveness, target box bits) is removable with no test failing | low | defer | Mutation survival only; no wrong-output scenario found. Isolating each term needs new scene design. |
| 10 | blind, verification-gap | No test edits seeds; deleting the no-seeds flag from the key passes every test | medium | patch | Verification-gap showed a wrong output with the flag removed and seeds cleared. |
| 11 | verification-gap | No test changes tile-stage `Params`; deleting `midpoint_views`, `surface_occludes`, `cell_merge` or `max_cells_per_tile` from the key passes every test | medium | patch | Shown with a scratch test (wrong output once the line is removed). |
| 12 | verification-gap | No test moves geometry within one tile; the triangle-bits key line is unverified | medium | patch | Shown: the within-tile cube move fails identity with that line removed. |
| 13 | blind, edge-case | CLI: non-atomic cache write, `expect` panic on an unwritable path, empty positional 23, cannot tell rejected from all-miss | low | patch | Direct tools-side corrections; no library surface. |
| 14 | verification-gap | CLI `cache-file` path has no automated test | low | defer | Tools crate has no harness (as positionals 15-22). |
| 15 | blind | Matrix lacks the external harness scripts edits as a task | low | reject | Done by the implementer; the harness lives outside this repository. |
| 16 | blind | `TileCache` `Debug` prints all payload bytes | low | reject | No caller formats a cache; tests compare bytes. |
| 17 | blind | Add-box test's exact counts are brittle | low | reject | Exactness is the intended pin (verification-gap relies on it). |
| 18 | edge-case | Remove-wall / move-box tests assert loose hit counts | low | reject | The matrix asks those rows for identity to a fresh bake only. |
| 19 | edge-case | Threads row does not compare against a fresh bake | false | reject | The matrix row is 1 vs 4 threads, including cache bytes, which the test checks. |

## Verification

**Commands:**
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` -- expected: clean.
- `cargo test --release --workspace` -- expected: all pass.
- `cargo build --release -p tenebrae-tools`, then two runs of the evaluation harness with the same cache path -- expected: the ACs.
