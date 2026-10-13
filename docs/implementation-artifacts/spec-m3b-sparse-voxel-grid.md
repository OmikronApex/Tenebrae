---
title: 'M3b — tile-streamed sparse grid, parallel tiles, Umbra parity'
type: 'feature'
created: '2026-10-11'
status: 'done'
baseline_commit: '78a6911c769e1f64f037b92e44117f17966318aa'
route: 'dispatch'
review_loop_iteration: 0
context:
  - '{project-root}/docs/specs/spec-m3b-sparse-voxel-grid/SPEC.md'
  - 'the m3b-umbra-global-stage research answers (external)'
  - '{project-root}/docs/planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `bake` allocates several dense whole-domain voxel arrays (`surface`, `blocked`×3, `reached`, `solid`, `cell_of`, `view_of`). That is ~4 GB at 0.25 m on the parity scene's reduced domain, so Umbra's 0.125 m and full domains are out of reach. The output also keeps unreachable cells, and the tile stage is single-threaded.

**Approach:** Implement every capability in `docs/specs/spec-m3b-sparse-voxel-grid/SPEC.md` (CAP-1..CAP-8, all goals kept — user 2026-10-11). Build order:
1. Restructure the bake into a tile-streamed pipeline with brick-sparse per-tile storage. Output stays byte-identical (CAP-1, CAP-3, CAP-4).
2. Run the tile stage on a `std::thread::scope` pool (CAP-8).
3. Add reachability (CAP-6), then leaf-tile collapse (CAP-7).
4. Move the default voxel to 0.125 m (CAP-5) and accept on the parity setup (CAP-2).
5. **Amendment (user, 2026-10-11, after CAP-2 missed: 9.05 MB tome, status 2 on all 586 queries):** add three switchable Umbra-parity levers, in this order: L1 large top-tree leaves over empty space, L2 a per-tile cell merge at R = 6 m, L3 grouped global cells. Each is accepted by the external evaluation. Midpoint view-tree encoding was offered and **not** chosen.

## Boundaries & Constraints

**Always:**
- Library rules:
  - Spine AD-1: no dependencies, no `unsafe`. Work distribution uses `std::sync` only.
  - AD-5: deterministic output. Hash containers may aggregate but never order anything, and results join in tile-index order.
  - AD-7: 1 thread spawns nothing.
  - AD-11: every returned index refers to the tome after compaction.
- AD-8 drops: a target is dropped only when it touched ≥1 cell before CAP-6 and none after. Buried targets are kept. Surviving targets keep input order.
- AD-12 CLI compatibility: CLI positionals are append-only, and each default equals `Params::default()`. Any new blob starts with a magic and version word. Any the external harness scripts that a CLI change affects is updated in the same session.
- Byte-identity gates:
  - CAP-1 + CAP-3 + CAP-4 land as one commit whose 0.25 m parity-scene tome and `.samples.bin` are byte-identical to the M3 dense baker. From the amendment on, that gate holds with L1–L3 off (`Params::levers_off()`), and it must keep holding after each lever lands.
  - CAP-8 is gated on 1-thread vs 16-thread byte identity.
- CAP-6 and CAP-7 change bytes, so they are accepted only by the external 586-view evaluation.
- **Decision (user, 2026-10-11): default tiles stay 32 m.** CAP-7 is a no-op at defaults, as in Umbra. Once collapse is implemented, it is also measured on a 16 m-tile parity bake: tome size, cells per tile, and the external evaluation. The SPEC's "16 m tiles" in CAP-5 and CAP-7 reads as "the tile size in use".
- **Decision (user, 2026-10-11): the seam merge uses the fill rule.** A pair across the shared plane merges when `union_fill ≥ fill`, with no length threshold. If the parity goals are missed, the next option is a squared-length cost in m²: the union's non-open volume divided by the shared portal area, compared against (0.5 × block)².
- **Amendment levers (user, 2026-10-11).** Each lever is a `Params` field, on by default, with a CLI positional. A bake with all three off reproduces the pre-amendment bytes.
  - **L1 `empty_leaves: bool`.** The top tree splits only along grid planes that bound tiles with triangles, using the same 1-voxel-grown boxes as tile bucketing. Each maximal empty region becomes one leaf tile. That tile is an ordinary tile, either reached-open or solid, with cells and portals derived as for any tile. Tile-less (`None`) leaves are never emitted. Umbra: 500 leaves, 180 of them larger than 32 m.
  - **L2 `cell_merge: f32` (metres, 6.0; 0 = off).** This is a new per-tile pass that runs after the best-first merge and its relabel, and before faces and the view tree. It leaves the best-first merge itself unaltered. It runs Umbra's modes in order: mode 1 grows each cell's box toward its 6 faces while the cost stays within the threshold, then mode 2 makes one pairing pass along portals. Cost = the solid volume pulled into the union's bounding box ÷ the shared opening area (m); a merge happens when the cost < `cell_merge`. If parity is missed, the fallback is boundary-area growth in m² against `cell_merge`². Cells are visited in tile-local order, so the pass is deterministic.
  - **L3 `global_groups: bool`.** Each global cell is one connected component of a tile's cells over in-tile portals, instead of one global cell per tile cell. Global portals join global cells, and the tome's cell-run list (`+0x60`) maps each global cell to its tile cells. Umbra: 1,158 global cells for 2,538 cells. Global portals are still never stripped.

**Never:**
- No inner-node LOD tiles.
- No progress or cancel hooks.
- No M3c tile cache.
- Global portals are never stripped.
- Flood, block components, best-first merge, view-tree split choice and solid ownership are not altered.
- No backface ray tests.
- No dense whole-domain voxel array of any type.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| 0.25 m regression | The parity scene, `32 0.25 merged0.95b6 flood-occluding`, seeds, L1–L3 off | tome + samples bytes == the M3 dense baker build; lower peak | N/A |
| Threads | same bake at threads 1 and 16 | tome, samples, object map byte-identical | N/A |
| Over limit | domain/voxel/threads whose estimate > `Params::memory_limit` | `Err(Error::TooLarge{needed, limit})`; no grid allocated | typed error |
| Bad triangle index | index ≥ vertices.len() | `Err(Error::Params(..))` | no panic |
| Owners mismatch | `owners.len() != triangles.len()` | `Err(Error::Params(..))` | no panic |
| Non-finite vertex / box | NaN or ∞ in vertex, target, view volume; box min > max | `Err(Error::Params(..))` | no panic |
| Grid overflow | voxel count overflows `usize` | `Err(Error::TooLarge{..})` or `Params` via checked math | no panic |
| Unreachable target | target whose cells all removed by CAP-6 | absent from tome; `Report::object_map[i] == None` | N/A |
| Emptied tile | tile with all cells removed | kept as leaf with 0 cells and a 1-node view tree whose leaf value is 1 | N/A |

</frozen-after-approval>

## Code Map

- `src/bake.rs` -- the whole baker (1,582 lines). What it holds today:
  - `Params` (block in **voxels**, tile 32, voxel 0.25) and `Report`.
  - `Error`, which has only `Params(&'static str)` and no `Display`.
  - Functions: `bake` → `voxelize` (global surface/blocked/flood/parity) → `bake_cells`, which builds the top tree, then cells, merge, samples, portals (`BTreeMap` keyed `(cell, axis, side, neighbour, k)`), objects, `view_of`, view trees and the tome.
  - Reuse unchanged: `build_kd`, `choose_split`, `portal`, `pack_box`, `quant`, the cell-growth and merge logic, and the owner rule.
- `src/geom.rs` -- `Mesh{vertices, triangles, owners}`, `Bounds`, `triangle_box` (SAT), `Bvh::crossings` (parity). Do not change.
- `src/tome/mod.rs:221` -- `tome::Error`. `Version(u16)` needs a `Display` description.
- `src/lib.rs` -- module wiring. Add `mod` lines for any new private bake submodules.
- `tools/src/bin/bake.rs` -- positional CLI. Args: `[10 tile 11 voxel 12 mode 13 solid 14 SEEDS]`. The block in `mergedF bB` is parsed in metres and converted to voxels. Its `//!` header documents the blobs.
- `tools/src/bin/diagnose.rs` -- reads `Voxels.surface/solid` as dense `Vec<bool>`. It must keep working (AD-10 lets `Voxels` change shape).
- the evaluation harness -- calls `bake.exe` positionally and writes an **identity** `map.json` itself. It must read Tenebrae's object map once CAP-6 can drop targets.
- the evaluation harness -- the Q4 memory method to mirror for Tenebrae.

## Tasks & Acceptance

**Execution:**
- [x] `src/bake.rs` (split into private submodules `src/bake/{grid,tile,global}.rs` if it helps) -- CAP-4: change `Params` block fields to metres, validate input (indices, owners, finite, `min ≤ max`, checked grid sizes), add `impl Display + std::error::Error for Error` with `TooLarge{needed:u64,limit:u64}` -- validation precedes any allocation.
- [x] `src/tome/mod.rs` -- describe `Error::Version` in `Display`.
- [x] `src/bake.rs` -- CAP-1/CAP-3: tile-streamed pipeline with brick-sparse tile storage. See Design Notes for the stages and the memory estimate. Add the `Params` fields `threads` (16), `memory_limit` (4 GiB), `reachability_threshold` (0.1) and `collapse_size` (32.0), and the `Report` fields `grid_bytes` and `object_map`. Rationale: memory follows the touched bricks.
- [x] `src/bake.rs` -- CAP-8: the per-tile passes run on a scoped pool. An atomic counter hands out tiles, and per-tile result slots are joined in index order.
- [x] `src/bake.rs` -- CAP-6: Umbra pass 2.
  - Compute the reach boxes, then the components and the threshold test.
  - Renumber cells within each tile, keeping their order.
  - Drop portals to removed cells. A view leaf that pointed at a removed cell becomes the solid value. Then collapse sibling leaves that hold equal values, bottom-up.
  - Drop targets and fill the object map.
- [x] `src/bake.rs` -- CAP-7: post-order collapse over the top tree.
  - Merge gate: `max_extent(union) ≤ collapse_size` and summed cells < 96.
  - Run the seam merge across the shared plane, using the fill rule.
  - Build the merged view tree: a root split at the shared plane, with A's and B's trees as its children and leaves remapped.
  - Re-quantize portals against the union frame. Keep portals as `PortalSpec` (world floats) until emission so this is possible.
  - Rebuild the top tree.
- [x] `src/bake.rs` -- CAP-5: `Params::default().voxel = 0.125`, once CAP-2 has been measured.
- [x] `src/bake.rs` tests -- unit tests:
  - every I/O-matrix error row;
  - sparse vs a small dense reference, which must be equal;
  - threads 1 vs 4, byte-identical on a multi-tile scene;
  - reachability drops a sealed pocket and its target;
  - two 16 m tiles collapse.
- [x] `tools/src/bin/bake.rs` -- append positionals `15 threads 16 reach-threshold 17 collapse-size 18 memory-limit-MiB`, all defaulting to `Params::default()`. Write `OUT.map.bin` (magic `TBMP`, version 1, u32 count, then a u32 per target with `u32::MAX` = dropped). Print a `scene loaded` stderr line before calling `bake`, and print `grid_bytes`.
- [x] `tools/src/bin/diagnose.rs` -- adapt it to the new `Voxels` shape. Its output must be unchanged for the same args.
- [x] the evaluation harness -- use `OUT.map.bin` for `map.json`, and pass the new positionals through.
- [x] the evaluation harness (new) -- run the Q4 method on the `bake` child: sample WS and private bytes every 50 ms, baseline at the `scene loaded` line.
- [x] `src/bake.rs` -- Amendment L1: `Params::empty_leaves`. Top-tree build over the populated tiles, with maximal empty regions as large leaf tiles; see Boundaries. Also add `Params::levers_off()`.
- [x] `src/bake.rs` -- Amendment L2: `Params::cell_merge`. Per-tile mode 1 + mode 2 merge pass after the best-first merge; see Boundaries.
- [x] `src/bake.rs` -- Amendment L3: `Params::global_groups`. Global cells as in-tile connected components, using the cell-run list.
- [x] `src/bake.rs` tests -- amendment unit tests:
  - with levers off, output equals the dense reference;
  - L1 makes one large leaf for an empty region and keeps single tiles where triangles are;
  - L2 reduces a corridor's cells, and its output is identical at threads 1 and 4;
  - with L3 on, every global cell's tile cells are connected and global portals join distinct groups.
- [x] `tools/src/bin/bake.rs` + the evaluation harness -- append positionals `19 empty-leaves(0/1) 20 cell-merge-m 21 global-groups(0/1)`, defaults = `Params::default()`. Update the `//!` header.

**Acceptance Criteria:**
- Given the M3 dense baker release `bake.exe` and the new one with L1–L3 off, when both bake the parity scene at `32 0.25 merged0.95b6 flood-occluding` with the same seeds, then the tome and `.samples.bin` are byte-identical and the new peak working set is lower.
- Given the parity setup (the parity scene, domain and VV (−192,−160,−672)..(160,192,608), 3,385 stock boxes) and default `Params`, when it is baked at threads 1 and 16, then:
  - the outputs are identical;
  - the deltas are ≤ +322/+334 MB (1 thread) and ≤ +1,577/+1,723 MB (16 threads), WS/private;
  - the tome is ≤ 1,114,672 B;
  - the external research reports 0 wrongly culled objects and status 0 on all queries.
- Given collapse is implemented, when the parity setup is baked at 16 m tiles, then its tome size, cells per tile and external evaluation are recorded in Implementation Notes next to the 32 m result.
- Given a default bake and an explicit 0.125 m bake, when both are written, then the bytes are equal.
- Given the parity setup, when it is baked with each lever added cumulatively (none, L1, L1+L2, L1+L2+L3), then each bake's tiles, cells, portals, global cells, tome bytes and external evaluation (status, wrongly culled, objects per view) are recorded in Implementation Notes.

## Design Notes

**Pipeline.** The bake makes two passes over each tile, because flood fill is global and tiles are not.
1. **Prepare** (serial):
   - Validate the input.
   - Bucket triangle indices per tile, using each triangle's box grown by 1 voxel.
   - Count the bricks each tile touches, using `triangle_box` against each brick grown by 1 voxel.
   - Compute the memory estimate and refuse if it is over the limit.
2. **Pass A** (parallel), per tile:
   - Voxelize into bricks.
   - Label the components of non-surface voxels over unblocked faces.
   - Mark the start voxels: seeds, or domain-edge voxels when there are no seeds.
   - Emit the 6 boundary layers as run-length rows of (label | SURFACE, blocked bit).
   - Free the bricks.
3. **Join** (serial): a union-find over (tile, label) across shared faces. A component is live when it holds a non-surface start voxel.
4. **Pass B** (parallel), per tile:
   - Re-voxelize and relabel, then decide `reached`. A surface voxel counts as reached if it is a start voxel, or if it is adjacent to a live voxel across an unblocked face, which may lie in a neighbour tile's layer.
   - Then `solid`, cells, merge (per-tile heap), samples, in-tile faces, `view_of`, the view tree, per-cell target lists and boundary-portal cells.
   - Emit the cell-id boundary layers (run-length).
5. **Global** (serial): cross-tile faces from layer pairs, then the portal `BTreeMap` with global ids, then CAP-6, CAP-7, numbering, and the tome.

**Why this stays byte-identical:**
- The flood's reached set is a closure, so it does not depend on traversal order.
- Cells are numbered tile by tile in z, y, x order today.
- The global merge heap restricted to one tile pops in that tile's own order, because pairs never cross tiles and local ids map monotonically onto global ids.
- Within a cell, sample order in global grid order equals tile-local z, y, x order.
- Blocked faces on a tile plane must be computed from the lower voxel's `lo + voxel`, exactly as today. Never use `coord(k)` for them, or the floats can differ.
- Parity mode: per tile, cast the rows' rays with today's endpoints (`reach` = domain ∪ all vertices). Slow but identical.

**Bricks.** A brick's edge is the largest power of two ≤ 16 voxels that divides both voxels-per-tile and voxels-per-seed-block. That makes it 2 m at both 0.25 and 0.125 m, so a uniform brick never straddles a seed block. Storage is `enum Brick<T> { Uniform(T), Dense(Box<[T]>) }`, with bools packed as `u64` words.

`grid_bytes` is the bytes in global structures, plus the sum of the `threads` largest per-tile brick totals. It is a deterministic upper bound on what is in flight, and the estimate uses the same formula.

## Verification

**Commands:**
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` -- expected: clean.
- `cargo test --release --workspace` -- expected: all pass (the round trip needs `TENEBRAE_TOMES`).
- `cargo build --release -p tenebrae-tools`, then `<harness> 32 0.25 merged0.95b6 flood-occluding`. Run it against a M3 dense baker build in a worktree, then `fc /b` the tomes and samples -- expected: identical.
- `<harness>` at 1 and 16 threads on the parity setup -- expected: within the Q4 rows.

## Implementation Notes

Landed in order: CAP-1/3/4, CAP-8, CAP-6/CAP-7 (+ packed bricks, object-pair
dedup), CAP-5 (default voxel 0.125). Code: `src/bake.rs` (API, validation, prepare/estimate, pool),
`src/bake/{grid,tile,global}.rs`, test-only dense reference `src/bake/reference.rs` (the M3 dense
baker verbatim, blocks converted from metres).

**CAP-1 regression (byte identity).** The M3 dense baker release build cannot bake the script's default
reduced box (-160,-160,-288)..(160,192,192) on this machine: its flood stack asks for 24 GiB on top of
~25 GB of dense arrays and aborts. Identity was therefore checked on (-160,-160,-96)..(160,192,96)
(660 tiles), `32 0.25 merged0.95b6 flood-occluding`, captured seeds:

| scene | M3 dense baker | tile-streamed (CAP-1 commit / final) | tome + samples |
|---|---|---|---|
| The parity scene | 584 s, peak WS 16,477 MB / private 25,638 MB | 125 s 1 thread; final 18 s 16 threads, peak WS 516 MB | identical |
| The parity scene with terrain | 569 s, peak WS 17,801 MB / private 25,639 MB | 19 s, peak WS 689 MB | identical |

On the full reduced box the new baker gives 15,056 cells, 5,947,520 B, 0 wrongly culled, but 22 of
586 queries status 2 (identical bytes from the CAP-8 build, so not caused by CAP-6/7; the old baker
cannot run there to compare). `diagnose` output is byte-identical to the old build on the small box.

**CAP-8.** Tome, samples and object map identical at 1 / 16 threads on the small box and on the parity
setup (and 1/4/16 in a unit test, flood and parity solid modes).

**CAP-2 / CAP-5, parity setup, default `Params` (0.125 m, 32 m tiles, CS 32).** Default bake ==
explicit `32 0.125 merged0.95b6 flood` bake (tome, samples, map). Memory (Q4 method,
the evaluation harness, baseline at `scene loaded` = WS 429 / private 556 MB):

| threads | delta WS / private | Umbra row | wall |
|---|---|---|---|
| 1 | **+86 / +141 MB** | +322 / +334 MB | 668 s |
| 16 | **+209 / +227 MB** | +1,577 / +1,723 MB | 73-96 s (Umbra 359 s) |

Result: 4,840 tiles, 19,516 cells, 117,167 portals, **9,052,096 B** (target 1,114,672 B: missed),
Engine evaluation: **status 2 on all 586 queries** (evaluation therefore falls back to frustum culling). Cause
(inferred): Tenebrae's top tree splits the 11x11x40 grid into 4,840 unit tiles, nearly all of them
open sky with one cell and six portals; Umbra's grid holds only 500 leaves. Status 0 appears as soon
as empty tiles collapse: with `collapse-size 0` (no size gate) the same bake has 250 tiles,
14,992 cells, 7,171,728 B, **status 0 on 586/586, 0 wrongly culled**, keeps 363 objects per view
(stock 343, Unity 419). CAP-6 removes no cell on the parity scene (flood fill already makes unreachable
space solid); no target is dropped.

**CAP-7 at 16 m tiles (parity setup, 0.125 m, CS 32).**

| bake | tiles | cells | cells per tile median / p90 / max | tome | engine |
|---|---|---|---|---|---|
| 32 m tiles (default) | 4,840 | 19,516 | 1 / 1 / 256 | 9,052,096 B | status 2 x586 |
| 16 m tiles, CS 32 | 9,460 | 24,450 | 1 / 1 / 95 | 10,500,256 B | status 2 x586 |
| 16 m tiles, CS 16 (no collapse) | 38,720 | 53,115 | 1 / 1 / 80 | 22,620,832 B | status 2 x586 |
| 32 m tiles, CS 0 (unbounded) | 250 | 14,992 | 51 / 136 / 256 | 7,171,728 B | status 0 x586, 0 wrongly culled |

**Open / not met:** The tome-size goal (<= 1,114,672 B) and status 0 at default `Params`. The size gap
is cells (14,992-19,516 vs Umbra's 2,538) and the view trees; the spec's next lever is Umbra's
per-tile SO cell merging (spec update). Status 0 needs empty space in fewer tiles: unbounded
collapse does it today, but changing `collapse_size`'s default is the user's call (decision
2026-10-11: CAP-7 a no-op at defaults).

**Deviations / choices.** `collapse_size` 0 means no size gate (Umbra's CS <= 0). The seam merge runs
only for `CellMode::Merged` (it needs the fill). The flood with seeds given but none inside the
domain now fails (`Params`, spine AD-6); `Report::seeds` counts in-domain seeds. Pass-A layers are
estimated from touched bricks only, and the estimate charges 2 KiB per tile for kept results.
The evaluation harness gained `TENEBRAE_BAKE` / `TENEBRAE_NAME`; the evaluation harness is new.

**Amendment levers L1-L3 (parity setup, 0.125 m, 32 m tiles, 16 threads, cumulative).**

| levers | tiles | cells | cells/tile median / p90 / max | tile portals | global cells | tome | engine status | wrongly culled | objects per view |
|---|---|---|---|---|---|---|---|---|---|
| none (`levers_off`) | 4,840 | 19,516 | 1 / 1 / 256 | 117,167 | 19,516 | 9,052,096 B | 2 x586 | (frustum fallback) | 1,146 |
| L1 | 344 | 15,020 | 22 / 118 / 256 | 90,699 | 15,020 | 7,188,496 B | **0 x586** | **0** | 363 |
| L1+L2 | 344 | 8,367 | 6 / 72 / 201 | 49,362 | 8,367 | 4,646,224 B | **0 x586** | **0** | 384 |
| L1+L2+L3 (default) | 344 | 8,367 | 6 / 72 / 201 | 49,362 | 838 | **3,765,856 B** | **0 x586** | **0** | 384 |

(Stock tome 343 objects per view, Unity 5.1.1 rebake 419 with 9 wrongly culled.) Default `Params`
(all levers on): 1 vs 16 threads byte-identical (tome, samples, map); default == explicit
`32 0.125 merged0.95b6 flood`; memory **+109 / +125 MB** (1 thread, 587 s) and **+194 / +212 MB**
(16 threads, 72 s), WS / private. Levers off still byte-identical to the M3 dense baker on the 0.25 m small
box (+86 MB peak delta).

**Still not met: tome <= 1,114,672 B** (3,765,856 B). Breakdown (`inspect`): view trees ~2.6 MB
(488,618 nodes, every split stored as a float), tile portals ~0.77 MB, cells ~0.29 MB, global
portals ~0.06 MB. The remaining gap is the declined midpoint view-tree encoding; the L2 fallback
cost (boundary-area growth) was not tried, because cells and portals together are ~1.1 MB.

**Lever choices.**
- L1: grid tiles stay single when triangles reach them (non-empty bucket) or when they are not
  wholly one open cell or wholly solid. Split planes: those bounding such tiles; the plane cutting
  off the largest region free of them wins, else the most even one on the longest axis. A wholly
  empty region of mixed open/solid tiles splits at the middle of its longest axis (needed so that
  every leaf is either reached-open or solid). A merged open region is one cell; its samples are
  recomputed over the region box.
- L2: per face, neighbours are taken one at a time in id order; "inside the box" neighbours count
  on every face. Cost = solid voxels in the union box × voxel ÷ shared voxel faces (metres).
- L3: a leaf tile cell's `+16` names its group's global cell with `+20 = 0`; this is the form
  findCell reads for leaf cells (the format reference: `+60` runs serve coarse cells that expand to
  several global cells, which leaf tiles never are), so the `+60` cell-run list stays empty.
  `Report::cell_samples` is per global cell (the members' samples, thinned to 64).

## Spec Change Log

- 2026-10-11 -- **Amendment (user-renegotiated frozen block).** Trigger: at default `Params`, CAP-2 missed. The tome was 9,052,096 B against a 1,114,672 B target, and all 586 queries returned status 2. Investigation:
  - Status 2 tracks the ~4,578 one-cell sky tiles. Umbra covers empty space with large leaves instead, and a run with unbounded collapse gave status 0.
  - The size gap has three parts: the cell count, the 1:1 global cells, and the explicit split floats on every view node.
  - The user chose levers L1, L2 and L3, and declined midpoint view-tree encoding. The 0.25 m byte gate is redefined as levers off. Known risk: the size target may still miss without midpoint encoding (estimated 3–4 MB). The criterion is kept, and a miss is reported, not relaxed.
  - KEEP: everything implemented for M3b so far.

## Review Triage Log

| # | Source | Finding | Verdict | Route | Evidence |
|---|---|---|---|---|---|
| 1 | blind, edge | `levers_off()` doc claims 0.25 m byte identity | low | patch | `bake.rs:177`: identity also needs threshold 0, collapse = tile and VV = domain (`same_as_reference`); doc fix |
| 2 | blind | `Default` comment numbers are stale (78 s/+201 MB, pre-lever) | low | patch | `bake.rs:150`; diverges from README and Implementation Notes |
| 3 | blind | README says status 2 and 9.1 MB are open at defaults | low | patch | README M3b row predates L1-L3 (status 0, 3.77 MB now) |
| 4 | blind | `Report::cells` counts tile cells, while `cell_samples` and `samples.bin` are per group | low | patch | `bake.rs:212/219`, `tools/src/bin/bake.rs:16` says "u32 cells, then per cell"; docs fix |
| 5 | blind | merged samples are not spread evenly (strided concatenation) | medium | patch | `global.rs:711`, `global.rs:1093`: each member contributes ≤64 samples regardless of `voxels`, which biases the probes of L3 groups and collapsed cells |
| 6 | blind | `memory_limit` estimate is not a strict bound (2 KiB per tile for results) | low | patch | Implementer noted "practical bound"; measured +212 MB against a 4 GiB limit. Fix the doc wording only |
| 7 | blind | public `voxelize` holds every tile | low | reject | diagnose-only path; the old dense `Voxels` held more |
| 8 | blind | no test that the estimates bound the measured bytes | low | patch | test-only; guards the TooLarge refusal |
| 9 | blind | default lever combination has no tome-validity test | medium | patch | `levers_on_are_thread_independent` only compares outputs; add round trip + portal/view-leaf range checks |
| 10 | blind | a sub-voxel block rounds to the whole tile for Components | false | reject | documented at `bake.rs:431` and on `CellMode`; same as the reference |
| 11 | blind | an odd block in voxels gives 1-voxel bricks | low | reject | slow, not wrong; rule fixed by spec Design Notes |
| 12 | blind | `threads: 16` fixed default | false | reject | spec task mandates 16 |
| 13 | blind, edge | CLI `m << 20` wraps | low | patch | `tools/src/bin/bake.rs:163`; `checked_mul` |
| 14 | blind | `emit` depends on reachability having filled `object_map` | low | reject | private call order in one function; no other caller |
| 15 | blind | 16-thread test compares only 2 of 4 fields | low | patch | test-only, direct correction |
| 16 | blind | tome test sits in bake tests | low | reject | cosmetic |
| 17 | blind | `samples.bin` has no magic/version | low | reject | not a new blob (AD-12 covers new blobs) |
| 18 | blind | triangle bucketing is serial | maybe-false | reject | perf only; 72 s at 16 threads vs Umbra's 359 s |
| 19 | vgap | L3 global portals unverified (mutation survived) | medium | patch | pre-verified: no-op on `group_links` push passes all tests |
| 20 | vgap | L1 solid/mixed-region handling unverified | medium | patch | pre-verified: dropping `!(kinds[1]&&kinds[2])` passes |
| 21 | vgap | L2 cost refusal unverified | medium | patch | pre-verified: `&&`→`||` in `cost_ok` passes |
| 22 | vgap | collapse 96-cell cap and seam fill gate unverified | medium | patch | pre-verified: both mutations pass |
| 23 | vgap | reachability plane-in-reach-box rule unverified | medium | defer | pre-verified; needs a VV narrower than the domain (M4 view-volume work) |
| 24 | vgap | bake CLI positionals and `map.bin` untested in-repo | medium | defer | pre-verified; tools crate has no harness, the external harness scripts asserts the format |
| 25 | vgap | The evaluation harness indexes samples by cell, now per group | maybe-false | defer | depends on whether `find_cells` returns global-cell ids; would be medium |
| 26 | vgap | new defaults not pinned by a test | low | reject | minor; values are in the spec |
| 27 | edge | `tile_bytes_bound` can overflow `usize` | low | patch | `tile.rs:1575` `tile().pow(3)` × widths unchecked; saturating math is a direct correction |
| 28 | edge | touched bitsets allocate before the limit check | false | reject | 512 B per tile (4,096 bricks of bits); ~2.5 MB on the parity setup |
| 29 | edge | `Voxels` accessors don't range-check | low | reject | caller contract, same as the dense version |
| 30 | edge | a tile can exceed `max_cells_per_tile` when the heap empties | low | defer | same stop rule as the M3 dense baker merge (`reference.rs:801`), pre-existing |
| 31 | edge | L3 leaves the cell-run list `+0x60` empty | false | reject | The format reference: `+0x60` serves coarse cells; leaf cells name their global cell at `+16`; external evaluation passes. Only the spec wording is wrong |
