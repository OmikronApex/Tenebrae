---
title: 'M6 — native runtime (`tenebrae-runtime`)'
type: 'feature'
created: '2026-10-13'
status: 'done'
approved: '2026-10-13'
completed: '2026-10-13'
route: 'dispatch'
review_loop_iteration: 0
context:
  - '{project-root}/docs/planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md'
  - '{project-root}/docs/implementation-artifacts/spec-workspace-crate-split.md'
  - '{project-root}/docs/implementation-artifacts/spec-m5-per-camera-visibility-query.md'
  - 'the format reference (external)'
  - 'the format research notes (external)'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Tenebrae bakes tomes but cannot ship a way to query them. The only camera query in
the workspace, `tenebrae-oracle`, is a port of the engine's Umbra 3.3.6 runtime: it runs the engine's code
paths over the tome's bytes in an emulated work memory, with the engine's `f32` operation order and
a CPU-specific `rcpps` table. That makes it the right validation oracle (bit-equal to the engine on
16,000 benchmark views and 1,029 live captures) and the wrong product: it is 1.39× the native
call, derived from proprietary code (AD-13: never published), has no API a renderer
would want (one camera convention, a D3D-only depth range, no gate control, no reusable
per-thread scratch), and cannot be made faster without breaking the equality it exists for.

**Approach:** A new pure crate `crates/tenebrae-runtime` that implements portal-and-coverage
visibility over the typed tome model, written from the format reference and the model, not from
the oracle's code. It decodes a `Tome` once into native structures, answers frustum and portal
queries from a clean `Camera`, keeps per-thread scratch in a reusable `Query`, and exposes the
counters a cost model needs. It is accepted against the oracle (AD-4): the same status where the
answer is deterministic, the same frustum lists, and portal lists that never cull what the
oracle keeps while visible, within a few percent of the oracle's kept count. Speed is measured
against the engine's native runtime through the external harness, target at or under native.

**Not in M6:** replacing `tenebrae::query`'s camera queries in the facade (the AD-13 flip). That is
a separate, small follow-up once the consumer confirms what it links; this spec only adds
`tenebrae::runtime`.

## Boundaries & Constraints

**Always:**
- **Clean room.** `tenebrae-runtime` is written from the format reference, `umbra_research.md`, the
  `tenebrae-tome` model and its doc comments, and the behavioural facts recorded in this
  workspace's specs and handovers. It carries no engine-address references, no runtime memory
  layout, no `rcpps` table, none of the oracle's constants beyond those the format defines
  (tile cell record fields, portal word layout, `+0x9C` node distances, view-tree leaf codes,
  gate bits). The oracle is used only as a black box in tests. A reviewer must be able to read
  the crate without the oracle.
- **Pure library** (AD-1): `std` only, no dependencies outside the workspace, no `unsafe`, no
  I/O. Depends on `tenebrae-tome` only. `Runtime: Send + Sync`, `Query: Send`.
- **Semantics that are the format's, kept exactly:** the global cell at a point
  (`tenebrae_tome::locate`); portal word fields (axis, side, boundary and gate bits, target node
  and local cell, quantised plane and rectangle); the level-of-detail rule (an inner node with a
  tile is traversed in place of its subtree when the camera's distance to the node's box exceeds
  the node's `+0x9C` entry times `max(accuracy, base_distance)` with `accuracy` the caller's,
  −1 meaning base); gates: a gate portal passes only when every gate it names is open in the
  caller's gate state; object runs; user lists untouched (objects are indices, mapping to users
  is the caller's).
- **Conservative where the oracle is conservative.** A portal's screen footprint and an object's
  screen rectangle are computed so that any pixel the exact projection touches is covered;
  near-plane crossing is handled by clipping, not by dropping.
- **Statuses:** `Ok`, `NoCell` (camera in no cell), `BadCamera` (degenerate matrix), `Budget`
  (traversal cap hit). The runtime never panics on a `Tome` from `read` or `bake`; a malformed
  model (out-of-range indices) returns an `Error` from `Runtime::new`, which validates once.
- **Determinism:** the same tome, camera and options give the same answer on every machine
  (no hardware reciprocal, no platform `sin`/`cos`; `f32` arithmetic only, with no dependence on
  summation order across threads: one query runs on one thread).
- New public items are additions (contract v2 unchanged). `tenebrae-tools`' blobs stay
  magic-and-version prefixed (AD-12).

**Never:**
- No copying from `tenebrae-oracle`, no shared private helper crate between the two.
- No game data in the repository.
- No dynamic occluders, no occlusion-buffer output, no multi-worker screen split, no plane-BSP
  leaves beyond what `locate` does (a BSP leaf resolves as `locate` resolves it); these are
  recorded as deferred with their trigger.
- No change to the baker, the format or the bench's existing baseline keys.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Decode | `Runtime::new(&tome)` on a tome from `read`/`bake` | `Ok(Runtime)`; per-tile cells, portals, view trees, node boxes, object boxes decoded once | malformed indices → `Err(Error::Malformed(..))` |
| Frustum query | `Query::frustum(&camera)` | ascending object indices whose box is not outside any frustum plane | status `BadCamera` on a degenerate matrix |
| Portal query, eye in a cell | `Query::visible(&camera, &Options::default())` | `Ok`, ascending objects, counters filled | — |
| Eye in solid / outside domain | camera with no cell | status `NoCell`, empty list (caller falls back to frustum) | — |
| Degenerate camera | zero matrix | `BadCamera` | no panic |
| Budget | tome with many cells in a tile and a wide front | `Budget` (never where the oracle returns `Ok`) | — |
| Radius | `Options::radius > 0` | the near quad grown by the radius starts in every cell it touches | — |
| LOD | camera far from a node with an inner tile | that tile traversed, its subtree skipped; cells reported are the coarse ones | — |
| Gates | `Options::gates = Some(&[bool])` | gate portals pass iff all their gates are open; `None` = all open | out-of-range gate index → `Error::Params` from `visible` |
| Camera conventions | `Camera::from_world_to_clip_rows`, `…_cols`, `from_view_projection(view, proj, Depth::ZeroToOne | MinusOneToOne)` | identical queries for the same frustum | — |
| Reuse | one `Query` across thousands of cameras | no allocation after the first query of the largest size (checked with a counting allocator in tests) | — |
| Threads | one `Runtime`, a `Query` per thread | identical answers to a single thread | — |

</frozen-after-approval>

## Design

### Public API (`tenebrae_runtime`)

```rust
pub struct Runtime { .. }                         // decoded tome; Send + Sync
impl Runtime {
    pub fn new(tome: &Tome) -> Result<Runtime, Error>;
    pub fn objects(&self) -> usize;
    pub fn find_cell(&self, p: [f32; 3]) -> Option<u32>;   // tenebrae_tome::locate, cached ranks
    pub fn base_distance(&self) -> f32;
}
pub struct Camera { .. }                          // frustum planes, position, near quad
impl Camera {
    pub fn from_world_to_clip_rows(m: [[f32; 4]; 4], position: [f32; 3]) -> Camera;  // p * M, D3D depth: the engine's
    pub fn from_world_to_clip_cols(m: [[f32; 4]; 4], position: [f32; 3]) -> Camera;  // M * p
    pub fn from_view_projection(view: [[f32; 4]; 4], proj: [[f32; 4]; 4], depth: Depth) -> Camera;
}
pub enum Depth { ZeroToOne, MinusOneToOne }
pub struct Options<'g> { pub radius: f32, pub accuracy: f32 /* -1: base */, pub gates: Option<&'g [bool]> }
pub struct Query { .. }                           // per-thread scratch; Send
impl Query {
    pub fn new(rt: &Runtime) -> Query;
    pub fn frustum(&mut self, rt: &Runtime, camera: &Camera) -> Result<&[u32], Status>;
    pub fn visible(&mut self, rt: &Runtime, camera: &Camera, opts: &Options) -> Result<&[u32], Status>;
    pub fn counters(&self) -> &Counters;          // tiles, cells, portals, rasterised, objects tested, objects out
    pub fn visible_cells(&self) -> &[u32];        // global cells reached (for debugging and tools)
}
pub enum Status { NoCell, BadCamera, Budget }
```

Free functions `visible(&Runtime, &Camera, &Options)` and `frustum(..)` allocate a `Query` per
call for one-off use. The facade adds `pub use tenebrae_runtime as runtime;`.

### Algorithm (format semantics; the runtime's own data structures)

1. **Decode** (`Runtime::new`): top KD tree → per node box (`locate::leaves` and the implicit
   midpoint rule), rank directory for child lookup, `+0x9C` distances; per tile: cells with
   their portal slice, object list (runs expanded to a shared `Vec<u32>` with per-cell ranges),
   global cell, box; portals decoded to `{target node, local cell, axis, side, plane, rect,
   boundary, gate}` in world units; view trees kept as `locate` reads them; gates; object boxes.
2. **Camera**: six planes from the clip matrix (Gribb–Hartmann), normalised; position; the four
   near-plane corners; the near quad grown by `radius`; a degenerate matrix (upper-left 3×3 a
   multiple of the identity, or non-finite) → `BadCamera`.
3. **Start**: the tiles whose box meets the near quad (or holds the position when the radius is
   0); in each, the cells the quad meets through the view tree. Each gets a slot with full-screen
   coverage. None → `NoCell`.
4. **Level of detail**: walk the top tree from the root; drop nodes whose box (grown by the
   tile's `level` and the radius) is outside the frustum; a leaf, or an inner node with a tile at
   distance > `+0x9C × max(accuracy, base)` from the camera's position, becomes a *selected
   tile*; otherwise descend. Record selected tiles in front-to-back order by the KD split
   against the camera position.
5. **Traversal**: for each selected tile front to back: enter it (its cells get slots as they are
   reached), process its active cells in portal-dependency order (a cell whose portal leads
   into another active cell of the same tile is finished after that cell when the portal is
   back-facing to it, as the format's "internal first" ordering intends); for each cell, for
   each portal: skip gates that are closed and portals facing away from the camera; project the
   portal rectangle (clipped to the near plane) to a screen rectangle; if it meets the source
   cell's coverage, rasterise the quad conservatively into the target cell's coverage masked by
   the source's; a target in another selected tile is queued on that tile. A portal into a
   non-selected node (its selected ancestor stands in) is retargeted to the ancestor's coarse
   cell that contains the portal's target cell, found through the cell-run list (`+0x60`) in
   the coarse tile. Boundary portals are ignored.
6. **Finish**: a cell with no unprocessed portals outputs the objects of its list whose
   screen rectangle meets its coverage, each object once. A per-query cap of
   `2 × cells` visits per tile and a slot pool of 1,024 cells bound the work; exceeding them
   returns `Budget`.
7. **Coverage**: 64×64 pixels per cell as 128 `u32` words in a pool indexed by slot; the
   rasteriser marks every pixel whose square the quad touches (edge functions evaluated at the
   pixel corners nearest the edge), so it is conservative. Resolution is a crate constant for
   M6; making it an `Options` field is a later addition.
8. **Counters**: tiles entered, cells finished, portals considered, portals rasterised, object
   candidates, objects output, evictions.

### Where the answer may differ from the oracle, and why that is accepted

The oracle reproduces the engine's rounding (`rcpps`, lane order, truncation to pixels). The
runtime projects in plain `f32` and rasterises conservatively, so a portal or object near a
pixel edge may be covered here and not there. Differences are one-sided by design (the runtime
keeps more), bounded by the gate below, and never a false cull the oracle avoids.

## Code Map

- `crates/tenebrae-tome/src/lib.rs` — `Tome`, `Tile`, `TileCell` (word layout: portal start/count,
  object start/count, global cell / run, packed box), `Portal` (`is_gate`, boundary bit 28, axis
  bits 31..30, side bit 29, target `& 0x3FF_FFFF`, local cell `w[1] >> 16`, quantised plane and
  rectangle), `ViewTree`, `Gates`, `Header::{bounds, base_distance}`, `node_distances`.
- `crates/tenebrae-tome/src/locate.rs` — `leaves`, `Locator`, `find_cell`, `global_portals`,
  `global_cell_boxes`: the dequantisation and tree conventions to reuse (make `ranks`, `leaf_ranked`
  and `dequant` `pub` for the runtime, documented).
- `crates/tenebrae-bake/src/lod.rs` — how LOD tiles are written: coarse cells list their leaf
  cells' global cells through the cell-run list; openings listed between every pair of levels.
- `crates/tenebrae-bake/src/lib.rs` — `Params::lod_tiles` and `base_distance` docs: the runtime's
  walk rule as the baker relies on it.
- the format research notes, the format reference — format and runtime behaviour.
- `crates/tenebrae-bench/src/run.rs` — `visibility()`: the scorer to generalise over a query
  function so the runtime is scored with the same keys; `truth.rs` — `camera()` builds the
  oracle's `Camera` from the harness matrix; the runtime takes the same `f32` rows.
- `crates/tenebrae-tools/src/bin/query.rs` — gains `--native`: the `TBQV` blob from the runtime, so
  the evaluation harness compares it with the native engine too.
- the engine's native-runtime harness — timing reference (`queryPortalVisibility` best of 5).

## Tasks & Acceptance

**Execution:**
- [ ] `crates/tenebrae-runtime` (pure, `tenebrae-tome` only); workspace member and default member;
  facade `pub use tenebrae_runtime as runtime`.
- [ ] `locate`: `ranks`, `leaf_ranked`, `dequant` made `pub` with docs; a `Locator::leaf_cell_at`
  that returns the tile-local cell (the runtime needs local cells, not only global ones).
- [ ] `runtime/src/{lib, decode, camera, coverage, raster, walk, traverse, output}.rs` per the
  design; unit tests on hand-built tomes (reuse the oracle tests' `two_cell_tome` shape, rebuilt
  here, not imported): statuses, wall culls the box behind it, gates closed/open, LOD selects
  the coarse tile, radius starts in two cells, reuse allocates nothing after warm-up.
- [ ] `tenebrae-bench`: `run::visibility_with(scene, baked, truth, |cam| ..)`; `tests/runtime.rs`
  gates per case and tile size against the oracle: frustum lists equal on every view; status
  equal on every view where the oracle's is not `Budget`; `false_culls_hole` ≤ oracle's;
  `kept` at most 10 % above the oracle's; objects the runtime culls but the oracle keeps at
  most 3 % of the oracle's kept per case, and none of them truly visible. Baseline keys
  `native_kept`, `native_false_culls_hole` added by `bench baseline`. (Slacks amended
  2026-10-13 from 3 % / 0.1 % with the user's consent: measured, a conservative rasteriser
  keeps up to 8 % more than the oracle on `terrain` and disagrees with it on 0.3-2.6 % of
  objects, none of them visible; see the change log.)
- [ ] `tenebrae-tools` `query --native`; the external evaluation harness with `--native` compares runtime
  vs oracle vs the engine on the 40 export cases and the 1,029 live captures; harness-style
  timing of the runtime added to the harness's `--time` (best of 5 per camera, median).
- [ ] README M6 row, spine (structural seed, AD-2 crate graph, AD-13 step recorded as pending the
  facade flip), `deferred-work.md` entries (dynamic occluders, plane-BSP leaves, coverage
  resolution option, facade flip).

**Acceptance Criteria:**
- Given the 40 benchmark cases (both bakers' tomes, 16,000 views), when `tests/runtime.rs` runs,
  then frustum lists equal the oracle's on every view, statuses equal on every non-`Budget`
  view, no view culls a truly visible object the oracle keeps, and per case `kept` is within
  +10 % of the oracle's.
- Given the 1,029 live captures and the stock tome (external harness, local), when replayed through the
  runtime, then 0 truly visible objects culled, statuses equal to native on every non-`Budget`
  view, kept per view within +10 % of native on average.
- Given the same captures, when timed best-of-5 per camera on the validation machine, then the
  runtime's median is at most 1.5× the engine's native `queryPortalVisibility` median on the same tome
  (goal: ≤ 1.0×), and below the oracle's.
- Given a reviewer with the crate and the format docs only, then every structure and constant
  in `tenebrae-runtime` is explained by a format field or a design choice documented in this
  spec; no engine address appears in `crates/tenebrae-runtime`.
- Given clippy, fmt, `cargo test --release` and `cargo test --release -p tenebrae-bench`, then all
  pass with the existing baseline keys unchanged.

## Implementation Notes

- **Increments 1-3 landed 2026-10-13** (decode, camera, frustum; portal
  query; radius; then speed and tools). Crate
  `crates/tenebrae-runtime`, facade `tenebrae::runtime`, `query --native`, the external evaluation harness
  with `--native [--time]`, bench gate `tests/runtime.rs`, diagnostics
  `examples/{rtdiag,rtdiag_tome,rtdiag_radius,rtcount}.rs`.
- **Correctness, measured.** Benchmark export, 40 cases, both bakers' tomes, against the oracle:
  frustum lists equal on 16,000 views, statuses equal, no truly visible object culled, `kept`
  −0.7 % to +9.1 %, culled-but-oracle-kept ≤ 2.9 %. live captures (1,029 views, stock
  tome) against the engine's native runtime: statuses and frustum lists equal, `kept` +4.2 / +5.0 /
  +7.0 %, culled-but-native-kept 2.98 / 1.84 / 2.71 % (truth not available there).
- **The collision radius** needed four conservatisms to reach the engine's behaviour: portal
  rectangles grown by 1.75 radii per side, the frustum planes and the frustum box grown by
  the radius for node and object tests, object boxes grown by the radius, and every cell the
  near quad's box grown by the radius meets as a start cell. A sub-pixel dilation of
  rasterised portals (1/8 pixel) closes most remaining pixel-boundary disagreements. What the
  radius adds has the same distance profile in both engines (median ~50 m), so the mechanism
  matches; the engine's dilation is somewhat larger than a geometric `r` and the knobs above are
  tuned against the captures, not derived.
- **Speed, final for V1 (user's decision 2026-10-13).** The stock parity tome, 400 captured
  cameras, best of five per camera, median: 1,244 µs at first; 439 µs after the scanline
  rasteriser, covered-pixel boxes, fresh-pixel revisits, per-query caches and nearest-first
  ordering; **272 µs** after the exact back-face test (a straight sight line crosses a portal's
  plane once, from the eye's side: a portal whose neighbour lies on the eye's side, within the
  radius, is never exited through). engine native 129 µs on the same tome and cameras, the oracle
  ~180 µs. Through the external harness: captures 1.86 / 1.99 / 2.33× native; the benchmark export
  (80 tome rows) median 0.77× native, 0.10× to 1.75×, i.e. faster than the engine on most
  benchmark tomes and 2× slower on the stock one. The ≤ 1.5× acceptance stays open as deferred
  work. Counters per query on the stock tome: 24 tiles, 154 cells reached, 136 visits, 1,654
  portals considered (808 rejected by the back-face test), 445 rasterised over 4,400 pixel
  rows, 1,108 object candidates; the engine processes about 500 portals and 133 cells. Deferring
  cross-tile feedback to a later sweep (the "single ordered pass") changed no count: the
  back-face test had already removed the revisits, so the remaining time is rasterisation and
  portal-check volume, not ordering.
- **Final gates (end of M6 V1):** benchmark 40 cases against the oracle: 11/11
  tests, frustum lists equal on 16,000 views, statuses equal, 0 truly visible objects culled,
  kept −0.7 % to +9.1 %; export against engine native: 0 mismatches, kept −1.1 % to +9.2 %,
  oracle-only culls ≤ 2.1 %; captures against engine native: statuses and frustum lists equal,
  kept +3.0 / +4.0 / +6.5 %, native-kept-but-culled 3.13 / 1.90 / 2.80 % (one file a hair over
  the 3 % slack; no truth exists for captures, and the same views on the benchmark truth show
  no visible object lost).


## Spec Change Log

- 2026-10-13 (increment 2, user-approved): the benchmark slacks against the oracle are
  `kept` ≤ +10 % and culled-but-oracle-kept ≤ 3 % (were +3 % / 0.1 %). Measured on all 40
  cases with the touch-conservative rasteriser and the inclusive maximum pixel edge: 0 truly
  visible objects culled on 8,000 views, `kept` −1.5 % to +2.1 % on 36 cases and +6 to +8 % on
  `terrain`, culled-but-oracle-kept 0.3 % to 2.6 % (highest on 8-16 m tiles of `rooms` and
  `maze`); every one of those is an object the oracle
  keeps at a pixel boundary or through a coarse LOD cell, not a visible one. Sampling pixel
  centres (half-conservative) brought `terrain` to +4 % but culled truly visible objects on
  `city` and `rooms`, so conservative stays.

## Review Triage Log

| # | Source | Finding | Verdict | Route | Evidence |
|---|---|---|---|---|---|

## Verification

**Commands:**
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` -- clean.
- `cargo test --release` -- runtime unit tests pass; `cargo test --release -p tenebrae-bench` --
  `tests/runtime.rs` gates pass; existing gates unchanged.
- `cargo build --release -p tenebrae-tools && <harness> --native <export dir>` -- the acceptance numbers; `--capture` on the three capture files; `--time` for the median.
