# Tenebrae

A reader, writer, baker and runtime for **Umbra 3.3.6 visibility tomes** (format version 18), the
occlusion-culling data a shipped game engine loads every frame, written in pure Rust with no dependencies.

A tome is the occlusion data a renderer queries every frame: the camera's cell is located in a
KD tree, portals between cells are rasterised into a coverage buffer, and the objects behind
covered portals come back as visible. Tenebrae reads and writes the format losslessly, bakes new
tomes from occluder geometry, and answers the frustum and portal queries natively.

## Crates

A Cargo workspace under `crates/`:

| crate | role | depends on |
|---|---|---|
| `tenebrae-tome` | the format: lossless typed model, reader, writer, point lookups (`locate`) | nothing |
| `tenebrae-geom` | triangle soup with owners, BVH, exact segment occlusion | nothing |
| `tenebrae-bake` | the baker: voxelised occluders, cells and portals, view trees, LOD tiles, incremental re-bake | tome, geom |
| `tenebrae-runtime` | frustum and portal queries over a decoded tome | tome |
| `tenebrae` | the facade: `tenebrae::{tome, geom, bake, runtime, query}` (contract v2) | all four |
| `tenebrae-tools` | CLI binaries (`bake`, `query`, `inspect`, `cells`, `edit`, `diagnose`, `truth`); the only file I/O | facade, bench |
| `tenebrae-bench` | generated and real-world scenes, ray-cast truth, baseline gates | facade |

The library crates are pure: no filesystem, network, process or GPU access, no dependencies
outside the workspace, no `unsafe`. `tenebrae-runtime` is written from the format and the typed
model alone. Design notes per milestone are in `docs/implementation-artifacts/`.

## Status

| Milestone | State |
|---|---|
| M1 format: lossless typed model, reader, writer that **recomputes** every derived structure | **done**: byte-identical round trip on 26/26 stock tomes and 4/4 Unity 5.1.1 (Umbra 3.3.6) bakes |
| M2 geometry core: triangle soup, BVH, exact segment occlusion (`truth` tool) | **done**: 784k segments against 10 M triangles in 2.4 s |
| M3 baker: voxelise occluders, flood open space from seeds, cells as merged open regions, portals from voxel adjacency, leaf tiles and top tree, objects to cells | **done, at parity** with the stock tome on the parity scene (10 M triangles, 586 captured engine views): every query status 0, 0 truly visible objects culled |
| M3b scale: tile-streamed brick-sparse grid, parallel tiles, reachability, leaf-tile collapse, 0.125 m voxels, Umbra-parity levers (large empty leaves, per-tile cell merge, grouped global cells, midpoint view trees) | **done** (`spec-m3b-sparse-voxel-grid.md`, `spec-m3b-l4-midpoint-view-tree.md`): about 1/30 the memory of a dense grid; the parity scene bakes in 65-78 s on 16 threads at about +200 MB over the loaded scene (Umbra's optimizer: 164-359 s, +1,577 MB) |
| Tome size at Umbra parity: Umbra's cell pipeline (8 m components, visibility-cost cell merge, coalesced portals), strict view volumes (`Params::view_mask`), simplified view trees, shared object-run windows (`Params::share_lists`), optional domain margin | **done**: parity scene 1,106,272 B at defaults (Umbra 1,284,240 B) and 667,168 B with LOD tiles off (below; Umbra with its inner LOD tiles stripped 1,114,672 B); stock tome 976,304 B, status 0 on 586/586 views, 0 truly visible objects culled, 423 objects kept per view (stock 343, Unity rebake 419). Benchmark: smaller than Umbra on 35 of 40 cases, all 40 bakes in 72 s against Umbra's 112 s |
| M3c incremental re-bake: per-tile cache of both tile passes keyed by a 128-bit hash of each tile's inputs | **done** (`spec-m3c-incremental-rebake.md`): a cached re-bake is byte-identical to a fresh bake; unchanged re-bake of the parity scene 2.4 s (fresh 66 s), one prop moved 4.9 s |
| M4 bake API (**contract v2**): instanced or soup occluders, targets with user lists, a view volume in; tome, object map, representative cell points, counts and cache bytes out; progress with cancel; presets; cell transfer and overlay geometry | **done** (`spec-m4-bake-api.md`): default bakes byte-identical to M3c; an instanced input bakes byte-identically to its applied soup; `keep_every_target` keeps object *i* = target *i* |
| Query cost in the engine's runtime: each cell lists its in-tile portals first; inner-node LOD tiles (`Params::lod_tiles`, on by default) | **done**: portal queries 11 % faster with LOD tiles (23 % at 8 m) and 13 % faster than on Umbra's own tomes, measured in the engine's native runtime; objects kept +0.2 %, no new false cull |
| M6 native runtime (`tenebrae-runtime`): `Runtime` decodes a tome once, a per-thread `Query` answers the frustum and portal queries for a `Camera` (row or column clip matrices, either depth range) with radius, level of detail, gates and counters | **done, V1** (`spec-m6-native-runtime.md`). Against the engine's native runtime on 1,029 captured views of a stock tome: statuses and frustum lists equal, objects kept +3 to +6.5 %, 0 truly visible objects culled on the benchmark truth. Speed: 272 µs median per query on the stock tome against the engine's 129 µs (2.1×; the 1.5× target stays open) |

Every milestone was accepted by equality or parity with the engine's own runtime: round trips are
byte-identical, bakes answer captured engine views with the same statuses and no false culls, and
the runtime's answers were compared view by view with the engine's on 16,000 benchmark views and
1,029 live captures.

## Contract v2

Callers link the facade crate (`crates/tenebrae`, package `tenebrae`) and call one function per
view volume:

```rust
use tenebrae::bake::{bake, Control, Input, Occluders, Params};
let baked = bake(&Input { occluders: Occluders::Instanced { meshes: &meshes, instances: &instances },
                          targets: &targets, view_volume, seeds: &seeds,
                          params: Params { threads, ..Params::tome_a() } },
                 Some(cache_bytes.as_slice()), Some(&mut |p| { report(p); Control::Continue }))?;
let bytes = tenebrae::write(&baked.tome);   // baked.object_map, baked.report, baked.cache
```

Querying a tome:

```rust
use tenebrae::runtime::{Camera, Options, Query, Runtime};
let rt = Runtime::new(&tome)?;                  // once; Send + Sync
let mut q = Query::new(&rt);                    // per thread; reused across cameras
let camera = Camera::from_world_to_clip_rows(world_to_clip, position);
let visible = q.visible(&rt, &camera, &Options::default())?;   // ascending object indices
let in_frustum = q.frustum(&rt, &camera)?;
```

- **Items under the contract:** `bake::bake` and its types (`Input`, `Occluders`, `Instance`,
  `Target`, `Params` with `tome_a()` / `tome_b()`, `Baked`, `Report`, `Stage`, `Progress`,
  `Control`, `Hook`, `Error`), and `query::{find_cell, Locator, transfer, CellVotes,
  global_cell_boxes, global_portals, PortalRect, cell_regions, leaves}`. A breaking change is a
  new contract version named here; new `Params` or `Report` fields are not breaking.
- **Runtime (additions, not breaking):** `runtime::{Runtime, Query, Camera, Depth, Options,
  Status, Counters, visible, frustum}`, also re-exported under `query`. `Camera` takes a
  row-vector world-to-clip matrix with D3D depth (`from_world_to_clip_rows`), a column-vector one
  (`from_world_to_clip_cols`), or view and projection matrices with either depth range
  (`from_view_projection`). A query returns `Err(Status)` when the camera is in no cell, the
  matrix is degenerate or the traversal budget is exhausted; callers then fall back to `frustum`.
- **Plane-BSP leaves (addition, not breaking):** `query::local_cell` (from `tome::locate`)
  resolves one view-tree leaf to its local cell. `find_cell`, `Locator` and
  `Runtime::find_cell` follow plane-BSP leaves of stock tomes to their local cell, where they
  used to answer `None`.
- **`Report::cell_points`** is `Vec<Option<Vec3>>` and `query::transfer` takes
  `&[Option<[f32; 3]>]`: with `Params::view_mask` (on by default, Umbra's strict view volumes),
  view trees resolve cells only within a voxel of the view volume, so a cell wholly outside it
  has no point `find_cell` resolves to it, and its entry is `None`.
- **Presets:** `tome_a()` is the validated parity setting (stock `SH 0.125`) with
  `keep_every_target`; `tome_b()` is the same at 0.5 m voxels (stock `SH 0.500`, every other
  stock setting equal) without LOD tiles. Measured 2026-10-13 on the parity scene against stock tome B's 3,422
  objects, replaying the 586 captured views in the engine's native runtime: status 0 on 586/586,
  0 truly visible objects culled, 430 objects kept per view (stock B 360, Umbra's own 0.5 m
  bake 424 with 8 truly visible culled), 592,640 B (stock B 881,168 B, Umbra 810,096 B; with
  LOD tiles 1,034,288 B), baked in 7.7 s. The captures were recorded on tome A; when the engine queries B is
  not known.
- **Occluders:** which geometry occludes is the caller's rule. The parity numbers above were
  measured with the map's static meshes (triangle lists, lowest LOD excluded) and its terrain
  tiles at full detail, as the engine's own bake selects them.
- **Volume:** the domain is the view volume snapped outward to whole tiles (32 m), grown over
  targets up to `Params::domain_margin` past it (default 0). Occluders past it are ignored (except
  by parity rays), seeds past it are ignored, and a target reaching past it is also listed in
  every cell with a boundary portal on that side. View trees resolve cells only within a voxel of
  the view volume; a point further out may find any nearby cell or none.

## Rules

- **Pure libraries.** `tenebrae-tome`, `-geom`, `-bake`, `-runtime` and the facade: no filesystem,
  network, process or GPU access; callers pass bytes. `unsafe` is forbidden. No dependencies
  outside the workspace.
- **No game data in this repository.** Tests read tomes from directories named in
  `TENEBRAE_TOMES` (`;`-separated); without it they pass vacuously and say so.
- Toolchain pinned in `rust-toolchain.toml`. `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo fmt -- --check` are gates (`.githooks/pre-commit`; enable with
  `git config core.hooksPath .githooks`).

## Benchmark

`crates/tenebrae-bench/` is a domain-agnostic benchmark that is safe to commit: eight seeded,
generated scenes (rooms, city, thin walls, terrain, maze, tile seams, instances, degenerate
geometry) and two real-world scenes reduced to occluder geometry (Amazon Lumberyard Bistro,
CC BY 4.0; Crytek Sponza, CC BY 3.0; `crates/tenebrae-bench/assets/`), each baked at 8, 16, 32 and
64 m tiles. `cargo test --release -p tenebrae-bench` holds the gates, none of which needs anything
outside the repository: round trip, determinism, cache, instanced = soup, every eye in a cell,
tile cell limits, size against `baseline.txt`, and visibility scored with `tenebrae-runtime`
against ray-cast truth (every query status 0, no growth of the errors, objects kept at most 10 %
above the baseline, frustum query equal to the baseline). Umbra 3.3.6's own numbers on the same
scenes, baked with its optimizer and scored in its runtime, are committed as `reference/umbra.txt`.
Details, workflow, current results and the diagnostic examples: `crates/tenebrae-bench/README.md`.

## Building and running

```
cargo test --release                       # every crate but the benchmark
cargo test --release -p tenebrae-bench     # the benchmark gates (a few minutes)
cargo build --release -p tenebrae-tools    # target/release/{bake,query,inspect,cells,edit,diagnose,truth}
```

The stock-tome round trip:

```
set TENEBRAE_TOMES=C:\path\to\tomes;C:\another\path
cargo test --release -p tenebrae-tome -- --nocapture
```

## Format reference

The typed model in `crates/tenebrae-tome/src/lib.rs` documents every record and field; the
section order and padding as the writer emits them are documented in
`crates/tenebrae-tome/src/write.rs`. The runtime's traversal rules (level of detail, gates,
portal ordering) are in `docs/implementation-artifacts/spec-m6-native-runtime.md`.

## Contributing

Tenebrae is developed with AI coding agents under human direction, following the BMAD Method; the
specs, architecture spine, review logs and deferred-work ledger under `docs/` are the record of
that process. `CONTRIBUTING.md` explains how to work the same way, `AGENTS.md` holds the layout and
policy any contributor or agent must keep to.

## License

GPL-3.0-or-later. See the LICENSE file.
