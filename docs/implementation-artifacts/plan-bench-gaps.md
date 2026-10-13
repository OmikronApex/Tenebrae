# Plan: closing the benchmark gaps (generator 1)

Source: `bench/README.md` results at `GENERATOR` 1 (2026-10-12), `bench/reference/umbra.txt`,
`bench/baseline.txt` and the open benchmark items in `deferred-work.md`. Ordered by value per
effort. Each item names how it is verified; "in-repo" means `cargo test --release -p
tenebrae-bench` and `bench report`, "the external research" means the evaluation harness scored in the engine's runtime.

## 1. Walls inside a tile are no cell boundaries (`rooms`, `seams`, `thin_walls` at 32/64 m)

**Done 2026-10-12.** Stage: lever L2 (`umbra_merge`), whose box growth wrapped the wall; it now
also caps other cells' open space a union's box wraps at `r³`. Numbers in `deferred-work.md`
(resolved entry) and `bench/README.md`. `thin_walls` (free-standing walls in an open field) still
keeps the whole frustum from 16 m up; Umbra culls 4-13 % there. Not pursued.

**Symptom.** `rooms.t32` keeps 6,250 = frustum (Umbra 3,703), `rooms.t64` 6,125; `seams.t32`
5,978 of 6,042; `thin_walls` keeps the whole frustum from 16 m up. 0 false culls.

**Reading.** The room walls stand at x, y = -48, -16, 16, 48: multiples of 8 and 16, not of 32 or
64. At 8 and 16 m every wall lies on a tile seam, which always separates cells, and Tenebrae culls;
at 32 and 64 m the walls lie inside tiles and nothing is culled. So inside a tile the cell build
(6 m block components, the best-first merge at fill 0.95, the L2 merge at 6 m, L3 groups) does not
keep a wall as a cell boundary. Which stage loses it is found by measurement first (step 1a).

**Steps.**
1. Diagnose: bake `rooms.t32` with each stage switched off in turn and print cells per tile and
   their boxes; find the stage that joins the two sides of a wall.
2. Fix that stage's acceptance rule so that a wall separating two open regions stays a boundary,
   whatever the tile height. Candidates: measure fill over the voxels a union *adds*, not its
   whole box; or bound the solid a union pulls in by the opening it merges across, per stage.
3. A unit test in `src/bake/tests.rs`: one tile, a wall with a doorway off the seams, the two
   sides must be different cells.
4. Re-baseline; check parity (586 views, status 0, 0 wrongly culled, objects kept, bytes)
   through the external harness before committing to the new defaults.

**Verify.** In-repo: `rooms.t32` / `t64` tile cell counts rise, no gate regresses. Engine evaluation: kept on
`rooms.t32` falls well under 6,250 with 0 false culls; parity holds.

## 2. Status 2 at 8 m tiles (`city.t8` 150/200, `terrain.t8` 166/200)

**Reading.** The runtime's per-tile traversal budget is twice the tile's cell count
(`Params::max_cells_per_tile` doc). At 8 m Tenebrae has about 1.2 cells per tile (city: 2,280
cells over 1,897 tiles) and about 6 portals per cell, so a one-cell tile entered more than twice
exhausts it. Umbra has fewer, fuller tiles.

**Research A, answered 2026-10-12 (static analysis of the engine).** The runtime sets a budget of
`2 × (cell count of the tile entered)` per tile; it decrements it each time the traversal's
cell-state slot pool (about 1,023 slots) has no free slot and a finished cell's state is evicted
for reuse; it returns status 2 when the budget goes negative. A wide front of small tiles fills the pool. Umbra survives because inner top-tree
nodes carry coarse LOD tiles: its own `city.t8` / `terrain.t8` tomes with those stripped
(`edit … strip-inner`) fail status 2 on 200/200 views. Tenebrae writes leaf tiles only.

**Options.**
- (a) Inner LOD tiles. Umbra's mechanism; needs external research work on their content (coarse cells that
  expand to several global cells through cell runs, `+0x9C` switch distances) and a spec.
- (b) Collapse: at `T` 8 with `collapse_size` 16 both scenes are status 0 on 200/200 (`city` keeps
  3,127, Umbra 3,030; `terrain` 7,360, Umbra 8,252) at 0.4 MB (Umbra 1.7 MB); `collapse_size` 32
  also passes (`city` 3,730). Cheap: a rule (collapse at least 2T below 32 m tiles) or a preset
  note for the consumer. The engine's own setting (T 32 = CS 32) is unaffected either way.

**Done 2026-10-12 (option b, as a documented rule).** `Params::collapse_size` says to keep it at
16 m or more; the library does not clamp it, because `collapse_size = tile` must keep meaning "no
collapse" (the dense-reference tests rely on it). The benchmark bakes with `max(T, 16)`: every 8 m
query is now status 0, `city.t8` keeps 3,127 (Umbra 3,030), `terrain.t8` 7,361 (Umbra 8,252).

**Follow-up.** At 8 m with collapse 16, `rooms` and `seams` cull less than with collapse 8 (4,599
and 4,444 kept against 3,081 and 3,203): collapse's seam merge (`union_fill_ok`, fill rule) joins
cells across walls, the same failure as item 1 in another stage. Option (a), inner LOD tiles,
stays the Umbra-faithful alternative.

## 3. Bistro: hole misses and size

**Misses done 2026-10-12; size open.** Research C (below) found no cell or portal defect: before
the fix, all of Tenebrae's own misses at 32 m were sight lines through openings its surface voxels
close, and 26 targets (bottles, glasses) had grown boxes of surface voxels only, so they were
listed in no cell. Objects are now also listed in every cell whose box overlaps their grown box
(`tile::objects`, mirrored in `reference.rs`; cache version 2). Through-a-hole misses 92 / 73 / 90
/ 48 became 34 / 36 / 40 / 0 at 8 / 16 / 32 / 64 m (Umbra 190 / 25 / 25 / 25), none shared with
Umbra; strict 360 / 322 / 387 / 221 became 108 / 119 / 160 / 0. Bytes +0.1 %. The parity scene: status 0 on
586/586, 0 truly visible culled, 377 kept per view, 2,498,624 B. Variants tried: restricting the
box rule to cells connected inside the tile to the object's own cells, or to tiles where the object
has an open voxel, kept 65 misses at 32 m. Cost: a target in a sealed pocket is listed in a cell
around the pocket when their boxes overlap (target 2 of the unit `scene()` is now kept).

**Symptom.** Through-a-hole misses 84 / 90 / 48 at 16 / 32 / 64 m (Umbra 25); tome 2.5-3.5x
Umbra's. Cells grow with the tile (1,048 at 16 m, 1,494 at 32 m, up to 145 per tile; Umbra 217
global cells at 32 m).

**Steps.** (a) Research C below. (b) Try the merge rule of item 1 and the L2 threshold on Bistro;
clutter (chairs, railings) makes many small seed cells the 0.95 fill test will not merge.
**Verify:** in-repo cells and bytes; external research misses.

**Size, 2026-10-12 (view-tree change uncommitted until the external research scores it).** On `bistro.t32` 68 % of
the tome was view trees (795 KB against Umbra's 28 KB; 368k leaves one voxel wide against 6.6k),
mostly from fixing one owner per surface voxel. Lever L4's trees now let a solid voxel that shares a
face with open space take the cell of any open voxel among its 26 neighbours (`tile::view_of`); a
leaf that could take several keeps the list until `reachability` gives it the first cell that stays.
Bytes: Bistro 509,008 / 673,984 / 1,168,832 / 787,488 became 442,880 / 524,080 / 673,216 / 441,712 at
8 / 16 / 32 / 64 m, Sponza -17 to -20 %, no case larger, cells and portals unchanged. Variants: every
surface voxel "don't care" gave 596,272 B at 32 m but leaves an eye inside a surface with no cell;
owning voxels that touch open space only at an edge or corner grew `terrain` 15 %; reach 2 (a cell
up to 0.25 m away) saves another 7 %. Still to run: the evaluation harness (killed for low memory) and
parity.

**What remains** (`bistro.t32` 673 KB against 287 KB): view trees 300 KB against 28 KB, tile
cells 69 KB against 35 KB, tile portals 185 KB against 117 KB. With 8 m blocks (item 5) portals
match Umbra's and cells fall to 1,270, but the view trees stay at 239 KB: even with every surface
voxel "don't care", 33k open leaves one voxel wide separate cells on either side of thin, slanted
geometry. Cell shapes are distributed like Umbra's, so the difference is how Umbra's tree resolves
those boundaries: handed to the external research as Q1 of the tome-size research request (external).

## 4. Benchmark: the 0.125-0.25 m hole band is untested

**Done 2026-10-12 (generator 2).** Slits 0.1, 0.15, 0.2, 0.25 and 0.4 m with sub-voxel centre
offsets. Both bakers now miss objects through a hole behind the 0.15-0.25 m slits (Umbra 2 per
tile size, Tenebrae 1 from 16 m up), so the hole truth's "must be 0" holds only from 2 SH; feeds
research B (does Umbra test holes below a voxel at all?).

`thin_walls` slits are 0.1 m (closed by both bakers) and 0.4 m (open). Add 0.15, 0.2 and 0.25 m
slits off the voxel lattice; bump `GENERATOR`, regenerate the Umbra reference and the baseline.

## 5. Size optimizations

- **8 m blocks (measured 2026-10-12, not yet the default).** The preset seeds cells in 6 m blocks
  (`CellMode::Merged { block: 6.0 }`), but Umbra's cell level, `T / 2^round(log2(T / SO))`, is 8 m
  at every benchmark tile size. Bytes with 8 m at 8 / 16 / 32 / 64 m: `bistro` -30 / -26 / -25 / +3 %,
  `city` -16 / -21 / -35 / -34 %, `terrain` -27 / -31 / -43 / -41 %; at 32 m `rooms` -45 %; at 32 / 64 m
  `degenerate` +14 / +38 %. Fewer cells (Bistro 1,914 to 1,270 at 32 m), so culling
  changes: needs the evaluation harness and parity before it replaces 6 m. 4 m blocks are
  worse almost everywhere (`instanced.t64` +211 %).
- **Portal coalescing: dropped.** Tenebrae has fewer portals per cell than Umbra (Bistro at 32 m:
  6.0 against 8.2); the portal bytes follow the cell count, and with 8 m blocks they match Umbra's.
- **Object renumbering: not possible for the engine.** Object runs are 5.6 times Umbra's entries on Bistro,
  but `Params::tome_a` keeps object `i` = target `i` for the bubble's object-indexed tables.
  How Umbra builds per-cell lists is Q3 of the tome-size research request (external).
- **Size-aware merge:** item 1 adds cells; keep bytes in check on `city`/`terrain` at 32/64 m,
  where Tenebrae is already larger than Umbra (city.t64 209 KB against 110 KB).

## 6. Benchmark infrastructure

- Record bake time and peak working set per case (report only, not gated).
- M5 portal query: score false culls and kept objects in-repo, so items 1-3 iterate without the external harness.

## 7. tome-size research handover (done 2026-10-12)

Source: the tome-size research answers (external) (rev 2).
Numbers: `bench/README.md` results and the README M3b row.

- **View trees (Q1).** The handover's "6 m blocks are not dyadic" was not the cause (8 m blocks
  still left 300k nodes on `bistro.t32`). Comparing Umbra's own Bistro tome tile by tile was:
  a tile with 86 cells has 12 view leaves, and above the street-level view volume Umbra's trees
  resolve nothing, because strict view volumes (the engine's flag 4) leave cells the camera cannot enter
  out of the tree. `Params::view_mask` makes space more than a voxel outside the view volume
  "don't care" (view nodes 359k to 65k, Umbra 39k); Umbra's export simplify (solid as don't-care,
  identical children, solid half, axis rotation) runs on every L4 tree. Contract v2:
  `Report::cell_points` is optional, `None` for a cell no sample of which resolves to it.
- **Merge cost (Q2).** Lever L2 is now Umbra's `CellMerger` (mode 1 box growth with the
  200-sample opening-visibility cost, mode 2 pairs with 1,000 samples, both against `R²`). It only
  works with exact openings: one bounding rectangle per portal spanned wall and doorway, every
  segment "passed", and rooms merged. Cells start as 8 m components with no fill merge (Umbra's
  pipeline; `CellMode::Components` keeps the cell cap as a safety net). `rooms.t32` then kept
  exactly Umbra's 3,703 before portal coalescing.
- **Portals (Q4).** `grid::coalesce` is `rect_coalesce`: per (cell, side, neighbour, plane), the
  cheapest of the bounding rectangle and the 4-quadrant splits at every corner, area + `R²/2` per
  rectangle in a tile, `(R/4)²` across seams. More exact portals cull more: `rooms.t32` 2,193,
  Bistro 111,092 (Umbra 113,006).
- **Objects (Q3).** Renumbering stays off for the engine. `Params::share_lists` packs lists into shared
  windows (Bistro 8,224 to 6,918 run entries, city 4,428 to 1,779).
- **Beyond-domain targets (follow-up).** `Params::domain_margin` (opt-in) grows the domain over
  targets past the view volume, as Umbra's scene-bounds grid does: `city.t32` 3,297 to 2,797 kept
  (Umbra 2,832) at +12 % bytes, but +37 % bytes over the whole benchmark, so it is off by default.

Open (`deferred-work.md`): Sponza keeps 11 % more than Umbra with 8 m blocks; Umbra keeps several
cells in open `seams` tiles where Tenebrae merges to one; the parity scene keeps 423 per view against the
old 377 (the price of Umbra's own parameters; `cell_merge` 3 m gives 394 at 893 KB).

## 8. Query cost in the engine's runtime (done 2026-10-13)

**Symptom.** In the engine's native runtime (the engine's native-runtime harness, portal call only, median of the best of 5
per view) Tenebrae's tomes cost the same as Umbra's overall, but up to 2.4x at 8 m tiles on dense
scenes (`terrain.t8` 152 µs against 70, `city.t8` 89 against 53).

**Reading.** The runtime's statistic counters (`runtime::count`, `s + 104`) per query, both tomes:
our tomes visited more tiles at 8-16 m, finished 3-6x the cells, and re-queued finished cells
(counter 6), which never happens on Umbra's tomes.

- **Portal order.** The runtime's cell sort reads a cell's portals
  only up to the first one leaving the tile; Tenebrae listed them by key, so internal portals
  often came after external ones and the sort never saw them. Internal first: re-queues gone,
  cells finished down up to 30 %, native time -3 % (up to -15 %), every answer unchanged.
- **More merging in open space (lever 2): revoked.** `cell_merge` 8-12 m or 16 m blocks speed up
  only 32-64 m tiles (about 7 % overall) and keep 4.7-7.5 % more objects (`city.t64` +27 %,
  `sponza` +17 %), where the defaults are at Umbra's culling (684,112 kept against 681,500).
- **Inner-node LOD tiles (lever L5, `bake/lod.rs`).** The cause at 8-16 m: stripping Umbra's
  (`edit strip-inner`) makes its tomes slower than ours (`thin_walls.t8` 44 to 174 µs) or status 2.
  - Runtime (port `walk.rs`, `lod_push`): a node is walked into while the camera's box distance
    is at most `max(accuracy, base_distance)` times its `+0x9C` entry; otherwise its own tile is
    traversed. `+0x9C` on Umbra's tomes is `cbrt(node volume) / base_distance` for every node.
    A portal into another tile is taken only when that node is selected, and Umbra's leaf tiles
    list each opening towards the neighbour at every level (leaf and the coarse tiles above it).
  - Optimizer (`umbraoptimizer64_336.dll`): `lod_buildTile` pools sibling
    subtrees bottom up (concatenates their graphs, portals between them become
    internal) and builds a tile once 5 tiles pooled and the coarse cells are at most 0.7 of the
    pooled cells (2,047 at most); mode 3 is union-find over the graph's
    portals, dropping components without external portals. Before merging, `lod_buildTile`
    removes objects from coarse cells through by a distance test on a per-object
    float that must be non-negative (unread; possibly a draw distance). Tenebrae keeps every
    object, which is conservative.
  - Open: Umbra's coarse tiles hold more cells than plain components give (about 5 per 32 m city
    tile against 1). Components over the whole node kept up to 21 % more objects (`city.t16`);
    joining only within each quarter of the node (two levels down) brings every case within
    7.4 %. Where Umbra's extra splits come from was not found.
  - Measured (benchmark, native, 2026-10-13): portal queries 11 % faster (23 % at 8 m, 15 % at
    16 m, 3 % at 32 m, none at 64 m) and 13 % faster than on Umbra's own tomes; our port equals
    native on 8,000/8,000 views; objects kept +0.2 % (worst +7.4 %), false culls unchanged,
    tomes +26 %. The parity setup: 1,106,272 B (667,168 B without; Umbra 1,284,240 B),
    status 0 on 586/586, 0 truly visible culled, 424 kept per view (423), median native query on
    the 1,029 captured engine cameras 147.8 µs (169.0 µs without, stock tome 136.4 µs).
- **Still slower than Umbra:** `bistro` (155 µs against 131 at 8 m) and 64 m tiles (`terrain.t64`
  52 against 32), where our tiles hold more cells (6.4 against 3.6 per tile). Coarser cells there
  cost culling (above).

## 9. The parity scene: the 9 % native query-time gap to the stock tome

**Symptom.** Median native portal query on the 1,029 captured engine cameras: Tenebrae 147.8 µs
(item 8 setup, the item-8 tome), stock 136.4 µs; the Unity rebake on the research recipe lands at
112-134 µs. No hidden occluders (external research answer 1); the gap is object work and cell shape.

**Sources.** The query-gap research answers (external) (answer 1), the query-gap research answers (part 2) (external)
(answer 2, 2026-10-13; corrects three points of answer 1), the requests beside them. Scripts:
The item-9 research scripts (external); tomes the external output directory.

**What external research established (answer 2, tags RE / BAKE / TOME).**
- Cost model over 4,116 queries (R² 0.92): 8.9 µs + 0.17 per portal + 0.019 per object
  candidate + 0.029 per visible object (tiles 0.09, cells 0.03). Stock's lead is 215 fewer
  candidates per query (single objects with tight lists) and 89 fewer visible.
- **Grouping (Q1).** The exact grouping does not matter, the statistics do: D13 (one object per
  user, 132 µs) regrouped by stock's own map (115 µs) or by Umbra's `ObjectGrouper` at OGC 4 /
  cost 40,000 (113 µs) both beat stock (134 µs on that bench) by 19-21 µs. Grouping leaves
  cells, portals and tiles unchanged and the group's list is exactly the union of the members'
  lists. Stock keeps singles **by size**: nothing under 1 m single, 2-9 % at 1-4 m, 39 % at
  4-8 m, 76 % at 8-16 m, 91 % over 32 m (statics 100 %); 6,342 users into 2,245 singles + 1,140
  groups (459 pairs, 268 triples, ... 24 of 9-12). The grouper is read in full (Q1.2): a
  surface-area agglomerative BVH over the 32 nearest union boxes, pair cost
  `(A_world − A_union)·OGC + (cost_i + cost_j)·A_union`, then a saturation cut (primal-dual) that
  keeps a leaf single when its own cost is the tightest on its chain.
- **Volume SO (Q2).** Stock's volumes were **SO 3** and the developer set **no** base distance
  (`Builder_finish` defaults it to 4 × min feature = 12). Under the override the tile's merge
  feature is SO_vol itself (R = 3 for the leaf `CellMerger`); the block level is
  `T / 2^round(log2(T / SO))` = 4 m. Tiles overlapping no view volume are built at
  `SO_global × 2^k` (12 m for the 64 m layer), and skipped when they hold no triangles, targets
  or gates; that is where stock's 3.4 cells per tile below the layer come from. Stock's
  in-layer tiles (30.7 cells per 32 m tile, 25,130 leaf portals, 303 m³ median cell) sit
  between a whole-layer SO 3 bake (35.1) and SO 5 (26.6): the stock volumes were several boxes
  smaller than the layer (`stock_view_volumes.json`, 18 boxes, in the tome folder). Per-volume
  SH and BF overrides change no leaf cell.
- **Inner tiles (Q3).** `lod_buildTile`: pool the children's graphs; attempt once `units ≥ 5`;
  merge with `CellMerger` **mode 6** (not 3: `HIERARCHY_DETAIL` defaults to 1, so R = F =
  **cbrt(V_tile) / 4** metres, independent of the feature size); keep iff cells ≤ 2,047 and
  (forced, i.e. a leaf among the pooled, or cells ≤ 0.7 × Σ children). Predicts every inner
  tile of D11 (276/276) and stock (264/264); no tile was ever rejected. The object filter is a
  no-op (it keys on per-object draw distances the engine does not set). Mode 6 is **top-down**: sample
  N Halton lines through the tile's bounding sphere (N = clamp(A/R² × 100, 10k, 1M); A = sphere
  area), find the entry and exit cells, BFS through portals the segment crosses; a sample whose
  exit is not reached is occluded. Start with one group per connected component (cells no
  sample reaches are dropped), then repeatedly split the group and axis-aligned portal plane
  that stops the most occluded samples from leaking through the group graph, while a split
  stops more than `k0 = ceil(R²·N/A)` samples. 64 m cube: R² = 256, N ≈ 15k, k0 = 100.
- **Tile `level` (`+0x48`)**: leaf = SH × shortest edge / T, inner = min of its children.
  Tenebrae writes 0.125 everywhere and `lod.rs` takes the max.
- **`+0x9C`**: `cbrt(V) / base_distance` for every node (= `cbrt(V) / (4 × min feature)` by
  default), as Tenebrae writes it.

**Steps**, cheapest first, each checked on its own counts (cells per in-layer 32 m tile,
leaf portals, median cell volume, inner-tile cells, candidates and visible per query with
the research cost script) before the native median on the 1,029 records (the evaluation harness).

1. **Settings, no library code.** View volume = playable layer `(-128,-160,-64)..(160,128,0)`,
   domain grown over the targets to the parity box (`Params::domain_margin`), 4 m blocks
   (`components4`), `cell_merge` 3, base distance 12. `bake.exe` gains the positionals
   `base-distance` and `domain-margin` (25, 26). Targets stay the 3,385 stock boxes. Counts to
   match: 30.7 cells per in-layer tile, 25,130 leaf portals, 303 m³.

   **Done 2026-10-13, with a different outcome than planned.** The recipe as stated made the
   tome worse: 8,158 cells, 42.0 per in-layer tile, 94,216 leaf portals, native median 322 µs
   (stock 146 in that run). Isolated with the research SO script / the evaluation harness (tomes `fg9_s1*` in
   the external output directory; stock in the same run 165-171 µs, the machine drifts between runs):

   | blocks / R | cells | per in-layer tile | leaf portals | median µs | kept |
   |---|---:|---:|---:|---:|---:|
   | stock | 2,760 | 30.7 | 25,130 | 165-171 | 330.6 |
   | 8 m / 6 (s1b) | 2,215 | 13.3 | 23,393 | 173.5 | 419.4 |
   | 8 m / 3 (s1d) | 3,074 | 18.2 | 36,551 | 222.2 | 391.1 |
   | 4 m / 6 (s1c) | 5,360 | 28.4 | 62,877 | 347.7 | 373.9 |
   | 4 m / 3 (s1) | 8,158 | 42.0 | 94,216 | 322.1 | 346.5 |

   Three readings. (a) The view volume and base distance change nothing: s1b equals the parity
   box bake (s1e) in cells and time. (b) Tenebrae's leaf merger does not take 4 m seeds back
   to Umbra's counts: at 4 m / 6 the in-layer count matches stock but the tome walks twice
   the portals and cells per query (1,120 / 249 against 502 / 133), so finer in-layer cells
   are not a lever for Tenebrae as they are for Umbra. (c) **The lever was outside the
   layer**: stock has 3.4 cells per tile there (Umbra builds a tile overlapping no view volume
   at feature `SO × 2^k`, Q2.3) and Tenebrae had 9-14. Implemented as `tile::level_shift` /
   `Ctx::feature` / `Ctx::block_of`: with `cell_merge`, such a tile merges
   at `R × 2^k` and seeds at Umbra's cell level for that feature (12 m and 16 m blocks for
   the 64 m layer at 6); keyed in the tile cache (version 6). Result at 8 m / 6 with the layer
   volume and margin 640 (`fg9_s1f`): 1,561 cells, 882,112 B (stock 976,304), 406 portals and
   114 cells per query (stock 502 / 133), kept 419.6, 0 wrongly culled, **median 175.1 µs
   against stock's 171.3 in the same run**. The remaining difference is objects: 1,131
   candidates and 420 visible per query against 836 / 331, i.e. step 4.
2. **Tile `level` rule** (`global.rs` `emit`, `reference.rs`, `lod.rs`): as above. **Done**
   (`tile_level`).
3. **Mode 6 inner tiles** replace the union-find in quarters (`lod.rs`, `SPLIT_LEVELS`): the
   Halton sampler, face-cell lookup, portal-crossing BFS (lever L2's test), link graph with
   `rect_coalesce(R²/2)`, the split search, F = cbrt(V)/4, the 5 / 2,047 / 0.7 acceptance with
   `forced`. Match D11 node 97 (the research LOD script: 17 pooled cells, 4 components, 7 cells,
   kept) and the per-tome inner-tile cell counts (cells == components on 22/54 tiles).
4. **Per-user targets, grouped about 2:1 by size.** The biggest lever (~18 µs). parity-scene input
   from the research user-box export (6,342 users with boxes; statics as their instance triangles). Tenebrae's
   own grouper on the M4 contract's user lists: port Q1.2 (BVH + saturation cut) with uniform
   cost 40,000 at OGC 4, or any rule that reproduces the Q1.3 size profile (~3,400 objects).
   Lists and boxes are unions of the members'; group **after** the per-user lists are built.
   Verify: objects ≈ 3,400, candidates per query within ~10 % of stock's 836, native median at
   or under stock's.

   **Done 2026-10-13 (`Params::group_cost`, `bake/group.rs`); the count
   matches, the time does not.** The evaluation harness with `TENEBRAE_TARGETS=users` bakes
   one target per engine user (6,342 boxes from the research user-box export) and writes `TOME.users.json` for
   the evaluation harness. Same cells as s1f (1,563); per query (the research cost script, one run):

   | tome | objects | candidates | visible | kept (stock units) | mean µs | median µs |
   |---|---:|---:|---:|---:|---:|---:|
   | stock | 3,385 | 836 | 331 | 330.6 | 153.6 | 139.9 |
   | s1f, stock boxes | 3,385 | 1,131 | 420 | 419.6 | 162.8 | — |
   | s4a, per user | 6,342 | 1,846 | 679 | 445.9 | 195.3 | 184.9 |
   | s4b, per user, cost 10,000 | 3,523 | 1,131 | 430 | 466.7 | 161.4 | 158.1 |

   Grouping recovers the 34 µs that per-user targets cost (s4a to s4b) and lands exactly on
   the stock-box bake's candidates, so the grouper is right and the ~18 µs external research measured is
   already in s1f. Stock's 836 candidates are **list tightness**, not object count: its
   per-object lists cover 0.89 of the union volume for single objects (answer 2, Q1.4) because
   its in-layer cells are finer (30.7 per tile, 303 m³ median against our 13.3 and 1,024 m³),
   so each cell overlaps fewer objects. Finer in-layer cells are the one lever left, and
   reading (b) above is what blocks it: Tenebrae's L2 merge at 4 m seeds gives stock's cell
   count with twice the portals walked. Open, as a research item: why Umbra's 4 m-seeded
   cells cost half the portal traffic of ours at equal count (cell shape, portal count per
   cell: stock 9.1 per cell against 11.7 for s1c; which cells the runtime enters).

**Status 2026-10-13.** Gap closed from +20 % (147.8 against 136.4 at item 8) to +5-13 %
(cost-model means 161 against 154; medians 158 against 140 in the s4 run, 175 against 171 in
the s1f run; the machine drifts about 15 % between runs, so compare within one). The parity scene
recipe now: `the evaluation harness 32 0.125 components8 flood -128 -160 -64 160 128 0 16 0.1
32 4096 1 6 1 1 "" 0 1 12 640 [10000 with TENEBRAE_TARGETS=users]`. Step 3 (mode 6 inner
tiles) is not started; its value is bounded by cells per query, where Tenebrae is already
below stock (114 against 133), so it ranks after the in-layer cell question above.

**Verify.** Engine evaluation: the evaluation harness with the step's settings, then the evaluation harness and
the research cost script on the tome (`TOME.map.json` from the driver). In-repo: benchmark unchanged or
re-baselined where a lever changes defaults.

## Research (engine analysis)

- **A.** What the status-2 budget counts (visits per tile, portals entered, global) in
  `umbra_native`.
- **B. Answered 2026-10-12: no defect; the misses are the voxel band.** Both bakers mark a 0.125 m
  voxel solid when a triangle overlaps it (exact box test; Umbra `the engine routine`) on the same
  world-aligned lattice, so an opening stays open only if a face-connected chain of whole open
  voxels runs through it: guaranteed from 2 voxels (0.25 m) for an axis-aligned slit, about
  2·sqrt(3) voxels (0.43 m) for a sight line at 45° in 3D. Widest clear beam (probe: centre plus 8
  edge rays) through each through-a-hole miss on Bistro at 32 m: Umbra's 25 all at most 0.19 m;
  Tenebrae's 40 are 36 at most 0.19 m and 4 at 0.25-0.375 m, all four eyes inside the bistro
  looking 12-20 m out through window frames on oblique lines (cameras 54, 163, 177). The sets do
  not overlap; Umbra keeps Tenebrae's (it keeps about 5 % more objects, looser cells or portals)
  and Tenebrae keeps Umbra's. Closing the band would need a sub-voxel opening test (beyond Umbra);
  not pursued. `thin_walls` (generator 2) shows the same band for both bakers.
- **C. Answered 2026-10-12.** Before the item 3 fix, at 32 m: 65 Tenebrae-only and 25 shared
  through-a-hole misses, Umbra had none of its own from 16 m up. Every Tenebrae-only miss's
  beam-clear sight lines cross surface voxels (voxel closure, B); two apparent all-open lines were
  a probe artifact (offset rays ending outside the object's grown box). Umbra at 8 m has 158 misses
  of its own whose sight lines cross no Tenebrae solid voxel.

## Doc fix (done with item 1)

`bench/README.md` says Tenebrae culls more than Umbra on terrain, maze, city and bistro at 16 m and
up. It does not on `city.t32` (3,336 against 2,832), `maze.t16` (3,067 against 2,855) and
`bistro.t64` (113,590 against 112,991); only `terrain` holds throughout. Fix with item 1's
re-baseline.
