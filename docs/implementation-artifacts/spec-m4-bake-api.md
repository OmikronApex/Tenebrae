---
title: 'M4 — consumer-facing bake API (contract v1)'
type: 'feature'
created: '2026-10-12'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context:
  - '{project-root}/docs/planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md'
  - 'the consumer's bake-API request (external)'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The consumer wants to re-bake tome A (and maybe B) of a bubble it edits: to remove the ghost
occluder after a stock edit, to give a clone its occlusion culling back, and to make added statics
occlude and be culled (the consumer handover request, 2026-10-12, §2). The crate's public items are not a
contract (AD-10), the bake takes a pre-transformed triangle soup, it renumbers objects when
reachability drops a target, the incremental cache is a typed value rather than bytes, there is no
progress or cancel, and the Umbra half of the cell transfer that rebuilds the dependent tables
lives in an external research Python script.

**Approach:** Freeze one bake entry point as **contract v1**: instanced or soup occluders, targets
that carry their own user lists, an identity mode that never drops a target, engine presets, cache in
and out as bytes, a progress hook with cancel, and a `Baked` result with the object map,
representative cell points and counts. Add the cell transfer and dequantised cell/portal geometry
to `query`. Answer the request's six questions with evidence. The per-camera visibility query
(request §3.7) is not part of v1 (decision D3).

## Decisions (ruled by the user 2026-10-12: every recommendation adopted)

| # | Decision | Ruling | Why it was the user's |
|---|---|---|---|
| D1 | **Amend AD-6**: accept instanced occluders (`meshes` + `(mesh, 3×4)` instances) and transform them inside the bake. | Yes. Avoids ~330 MB of materialised soup per map in the consumer; the cache key hashes world-space vertex bits, so it is unaffected (Q4). | AD-6 says "Tenebrae has no notion of instances". |
| The engine | **Amend AD-8**: `Params::keep_every_target` (object *i* = target *i*, always) and targets carrying a user list instead of one `u32`. | Yes, with the flag off by default (Umbra's behaviour stays the default; `tome_a/b` turn it on). | Changes the object-identity invariant. |
| D3 | **Visibility query** (camera + frustum → visible objects) out of M4, into its own milestone (M5) validated against the engine's native-runtime harness on the 586 captures. | Yes. It is a port of the runtime's portal rasteriser; a half-exact answer in a viewport would mislead. M4 ships `find_cell` and overlay geometry. | Scope of a milestone the consumer asked for. |
| D4 | **One entry point**: `bake(input, cache: Option<&[u8]>, hook)` → `Baked`; `bake_cached`, `TileCache` and `Report::object_map` leave the public surface. | Yes. Fewer contract items; the CLI moves over in the same commit (AD-10). | Freezes the contract's shape. |

## Boundaries & Constraints

**Always:**
- AD-1 to AD-5, AD-7, AD-9, AD-11, AD-12 hold. No dependencies, no `unsafe`, no I/O, no clock.
- **Byte identity, required:** with default `Params` and a soup input, the tome, samples and object
  map are byte-identical to the pre-M4 baker (pinned hashes in `l4_off_bakes_are_unchanged` plus a new
  pin at defaults). An instanced input bakes byte-identically to the soup that
  `Instance::apply` produces from it. A cached bake equals a fresh one (M3c oracle). Any thread
  count gives the same bytes.
- Progress: `total` fixed per stage, `done` monotone, hook called on the caller's thread only.
  `Control::Stop` returns `Error::Cancelled`, no partial output, within one tile's work per worker.
- A cache that is malformed, of another version, another grid or other `Params` is a miss, never
  an error; `Report` says which (`cache_hits`, `cache_rejected`).
- Everything returned indexes the tome as written (AD-11).
- `tools/bake` stays positional-compatible (AD-12); it moves to the new entry point.

**Never:**
- No engine keys, tags or record encoders in the crate (AD-8). The consumer writes its own map records.
- No change to bake algorithms, the tome format or the defaults' output.
- No hash rows, gates, plane BSP leaves or multi-volume domains (request §6).

## The API (contract v1)

```rust
// tenebrae::bake
pub struct Instance { pub mesh: u32, pub transform: [[f32; 4]; 3] }  // row-major 3x4, engine space
impl Instance { pub fn apply(&self, v: [f32; 3]) -> [f32; 3] }        // the bake's exact f32 order

pub enum Occluders<'a> {
    Soup(&'a Mesh),                                              // world space (today's input)
    Instanced { meshes: &'a [Mesh], instances: &'a [Instance] },  // mesh-local, placed per instance
}

pub struct Target<'a> { pub bounds: Aabb, pub users: &'a [u32] }   // >= 1 user; stock lists as-is

pub struct Input<'a> {
    pub occluders: Occluders<'a>,
    pub targets: &'a [Target<'a>],
    pub view_volume: Aabb,          // also the domain, snapped outward to whole tiles
    pub seeds: &'a [[f32; 3]],
    pub params: Params,
}

impl Params {
    pub fn tome_a() -> Params;   // = default() + keep_every_target: SH 0.125, the parity setup
    pub fn tome_b() -> Params;   // tome_a() with voxel 0.5 (stock "SH 0.500"); see Q2
    // new field: pub keep_every_target: bool   (default false = Umbra's drop rule)
}

pub enum Stage { Prepare, Keys, Flood, Join, Cells, Global }   // #[non_exhaustive]; Keys: cached only
pub struct Progress { pub stage: Stage, pub done: u64, pub total: u64 }
pub enum Control { Continue, Stop }

pub struct Baked {
    pub tome: Tome,
    pub object_map: Vec<Option<u32>>, // per target; identity (all Some(i)) under keep_every_target
    pub report: Report,
    pub cache: Option<Vec<u8>>,       // Some when a cache was passed in (empty slice = cold start)
}

pub fn bake(
    input: &Input<'_>,
    cache: Option<&[u8]>,
    progress: Option<&mut dyn FnMut(Progress) -> Control>,
) -> Result<Baked, Error>;

pub enum Error { Params(&'static str), TooLarge { needed: u64, limit: u64 }, Cancelled }

// Report gains (existing fields kept):
//   cell_points: Vec<[f32; 3]>  per global cell, one point find_cell provably resolves to it
//   global_cells, global_portals, objects, dropped_targets, sealed_targets: usize
```

```rust
// tenebrae::query (imports only tome; AD-2 holds)
pub struct CellVotes { pub votes: Vec<(u32, u32)>, pub nearest: Option<u32> }
pub fn transfer(old: &Tome, points: &[[f32; 3]], samples: &[Vec<[f32; 3]>]) -> Vec<CellVotes>;
    // per new global cell: (old global cell, sample count) ascending by old cell; for a cell with
    // no vote, the new cell with votes whose point is nearest (ties: lowest index)
pub fn global_cell_boxes(tome: &Tome) -> Vec<Aabb>;            // dequantised +0x80 boxes
pub struct PortalRect { pub from: u32, pub to: Option<u32>, pub axis: u8, pub rect: Aabb } // flat on axis
pub fn global_portals(tome: &Tome) -> Vec<PortalRect>;          // dequantised +0x84
// existing: find_cell, cell_regions, leaves
```

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Defaults, soup | today's tests and CLI | byte-identical to the pre-M4 baker | N/A |
| Instanced | meshes + instances; soup = `apply` of each | identical bytes to the soup bake | N/A |
| Instance index | `mesh` out of range, non-finite transform | — | `Error::Params` |
| Triangle count | instanced total over `u32::MAX` | — | `Error::Params` before allocation |
| Keep every target | a target whose cells reachability removes | object kept with no cells; map is identity; `sealed_targets` counts it | N/A |
| Drop (default) | same scene, flag off | dropped as today; `dropped_targets` counts it | N/A |
| Users | target with 0 users | — | `Error::Params` |
| Users | stock multi-user lists | `tome.users` equals the stock lists (as `edit graft-users` gives) | N/A |
| Target outside volume | box reaching past the domain | kept; also listed in every cell with a boundary portal on that side (pre-M4 behaviour) | N/A |
| Occluder outside volume | triangles past the domain | clipped (ignored past the grid) | N/A |
| Cell points | every global cell of every test bake | `find_cell(tome, cell_points[g]) == Some(g)` | N/A |
| Cancel | hook returns `Stop` at stage k | `Err(Cancelled)`, nothing returned | typed |
| Progress | 1 and 16 threads | `done` monotone, ends at `total`, same totals | N/A |
| Cache | `Some(&[])`, bad bytes, other tile settings | full bake, identical; `cache_rejected` set only for bad bytes; hits + misses = grid tiles | ignored |
| Transfer | old = new tome, samples = report's | every cell votes only for itself | N/A |
| Presets | `tome_a()` vs `Params::default()` | same output except identity mode | N/A |

</frozen-after-approval>

## Answers to the consumer's request

- **§3.1 AD-9 gaps — already closed** at the pre-M4 baker: `bake::Error` has `Display` and
  `std::error::Error` (`src/bake.rs`), `validate` checks triangle indices and `owners` length, grid
  sizes use checked arithmetic with `Error::TooLarge { needed: u64::MAX }` on overflow, and
  `tome::Error::Version` prints "tome version N, expected 18". The spine's gap list is stale; M4
  removes it. The memory ceiling exists: `Params::memory_limit` (default 4 GiB), checked before any
  brick is allocated; it does not count the tile caches.
- **§3.2 occluder rule (to be cited as *the* rule):** the statics blob's instances in
  **kind-7** groups; per static every mesh part with primitive type 3 (triangle
  list) of the group's buffer set, **except LOD 10**; plus terrain tiles (layout 61, terrain vertex shader), **detail 0** parts only. Source: the evaluation harness and
  the evaluation harness. Kind 2 (no prepass/shadow stage) is not solid. Copied into the README.
- **§3.2 volume:** the domain is the view volume snapped outward to whole tiles (32 m). Occluders
  past it are ignored (except by parity rays), seeds past it are ignored (a flood bake whose seeds
  are all outside fails with `Error::Params`), and a target reaching past it is kept and also
  listed in every cell with a boundary portal on that side: a sight line to its outer part leaves
  the domain there.
- **Q1 identity:** yes. A target reachability would drop is kept as an object listed in no cell.
  That is already how every **buried** target is emitted (AD-8, `global::reachability` maps a
  never-touched target to `Some`). Umbra itself keeps a sealed room's target
  (the evaluation harness). The parity scene's bake has 2 buried (zero-cell) objects and answers
  586/586 captured views with status 0 (Implementation Notes).
- **Q2 tome B:** stock B's objects are **not** A's. In all six installed pairs the counts differ
  (the parity scene 3,385 / 3,422; the second stock map `0729`/`072e` 7,523 / 7,476; the
  others 1,961/1,963, 6,964/6,976, 6,376/6,388, 5,217/5,235; `stock_tomes_answer_queries`), so
  `…12` is a real A→B remap today. Baked from A's targets with `keep_every_target`, a new B
  numbers its objects exactly as A, so `…12` would become the identity, but the consumer must then write
  it; leaving B stock leaves `…12` stock. Every stock header differs between A and B only in
  `SH` (0.125 / 0.500), so `tome_b()` is `tome_a()` at 0.5 m voxels (without LOD tiles since 2026-10-13, to stay
  under stock B's size). Its parity was
  measured on 2026-10-13 (README, Contract v2 › Presets): 586/586 status 0, 0 truly visible culled. **When the client uses B, and what `…12` maps (A objects to B objects, or
  something keyed differently), are external research questions** — forwarded in the response.
- **Q3 representative points:** a guarantee under every lever setting, not just TB1's. Every
  `cell_samples` point is the centre of an open voxel of that global cell, and
  `assert_samples_find_their_cells` (`src/bake/tests.rs`) checks centre and ±0.45-voxel corners
  through `query::find_cell` on L3 and L4 bakes. `cell_points[g]` is the sample nearest the mean
  of `g`'s samples. The deferred the evaluation harness concern is **void**: `.samples.bin` is per
  global cell, and `find_cells` goes through `cells.exe` → `query::find_cell`, which returns global
  cells (`global_of`), so both sides are global.
- **Q4 instancing vs cache:** compatible. Pass-A keys hash the world-space vertex bits of each
  tile's bucketed triangles, so an instanced bake computes the same keys as its soup. A moved
  instance changes only the tiles its old and new triangles touch (plus pass-B neighbours); an
  unrelated instance of the same mesh still hits.
- **Q5 the second stock map:** not measured. Task below; the number goes in the response.
- **Q6 clone domain:** grow the view volume to cover every camera-reachable place of the appended
  content. Cells dequantise against the tome's own domain, so nothing in the tome assumes stock's
  box. Downstream, the light-bake probes are world points and are reached through `transfer`, so
  they survive a different domain; cells outside the old domain get no votes and fall back to the
  nearest voted cell. A grown volume changes the grid, so the first re-bake after it misses every
  cache entry (M3c decision).

## Code Map

- `src/bake.rs` — `Input`, `Target`, `Params`, `Report`, `Error`, `validate`, `prepare`
  (triangle bucketing, `corners(input, ti)`), `run_tiles`, `bake_with`. `bake_cached` folds into
  `bake`.
- `src/bake/tile.rs` — `corners`, `tri_ranges`, `clip`: the only readers of occluder vertices
  besides parity's BVH.
- `src/bake/global.rs:817` `reachability` — the drop rule and object map; `emit` fills users and
  counts.
- `src/bake/cache.rs` — `Keys::a` hashes triangle vertex bits (must read through the same
  `corners`).
- `src/query.rs` — `find_cell`, `leaf_at` (recomputes ranks per call: add a prepared locator for
  `transfer`), `cell_regions`.
- `src/bake/global.rs:313,346` `quant`, portal words — inverse for `global_cell_boxes` and
  `global_portals`.
- `tools/src/bin/bake.rs`, `edit.rs`, `cells.rs` — callers.

## Tasks & Acceptance

**Execution:**
- [x] `src/bake.rs` -- `Occluders`, `Instance::apply`; triangle ids become a prefix-sum space over
  instances; `validate` checks mesh indices, transforms and the `u32` total. Parity mode
  materialises the soup for its BVH and counts it in the estimate.
- [x] `src/bake.rs`, `global.rs` -- `Target::users`, `keep_every_target`, `emit` writes multi-user
  lists (prefix only when some object has ≠ 1 user).
- [x] `src/bake.rs` -- `bake(input, cache, progress)` → `Baked`; workers report finished tiles
  over an `mpsc` channel to the caller's thread, which calls the hook; a stop flag workers read
  before each tile. One thread calls the hook inline.
- [x] `src/bake.rs` -- `Report::cell_points` and counts; `tome_a/b`.
- [x] `src/query.rs` -- `transfer`, `global_cell_boxes`, `global_portals`, prepared locator.
- [x] `tools/` -- bake, edit, cells on the new API; positionals unchanged.
- [x] Tests -- one per matrix row; defaults pinned against the pre-M4 baker.
- [x] Spine -- amend AD-6, AD-8 (per D1, the engine), AD-9 (gaps closed), AD-10 (v1 is the contract),
  Deferred (resolved items). Deferred work: close the evaluation harness entry.
- [x] README -- M4 row, "**Contract v1** at `<commit>`", the occluder rule, presets.
- [ ] Measure the second stock map (the evaluation harness scene, stock tome-A
  box, defaults, 16 threads): fresh, cached unchanged, one-prop edit, peak WS/private, tome bytes.
  **Blocked:** both scene builders are parity-scene-only (hard-coded tags); requested from the consumer as an
  instanced export (its §5 offer) in the response.
- [x] `the bake-API handover response (external)` -- the answers above
  plus measurements, for the consumer; external research question on tome B usage and A/B object correspondence.

**Acceptance Criteria:**
- Given default `Params` and a soup, when baked through the new entry point, then the output is
  byte-identical to the pre-M4 baker.
- Given an instanced scene, when baked, then the bytes equal the bake of its `apply`-ed soup, at 1
  and 4 threads, cached and fresh.
- Given `keep_every_target` and a scene where reachability removes a target's cells, when baked,
  then `object_map[i] == Some(i)` for every `i` and the target is listed in no cell (unit test);
  and given `tome_a()` on the parity scene, whose zero-cell objects the runtime meets the same way, the
  tome reads and queries with status 0 (the engine's native-runtime harness on the parity views).
  (Amended in review, the engine.)
- Given any test bake, then every `cell_points[g]` finds `g`.

### Review Findings

Code review 2026-10-12 of the M4 change (Blind Hunter, Edge Case Hunter, Verification Gap,
Acceptance Auditor).

- [x] [Review][Decision] Ratify the frozen-matrix edits — resolved: 0 users refused, other three ratified (Change Log D1). — The agent rewrote rows inside
  `<frozen-after-approval>` (0 users: `Error::Params` → stored empty list; target outside volume:
  "no cells" → listed in boundary-portal cells; `PortalRect::to` → `Option<u32>` + `axis`; cache
  row). The Change Log says "flagged to the user", but no human sign-off is recorded. The frozen
  API still says `users: &'a [u32] // >= 1 user`, contradicting the edited row; `validate` accepts
  `&[]` and `targets_carry_their_user_lists` bakes one. Either ratify (and fix the `>= 1 user`
  comment) or restore the frozen rows and refuse 0 users in `validate`.
- [x] [Review][Decision] AC3 never met to the letter — resolved: AC3 amended (Change Log the engine). — no tome with a *sealed* kept target has run
  through the engine's native-runtime harness. The parity scene has no sealed target (Implementation Notes), so the native
  run is byte-identical to the default bake; `keep_every_target_keeps_identity` uses
  `sealed_pocket_input`, not `tome_a()`, and no native check. The argument that the parity scene's 2
  buried zero-cell objects already exercise "object listed in no cell" is sound but is an
  argument, not the measurement the AC asks for. Accept the argument (amend AC3) or require a
  synthetic sealed scene written out and run through the engine's native-runtime harness.
- [x] [Review][Decision] Cancel latency is not bounded by one tile everywhere — resolved: `Stage::Keys` added (Change Log D3). — Pass-A key hashing
  (`run_tiles(.., None, ..)` plus `Keys::new` over the whole soup) on cached bakes, and the parity
  soup materialisation + `Bvh::build` after the final `Prepare` report, never call the hook, so
  `Control::Stop` cannot act there (Always: "within one tile's work per worker"). Fix needs a
  contract choice: fold hashing into `Prepare`'s total, add a `Stage` variant, or amend the rule
  to name these as uninterruptible. [src/bake.rs:1139]
- [x] [Review][Patch] Test the `u32::MAX` triangle-total refusal — `instanced_triangle_totals_are_checked`
  and `invalid_instances_are_errors` claim matrix row "Triangle count" but only build totals that
  fit; one ~65,536-triangle mesh (all on vertices 0/1/2) placed by 65,537 instances overflows in
  a few MB. Assert `Tris::new` is `None` and `bake` returns that `Error::Params`.
  [src/bake/tests.rs:2060]
- [x] [Review][Patch] Cover empty instances in `Tris::corners` — no test has duplicate `starts`;
  add an empty mesh placed before, between and after non-empty instances in `instanced_scene()` so
  `instanced_occluders_bake_like_their_soup` covers it. [src/bake/tests.rs:1903]
- [x] [Review][Patch] Exercise the in-loop `Prepare` progress — no test has > 65,536 triangles, so
  the periodic report (and a Stop on it) never runs; add a ~70k-triangle bake with the recorder
  hook and a stop on a mid-Prepare report. [src/bake/tests.rs:2236]
- [x] [Review][Patch] Pin samples and object map at defaults — the Always rule requires tome,
  samples and object map byte-identical to the pre-M4 baker; `l4_off_bakes_are_unchanged` hashes the tome
  only. [src/bake/tests.rs:1291]
- [x] [Review][Patch] Assert `cell_points` in the shared helper — AC4 says every test bake;
  only `cell_points_find_their_cells` (4 configs of `scene()`) checks it. Check `cell_points` in
  `assert_samples_find_their_cells` so every caller covers it. [src/bake/tests.rs:1207]
- [x] [Review][Patch] `bake` doc omits `cache_misses` from the fields a cached bake may change
  (the `Report` doc lists it). [src/bake.rs:1088]
- [x] [Review][Patch] "hits + misses = grid tiles" is false for pass A in parity mode (both 0);
  say so in `Report::cache_misses` and the handover response §3.6. [src/bake.rs:432]
- [x] [Review][Patch] Duplicate `reachability` doc — the old paragraph still ends "Returns the
  object map." above the new one; merge. [src/bake/global.rs:847]
- [x] [Review][Patch] README contract example does not compile — `Control` not imported;
  `Some(&cache_bytes)` is `Option<&Vec<u8>>`, not `Option<&[u8]>`; `meshes`, `instances`,
  `targets`, `seeds` need to be slices. This is the first code the consumer copies. [README.md:31]
- [x] [Review][Patch] README M4 row says **done** with the second stock map measurement still
  Blocked (the consumer Q5 waits on it); name it as open. [README.md:20]
- [x] [Review][Defer] Spine left stale in three places [ARCHITECTURE-SPINE.md:95] — deferred:
  edits an architecture doc. AD-6 still says "`Target::user`" (now `users`); AD-10's heading
  still reads "Public API is not yet a contract" over a body declaring v1 (line 131); Deferred
  "Visibility record ownership" says the README M4 row "stays as written", but this change
  rewrote it (line 205).
- [x] [Review][Defer] Parity-mode progress order untested [src/bake/tests.rs:2236] — deferred:
  the consumer presets are flood mode; add `SolidMode::Parity` to the progress test's loop and assert
  `Prepare, Cells, Global`.
- [x] [Review][Defer] Instanced-parity soup copy in the memory estimate untested
  [src/bake.rs:947] — deferred: estimate is practical, not strict, and the consumer presets are flood
  mode; test with a limit between the estimates with and without the copy.

**Rejected:**
- `low` `Tris::soup` vertex `base` / `users()` prefix can wrap past `u32::MAX` — needs > 4 G
  vertices (48 GB, refused by the memory estimate) or > 4 G user ids in caller memory.
- `low` Finite transform × finite vertex can overflow to inf — needs coordinates near 1e38; guard
  adds complexity for an input no map produces.
- `low` `transfer` silently skips cells when `points`/`samples` lengths differ — documented
  ("a cell without a point is never chosen"); mixing two bakes' reports is caller misuse.
- `false` `transfer` NaN points — `cell_points` are open-voxel centres, always finite.
- `low` `transfer` O(unvoted × voted) — ~50 M f64 ops on the largest tome, milliseconds.
- `low` `global_portals` drops axis-3 / out-of-range portal lists — only on a corrupt tome; stock
  tomes never carry axis 3.
- `low` `Tris::new` refuses after allocating `starts`, not inside `validate` — one
  `instances.len() + 1` vector, before any brick.
- `low` `Prepare` reports `done = ti` (65,535) at the step boundary — monotone, harmless.
- `false` Presets row not baked both ways — `Debug` equality of `tome_a()` and
  `default() + keep_every_target` proves identical params, so the output differs only by identity.
- `false` Handover's "by construction" for `cell_points` — `representative` picks one of the
  cell's own open-voxel centres, which `find_cell` resolves to that cell; tests confirm.
- `low` Matrix rows "Target/Occluder outside volume", "stock multi-user lists", "16 threads" lack
  M4 tests — pre-M4 behaviour already covered; 4 vs 16 threads exercises the same channel path.
- `low` Bake CLI cannot feed instanced scenes or multi-user targets, and shows no progress — the
  spec asked only for the tools on the new API with positionals unchanged.

## Implementation Notes

- **Shape.** `bake::bake(input, cache, progress) -> Result<Baked, Error>` is the only bake entry;
  `bake_cached`, `TileCache` and `Report::object_map` left the public surface (D4). The internal
  `bake_with(input, Option<&TileCache>, &mut Progressor)` is what the tests drive; tests keep
  thin local helpers (`bake`, `bake_map`, `bake_cached`) so the M3b/M3c suites stay as written.
- **Occluders.** `Tris` numbers the triangles once (`starts` = prefix sums over instances; a soup
  needs none) and gives world corners through `Instance::apply`; `ctx.tris.corners(t)` replaces
  every read of `input.occluders` (bucketing, voxelising, pass-A keys). Parity materialises the
  instanced soup once (`Ctx::soup`, a borrowed `Cow` for a soup input), builds its BVH from it and
  hashes it for the cache, so a soup's parity keys are unchanged. The memory estimate charges the
  copy (12 B per vertex, 16 B per triangle) only for an instanced parity bake.
- **Owners.** `Mesh::owners` may now be empty (instanced meshes rarely have them); non-empty and
  of the wrong length is still `Error::Params`.
- **Identity.** `global::reachability` returns the map and the count of targets whose every cell
  went; with `keep_every_target` those stay `Some(i)` and count as `sealed_targets`, else
  `dropped_targets`. Their cells are removed either way, so the tome differs from a dropping bake
  only in the object list and user list (test `keep_every_target_keeps_identity`).
- **Users.** `bake::users` builds `tome::Users` from the targets' lists (no prefix when every list
  has one entry), shared with the dense reference baker. Stock tomes give 15-37 % of their objects
  several users and none zero (`stock_tomes_answer_queries` prints the counts).
- **Progress.** `run_tiles` takes an optional stage. With one thread it calls the hook after each
  tile. With more, workers send `()` over an `mpsc` channel after filling their slot; the caller
  thread counts, calls the hook and on `Stop` raises a flag workers read before taking a tile,
  then drains the channel until every worker ends. No clock, no polling. Prepare reports every
  65,536 triangles; Global reports its five steps. The pass-A key hashing runs without a stage.
- **Cell points.** The sample nearest the mean of a global cell's samples (f64 distances, first on
  ties); asserted non-empty (an internal invariant: every cell has open voxels).
- **Cache counts.** `Report::cache_misses` added, so hits + misses = grid tiles per pass (the consumer
  asked for both). An empty slice starts a cache without a rejection.
- **Query.** `Locator` precomputes ranks and leaf ordinals for the top and view trees; it answers
  exactly as `find_cell` (lattice tests on bakes and on all 12 stock tomes). `transfer` uses it.
  `global_cell_boxes` / `global_portals` invert the bake's u16 quantisation; a grouped global
  cell's box is the union of its tile cells, so portals lie within, not on the face of, their
  cells' boxes. The `cells` tool now uses `Locator`.
- **CLI.** Positional 24 `keep-every-target` (0/1), append-only (AD-12). The cache summary line
  now prints hits/total from the report (the cache's entry counts are no longer public). The external research
  the evaluation harness usage texts updated.
- **Measurements (the parity setup, 16 threads, the evaluation harness, 2026-10-12):**

  | Run | Tome | Result |
  |---|---|---|
  | defaults (`m4_defaults`) | 2,147,600 B, **byte-identical** to the pre-M4 `l4_axis` / `l4_mem16` tomes, samples and map | status 0 on 586/586, 384 objects kept per view (stock 343), 0 truly visible culled; 2 buried targets, 0 dropped |
  | keep every target (`m4_keep`, positional 24 = 1) | identical to defaults (no target is dropped on the parity scene) | same |
  | reach threshold 0.99, keep 1 / keep 0 | identical to defaults | the flood leaves one connected group: no sealed target can be forced on the parity scene |
  | timing, the pre-M4 baker vs M4 `bake.exe`, alternating, same session | identical bytes | 82.8 / 85.8 s against 87.8 / 80.6 s: no change beyond noise (the machine ran slower than the 66 s of earlier sessions) |

- **Stock tomes** (12 installed, `TENEBRAE_TOMES` = the 0689 and 035A extracts): round trip 12/12;
  `Locator` = `find_cell` on a 24³ lattice per tome; every global portal links valid cells.

## Spec Change Log

- 2026-10-12, during implementation (agent; factual corrections to the frozen matrix, flagged to
  the user in the hand-off):
  - "Users, 0 users" was `Error::Params`; the user-list prefix stores an empty list as well as a
    long one (stock tomes give 15-37 % of their objects several users and none zero), and refusing
    it would be a rule of the crate's own, so an empty list is accepted and stored.
  - "Target outside volume" said "kept, no cells"; the bake (since M3) lists a target reaching
    past the domain in every cell with a boundary portal on that side. Unchanged behaviour; the
    row now says so.
  - `PortalRect::to` is `Option<u32>` (stock tomes can carry boundary portals) and carries the
    axis.
  - Cache row: a cache of other tile settings misses every tile; the row now names the counts.
- 2026-10-12, code review decisions (user):
  - D1: "Users, 0 users" restored to `Error::Params` (`validate` refuses an empty list: "target
    without a user"). The other three edits above are ratified: they describe existing behaviour,
    and undoing "Target outside volume" would break AC1's byte identity.
  - the engine: AC3 amended. The parity scene has no sealed target, so the native check rests on its 2 buried
    (zero-cell) objects, which the runtime meets exactly as a kept sealed one: an object listed
    in no cell. The sealed case itself is unit-tested (`keep_every_target_keeps_identity`).
  - D3: `Stage::Keys` added (cache-key hashing on a cached bake, counts tiles). Two single steps
    remain outside the one-tile stop bound, both parity-only: the whole-mesh key hash before
    `Keys` and the occluder hierarchy after `Prepare`'s last report (documented on `Stage`).

## Review Triage Log

## Verification

**Commands:**
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` -- clean.
- `cargo test --release --workspace` -- all pass.
- `cargo build --release -p tenebrae-tools`, then the evaluation harness
  -- parity numbers unchanged (status 0 on 586/586, 0 truly visible culled, 2,147,600 B).
