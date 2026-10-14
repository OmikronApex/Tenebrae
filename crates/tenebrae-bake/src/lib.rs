//! Baker: a portal-cell tome from occluder triangles, target boxes and a view volume.
//!
//! * The view volume is snapped to a grid of cubic tiles (`tile` metres); a top KD tree splits the
//!   tile grid at integer tile boundaries, one leaf per tile. Leaf tiles carry the cells; with
//!   [`Params::lod_tiles`] inner nodes also carry coarse LOD tiles, which the runtime traverses
//!   instead of distant leaf tiles (`bake/lod.rs`).
//! * The domain is voxelised (`voxel` metres). [`SolidMode::Flood`] (default) floods open space
//!   from seed points through voxel faces no triangle covers; unreached voxels are solid, and with
//!   [`Params::surface_occludes`] (default) so are voxels a triangle passes through (Umbra's
//!   "smallest hole" tolerance: gaps under a voxel count as closed). [`SolidMode::Parity`] makes
//!   a voxel solid only inside closed geometry by crossing parity.
//! * Seed cells are the connected components of open voxels inside blocks of a few metres (Umbra's
//!   cell level, derived from its "smallest occluder"), or greedy boxes; seeds are then
//!   agglomerated best first (see [`CellMode::Merged`]); each result is a cell. Cells that touch share portals (the
//!   bounds of their shared voxel faces per plane), inside a tile or across tiles. Open faces on
//!   the domain boundary get boundary portals.
//! * Each tile's view tree is a KD tree over its voxels, split where cells meet. A leaf is one
//!   cell: solid voxels are "don't care" (the camera is never in them), and only wholly solid
//!   regions become solid leaves (all-ones). Built breadth first, which reproduces the engine's
//!   level-order child numbering.
//! * Each target is listed in every cell within one voxel of its box: its surface often lies in
//!   solid voxels, and a sight line to it then arrives through an open neighbour. Global cells
//!   mirror the tile cells one-to-one, global portals mirror the tile portals. The engine's camera query
//!   gives the same answers without them, but its audio system walks them, so they are required.
//!
//! The bake streams tiles and never holds a whole-domain voxel array. A tile's voxels live in
//! bricks of up to 16³ voxels, each one value when uniform, so memory follows the bricks geometry
//! touches. The flood is global, so it takes two passes over the tiles: pass A labels each tile's
//! open components and emits their boundary layers, a serial join links components across tiles,
//! and pass B re-voxelises each tile, decides solid space and builds its cells, portals and view
//! tree. Both passes run on [`Params::threads`] workers; tiles join in tile order, so the output
//! does not depend on the thread count.
//!
//! A bake can re-bake incrementally: the tile cache an earlier bake returned (as bytes) supplies
//! every tile pass whose inputs did not change, and the result is byte-identical to a fresh bake.
//!
//! [`bake`] is contract v1 (README, M4): [`Input`], [`Params`] with [`Params::tome_a`] and
//! [`Params::tome_b`], [`Baked`], [`Report`], [`Progress`] and [`Error`]. [`voxelize`] and
//! [`Voxels`] are diagnostic and outside it.

mod cache;
mod global;
mod grid;
mod group;
mod lod;
#[cfg(test)]
mod reference;
mod tile;

use std::borrow::Cow;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError, mpsc};

use cache::TileCache;
use grid::{Bits, Bricks, Grid};
use tenebrae_geom::{Bounds, Bvh, Mesh, Vec3};
use tenebrae_tome::{Aabb, Tome, Users};
use tile::{Ctx, Space, clip, space_of, tile_bytes_bound, touched_bricks, tri_ranges};

/// An object the tome should answer for: its box and the caller's ids for it.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// World box; the object's box in the tome, never shrunk.
    pub bounds: Aabb,
    /// Caller ids, stored as the object's user list in this order: at least one, usually one (a
    /// stock tome's own lists can be passed as they are). An empty list is `Error::Params`.
    pub users: &'a [u32],
}

/// One placement of a mesh: `world = transform * [local, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    /// Index into [`Occluders::Instanced::meshes`].
    pub mesh: u32,
    /// Row-major 3×4 affine transform from mesh space to engine world space.
    pub transform: [[f32; 4]; 3],
}

impl Instance {
    /// The world position of mesh-space `v`, exactly as the bake computes it: per row
    /// `((m0 * x + m1 * y) + m2 * z) + m3` in `f32`, without fused multiply-add. A soup built
    /// with this function bakes to the same bytes as the instanced input.
    pub fn apply(&self, v: Vec3) -> Vec3 {
        let m = &self.transform;
        [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + m[r][3])
    }
}

/// Occluder geometry. Which geometry occludes is the caller's rule (the engine's rule the parity
/// numbers were measured under is in the README).
#[derive(Clone, Copy, Debug)]
pub enum Occluders<'a> {
    /// World-space triangles.
    Soup(&'a Mesh),
    /// Meshes in their own space, placed by instances. Triangles are numbered instance by
    /// instance, each instance in its mesh's triangle order; the total must fit in a `u32`.
    Instanced {
        /// The meshes.
        meshes: &'a [Mesh],
        /// The placements.
        instances: &'a [Instance],
    },
}

#[cfg(test)]
impl Occluders<'_> {
    /// The soup (the dense reference baker reads soups only).
    fn mesh(&self) -> &Mesh {
        match self {
            Occluders::Soup(m) => m,
            Occluders::Instanced { .. } => unreachable!("the reference bakes soups"),
        }
    }
}

/// The occluder triangles as one numbered list, whatever their form.
pub(crate) struct Tris<'a> {
    occluders: Occluders<'a>,
    /// Instanced: per instance its first triangle, plus the total.
    starts: Vec<u32>,
}

impl<'a> Tris<'a> {
    /// Numbers the triangles; `None` when the total does not fit in a `u32` (or a mesh index is
    /// out of range, which validation reports first).
    fn new(occluders: Occluders<'a>) -> Option<Tris<'a>> {
        let starts = match occluders {
            Occluders::Soup(m) => {
                u32::try_from(m.triangles.len()).ok()?;
                Vec::new()
            }
            Occluders::Instanced { meshes, instances } => {
                let mut starts = Vec::with_capacity(instances.len() + 1);
                let mut next = 0u32;
                for i in instances {
                    starts.push(next);
                    let n = meshes.get(i.mesh as usize)?.triangles.len();
                    next = next.checked_add(u32::try_from(n).ok()?)?;
                }
                starts.push(next);
                starts
            }
        };
        Some(Tris { occluders, starts })
    }
    /// Number of triangles.
    pub(crate) fn len(&self) -> u32 {
        match self.occluders {
            Occluders::Soup(m) => m.triangles.len() as u32,
            Occluders::Instanced { .. } => self.starts.last().copied().unwrap_or(0),
        }
    }
    /// World corners of triangle `t`.
    pub(crate) fn corners(&self, t: u32) -> [Vec3; 3] {
        match self.occluders {
            Occluders::Soup(m) => {
                let tri = m.triangles[t as usize];
                tri.map(|i| m.vertices[i as usize])
            }
            Occluders::Instanced { meshes, instances } => {
                // The last instance starting at or before `t` (empty instances share a start).
                let k = self.starts.partition_point(|&s| s <= t) - 1;
                let inst = &instances[k];
                let m = &meshes[inst.mesh as usize];
                let tri = m.triangles[(t - self.starts[k]) as usize];
                tri.map(|i| inst.apply(m.vertices[i as usize]))
            }
        }
    }
    /// The triangles as one world-space mesh (owners: the instance index), borrowed for a soup.
    fn soup(&self) -> Cow<'a, Mesh> {
        match self.occluders {
            Occluders::Soup(m) => Cow::Borrowed(m),
            Occluders::Instanced { meshes, instances } => {
                let mut out = Mesh::default();
                for (k, inst) in instances.iter().enumerate() {
                    let m = &meshes[inst.mesh as usize];
                    let base = out.vertices.len() as u32;
                    out.vertices
                        .extend(m.vertices.iter().map(|v| inst.apply(*v)));
                    out.triangles
                        .extend(m.triangles.iter().map(|t| t.map(|i| base + i)));
                    out.owners.extend(m.triangles.iter().map(|_| k as u32));
                }
                Cow::Owned(out)
            }
        }
    }
}

/// How empty voxels become cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellMode {
    /// Greedy boxes: many small cells, exact portals, the most occlusion.
    Boxes,
    /// Connected components of empty voxels inside each block of `block` metres (0: the whole
    /// tile): few cells, occlusion at block granularity and between separated pockets. Umbra's
    /// cell level is the block of edge tile / 2^round(log2(tile / smallest occluder)) (8 m at
    /// the stock T 32, SO 6, and at every benchmark tile size), with holes resolved at the voxel level
    /// below it. This is Umbra's own start: its cells are these components, merged by lever L2
    /// alone. A tile with more than [`Params::max_cells_per_tile`] components merges them best
    /// first (as [`CellMode::Merged`] does past its fill) until it is under the cap.
    Components {
        /// Block edge in metres, rounded to whole voxels; 0 = the tile.
        block: f32,
    },
    /// Greedy boxes, then neighbouring cells of one tile are merged best first: always the pair
    /// whose union's bounding box is the largest share open voxels of the two (larger opening
    /// on ties), as long as that share is at least `fill` (a merge that would wrap around a wall
    /// pulls the wall's solid volume into the box and is refused) or the tile is over
    /// [`Params::max_cells_per_tile`]. Bounds the cell count, which the runtime's portal culler
    /// needs, while keeping walls as cell boundaries.
    Merged {
        /// Minimum fraction of empty voxels in a merged cell's bounding box, e.g. 0.9.
        fill: f32,
        /// Seed cells: 0 = greedy boxes; otherwise components inside blocks of this many
        /// metres, rounded to whole voxels (see [`CellMode::Components`]).
        block: f32,
    },
}

/// How solid (occluding) space is decided. Voxels touched by a triangle are never solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolidMode {
    /// Inside closed geometry by crossing parity on all six axis directions. Exact for
    /// watertight meshes; open meshes yield no solid space.
    Parity,
    /// Everything not reachable from the domain boundary or a seed point through untouched
    /// voxels (Umbra's approach). Works with open meshes; gaps narrower than a voxel are
    /// treated as closed, the same tolerance as Umbra's "smallest hole".
    Flood,
}

/// Bake settings.
#[derive(Clone, Debug)]
pub struct Params {
    /// How solid space is decided.
    pub solid: SolidMode,
    /// How empty voxels become cells.
    pub cells: CellMode,
    /// Tile edge in metres. Must be a multiple of `voxel`.
    pub tile: f32,
    /// Voxel edge in metres.
    pub voxel: f32,
    /// Header `+0x0C`.
    pub base_distance: f32,
    /// With [`SolidMode::Flood`]: voxels geometry passes through occlude too (unless a seed is in
    /// them). This is what gives thin walls occlusion, at Umbra's price: gaps narrower than a
    /// voxel next to geometry count as closed, so slivers seen past an edge can be culled. The
    /// error shrinks with the voxel (the parity scene, wrongly culled view-object pairs: 429 at 1 m,
    /// 19 at 0.5 m, 4 at 0.25 m; Umbra's own setting is 0.125 m, "smallest hole").
    /// Off, the bake is strictly conservative and thin walls (under ~2 voxels) never occlude.
    pub surface_occludes: bool,
    /// With [`CellMode::Merged`]: merging continues below `fill` in any tile that still has more
    /// cells than this. The runtime's portal culler fails (status 2: its per-tile traversal
    /// budget is twice the tile's cell count) or crashes when tiles hold too many cells; 32 m
    /// tiles fail at 1,024, 16 m tiles at 256 (stock maximum: 338). 192 on 16 m tiles is clean.
    pub max_cells_per_tile: usize,
    /// Worker threads for the per-tile passes. 1 runs everything on the caller's thread; the
    /// output is the same for every count.
    pub threads: usize,
    /// Most bytes of grid storage a bake may plan for ([`Report::grid_bytes`]); a bake whose
    /// estimate exceeds it fails with [`Error::TooLarge`] before allocating its grid. The
    /// estimate is a practical bound, not a strict one: it bounds brick storage from the bricks
    /// triangles touch, but charges a flat 2 KiB per tile for the results tiles keep and bounds
    /// boundary layers from touched bricks only. The tile caches of a cached [`bake`] (the one
    /// passed in and the one returned, with their encoded payloads) are not counted.
    pub memory_limit: u64,
    /// Unreachable-cell removal: connected groups of cells smaller than this share of the
    /// largest group's cell count are removed (Umbra's `reachability_analysis_threshold`).
    pub reachability_threshold: f32,
    /// Lever L1: the top tree splits only along tile-grid planes that bound tiles with triangles,
    /// and each maximal empty region (all open or all solid) is one large leaf tile, as in
    /// Umbra's tile grid. Off: one leaf per grid tile.
    pub empty_leaves: bool,
    /// Lever L2: Umbra's per-tile cell merge (`CellMerger` mode 1 then mode 2) and portal
    /// coalescing, with Umbra's smallest occluder `R` in metres (stock: 6). A union is accepted while
    /// the area of its openings times the share of sight lines between them it hides is at most
    /// `R²`; portals per neighbour and plane become at most four rectangles, the cover with the
    /// least area plus `R² / 2` per rectangle (`(R / 4)²` across tile seams). 0 = off: no merge,
    /// one bounding rectangle per portal.
    pub cell_merge: f32,
    /// Lever L3: each global cell is a connected group of one tile's cells (over in-tile portals)
    /// instead of one global cell per tile cell.
    pub global_groups: bool,
    /// Lever L4: every view-tree split is an implicit midpoint of its node's box (the runtime's
    /// f32 `(min + max) * 0.5`, from the tile's box down), so tiles store no split floats, as in
    /// Umbra's view trees. The axis is still chosen per node. Leaves stay conservative: a node is
    /// a leaf only when every voxel it overlaps, even partly, belongs to one cell (solid voxels
    /// being don't-care). Off: every split is stored, on a voxel plane where cells meet.
    pub midpoint_views: bool,
    /// With [`Params::midpoint_views`]: the view trees resolve cells only within one voxel of the
    /// view volume, and treat space further out as "don't care", as Umbra's strict view volumes
    /// (optimizer flag 4, which the stock tomes set) do. The camera never leaves the view volume, so no
    /// answer changes for it; a point outside it may resolve to any nearby cell or to none, and
    /// a cell wholly outside it has no view-tree leaf (its [`Report::cell_points`] entry is
    /// `None`). Cells, portals and object lists are unchanged.
    pub view_mask: bool,
    /// How far past the view volume (metres, per side) the domain grows to take in the targets,
    /// as Umbra's tile grid covers the whole scene. Space there is baked into ordinary cells, so
    /// a target in it is listed where sight lines reach it instead of in every cell with a
    /// boundary portal on that side. 0: the domain is the view volume, snapped to tiles. With
    /// several view volumes ([`Params::extra_view_volumes`]) the margin is taken past the box
    /// around all of them.
    ///
    /// Tiles away from every view volume are built coarser with [`Params::coarsen_outside`].
    /// Tiles outside the view volume no longer get the feature and seed block the old rule raised
    /// from the volume's thinnest extent, so a margin bake differs from earlier versions even with
    /// `coarsen_outside` off.
    pub domain_margin: f32,
    /// With [`Params::cell_merge`] (`SO`): tiles away from every view volume are baked on coarse
    /// voxels, as Umbra builds tiles outside its view volumes. The camera never enters them, but
    /// on a large margin they are most of the domain. Each tile gets a level shift `k`
    /// (`tile::level_shift`): 0 when it overlaps a view volume grown by
    /// `pad = max(1.6 SO, 2 voxel)`; otherwise, with `d` its least distance to a view volume and
    /// `q = d / (4 SO) - 1`, `k = 1 + floor(log2 q)` when `q >= 1`, else 1, capped so that
    /// `2^k` divides the tile's voxel count. A tile at `k > 0` is voxelised at `voxel · 2^k`, with
    /// lever L2's threshold `SO · 2^k` and seed blocks of the same voxel count (so `2^k` times
    /// larger, at most the tile). At SO 6, 0.125 m voxels and 32 m tiles: `d` under 72 m gives
    /// 0.25 m voxels, under 120 m 0.5 m, under 216 m 1 m, under 408 m 2 m, under 792 m 4 m. Such
    /// a tile still reports cells, portals and seams at the grid's voxel, so its neighbours, the
    /// flood across tiles and the tome are built as for any tile; two sibling leaf tiles that are
    /// both coarse may collapse up to `tile · 2^min(k)`. Off, or with `cell_merge` 0, every tile
    /// is baked at `voxel`, with the feature and seed block of a tile inside the volumes (the old
    /// thinnest-extent raise of outside tiles is gone). Tiles touching a view volume never change.
    /// It matters only with a domain margin or several view volumes (tiles in the gap between
    /// volumes coarsen even at margin 0).
    pub coarsen_outside: bool,
    /// More view volumes beside [`Input::view_volume`]: the camera may be in any of them. The
    /// domain covers all of them, the view mask ([`Params::view_mask`]) keeps the union of their
    /// voxel ranges, unreachable-cell removal starts each cell from the box around its overlaps
    /// with the volumes, and [`Params::coarsen_outside`] measures distance to the
    /// nearest. Each is checked as `view_volume` is (finite, not empty). Default: none.
    pub extra_view_volumes: Vec<Aabb>,
    /// Cells share object-run windows (`+0x58`): a cell whose run entries already appear, in
    /// order, in the stream points at them instead of appending its own, and a new list reuses
    /// the stream's tail where it matches, longest lists first. Umbra packs its lists the same
    /// way (`global_compressLists`); the lists stay exact.
    pub share_lists: bool,
    /// Leaf-tile collapse: two sibling leaf tiles merge when their union's largest extent is at
    /// most this many metres (0: no size limit) and they hold fewer than 96 cells together
    /// (Umbra's "CS"). Without [`Params::lod_tiles`] keep it at 16 m or more: a front of many
    /// small tiles then exhausts the runtime's per-tile traversal budget (status 2; 8 m tiles
    /// with collapse 8 fail on most views of the benchmark's `city` and `terrain`, with collapse
    /// 16 on none). With LOD tiles collapse 8 passes too, but 16 is still faster and smaller.
    /// Equal to `tile` it collapses nothing.
    pub collapse_size: f32,
    /// Lever L5: inner top-tree nodes carry coarse LOD tiles, as Umbra's do (`lod_buildTile`):
    /// a node gets one once at least five tiles pooled below it and its cells, connected
    /// components of the leaf cells under it joined within each quarter of the node, number at
    /// most 0.7 of theirs. The runtime traverses a coarse tile instead of the tiles below it
    /// once the camera is farther from the node's box than the cube root of its volume (`+0x9C`
    /// times the base distance), so distant space costs a few coarse cells instead of a front
    /// of leaf tiles. Every opening is listed between every pair of levels; coarse cells list
    /// the union of their leaf cells' objects. Measured 2026-10-13 in the engine's native runtime
    /// (`plan-bench-gaps.md` item 8). Benchmark: portal queries 11 % faster (23 % at 8 m tiles,
    /// 15 % at 16 m) and 13 % faster than on Umbra's own tomes, objects kept +0.2 % (worst case
    /// +7.4 %), no new false cull, tomes +26 %. The parity scene: 1,106,272 B (667,168 B
    /// without; Umbra 1,284,240 B), status 0 on 586/586, 0 truly visible culled, 424 kept per
    /// view (423 without), median query on the 1,029 captured engine cameras 147.8 µs (169.0 µs
    /// without, stock tome 136.4 µs).
    pub lod_tiles: bool,
    /// Keep every target as an object, so object `i` is target `i` and the object map is the
    /// identity. A target whose cells unreachable-cell removal took is then an object listed in
    /// no cell (never visible), like a buried target. Off: such a target is dropped and later
    /// objects renumber, as Umbra does.
    pub keep_every_target: bool,
    /// Object grouping (`group.rs`): the baked objects are joined into fewer, larger ones by
    /// Umbra's `ObjectGrouper`, each group's box, user list and cell lists the union of its
    /// members', as the stock tomes hold about one object per two engine users. The value is
    /// the per-object cost against a grouping cost of 1; only the ratio matters, and Umbra's
    /// default costs (1,000 per object at cost 4) give 250. The stock parity tome's 6,342 users to
    /// 3,385 objects is matched by 10,000 (any grouper with that size profile recovers the full ~18 µs
    /// per query; the exact groups do not matter). Targets are then the engine's users, one box each, and
    /// [`Baked::object_map`] names each target's group. 0: every target is an object. Meant
    /// for one target per user; grouping already grouped targets groups them twice.
    pub group_cost: f32,
}

impl Default for Params {
    fn default() -> Self {
        // 0.125 m voxels: Umbra's "smallest hole" (the stock SH 0.125). Measured with these defaults
        // (levers L1-L4 on) on the parity scene (domain and view
        // volume (-192,-160,-672)..(160,192,608), 3,385 stock boxes, captured seeds; 586 engine
        // views, BVH ground truth; M3b and L4 spec implementation notes): 344 tiles, 8,367 cells,
        // 838 global cells, 2,147,600 B (3,765,856 B with L4 off), status 0 on 586/586, 0 truly
        // visible objects culled (Unity rebake: 9), 384 objects kept per view (stock 343, Unity
        // 419); 65 s on 16 threads at +216 / +234 MB (WS / private) over the loaded scene, 578 s
        // and +118 / +135 MB on 1 thread. With L2's cap on wrapped open space and the object box
        // rule (2026-10-12): 9,705 cells, 838 global cells,
        // 2,498,624 B, status 0 on 586/586, 0 truly visible objects culled, 377 kept per view,
        // 78 s on 16 threads (memory not remeasured). With Umbra's cell pipeline (8 m components,
        // L2 as Umbra's merge, coalesced portals), the view mask and shared object lists
        // (2026-10-12): 2,273 cells, 667,168 B (Umbra 1,114,672 B), status 0
        // on 586/586, 0 truly visible culled, 423 kept per view (Unity rebake 419), 66 s; L2 at
        // 3 m instead: 893,072 B, 394 kept; 6 m blocks: 872,672 B, 393 kept.
        Params {
            solid: SolidMode::Flood,
            cells: CellMode::Components { block: 8.0 },
            tile: 32.0,
            voxel: 0.125,
            base_distance: 24.0,
            surface_occludes: true,
            max_cells_per_tile: 256,
            threads: 16,
            memory_limit: 4 << 30,
            reachability_threshold: 0.1,
            collapse_size: 32.0,
            empty_leaves: true,
            cell_merge: 6.0,
            global_groups: true,
            midpoint_views: true,
            view_mask: true,
            domain_margin: 0.0,
            coarsen_outside: true,
            extra_view_volumes: Vec::new(),
            share_lists: true,
            lod_tiles: true,
            keep_every_target: false,
            group_cost: 0.0,
        }
    }
}

impl Params {
    /// The defaults with the Umbra-parity levers L1-L4 off: the bake of the tile-streamed baker
    /// before the levers. Its output is byte-identical to the whole-grid baker (`0ce18d0`) for
    /// the same tile, voxel, cell and solid settings when, in addition, unreachable-cell removal
    /// keeps every cell and no tiles collapse, which `reachability_threshold = 0.0`,
    /// `collapse_size = tile` and a view volume equal to its tile-aligned domain guarantee (the
    /// dense-reference tests use exactly that). The parity scene at 0.25 m on a tile-aligned box is also
    /// identical with the default threshold and collapse size, because neither changes anything
    /// there.
    pub fn levers_off() -> Params {
        Params {
            // The cell setting of the M3 parity bake, before lever L2 became Umbra's merge.
            cells: CellMode::Merged {
                fill: 0.95,
                block: 6.0,
            },
            empty_leaves: false,
            cell_merge: 0.0,
            global_groups: false,
            midpoint_views: false,
            share_lists: false,
            lod_tiles: false,
            ..Params::default()
        }
    }

    /// The stock tome A (stock header `SH 0.125`): the validated parity setting ([`Params::default`])
    /// with [`Params::keep_every_target`], so the bubble's object-indexed tables keep their
    /// numbering.
    pub fn tome_a() -> Params {
        Params {
            keep_every_target: true,
            ..Params::default()
        }
    }

    /// The stock tome B (stock header `SH 0.500`, every other stock setting equal to tome A's):
    /// [`Params::tome_a`] at 0.5 m voxels without [`Params::lod_tiles`]. On the parity scene
    /// (2026-10-13, 586 captured views on stock B's objects): status 0 on 586/586, 0 truly visible
    /// culled, 430 kept per view (stock B 360), 592,640 B (stock B 881,168 B; with LOD tiles
    /// 1,034,288 B).
    pub fn tome_b() -> Params {
        Params {
            voxel: 0.5,
            lod_tiles: false,
            ..Params::tome_a()
        }
    }
}

/// A leaf tile's `+0x48` level, as Umbra's optimizer writes it: the smallest hole (`voxel`)
/// times the tile's shortest edge over the tile size `tile`, so 0.125 on an ordinary 32 m tile
/// at 0.125 m voxels and less on a flatter large leaf. An inner LOD tile takes the least of its
/// children's (`lod.rs`).
pub(crate) fn tile_level(voxel: f32, tile: f32, frame: &Aabb) -> f32 {
    let shortest = (0..3)
        .map(|k| frame.max[k] - frame.min[k])
        .fold(f32::INFINITY, f32::min);
    voxel * shortest / tile
}

/// Bake input.
pub struct Input<'a> {
    /// Occluder triangles (mesh owners are ignored). Geometry past the domain is ignored, except
    /// that [`SolidMode::Parity`] rays cross all of it.
    pub occluders: Occluders<'a>,
    /// Objects, one per target in this order (see [`Baked::object_map`]). A target reaching past
    /// the domain is also listed in every cell with a boundary portal on that side.
    pub targets: &'a [Target<'a>],
    /// Where the camera may be (with [`Params::extra_view_volumes`]); also the tome's domain, with
    /// the extra volumes (snapped outward to whole tiles; see [`Params::domain_margin`]).
    pub view_volume: Aabb,
    /// Points known to be open space the camera can reach (spawn points, sample camera
    /// positions). With [`SolidMode::Flood`] they seed the open space besides the domain boundary.
    pub seeds: &'a [Vec3],
    /// Settings.
    pub params: Params,
}

/// Counts describing a bake.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Tiles (leaf nodes of the top tree).
    pub tiles: usize,
    /// Voxels: total, touched by triangles, solid. In grid voxels: a coarse tile's voxel
    /// ([`Params::coarsen_outside`]) counts as the `8^k` grid voxels it covers.
    pub voxels: [usize; 3],
    /// Tile cells (leaf-tile cells of the tome, summed over tiles). With
    /// [`Params::global_groups`] there are fewer global cells than tile cells.
    pub cells: usize,
    /// Tile portals (including boundary portals).
    pub portals: usize,
    /// Targets that touch no empty cell (they can never be reported visible).
    pub buried_targets: usize,
    /// Seeds inside the domain.
    pub seeds: usize,
    /// Indexed by **global cell** (the tome's `+0x80` index, which findCell returns; one per tile
    /// cell, or one per group of a tile's cells with [`Params::global_groups`]): up to
    /// [`CELL_SAMPLES`] centres of its open voxels, spread evenly over them. Leaf boxes of the view tree also cover solid "don't care" voxels; these do not, so
    /// they are what per-cell data about open space (light probes, ambient sets) should be
    /// derived from.
    pub cell_samples: Vec<Vec<Vec3>>,
    /// Per global cell, one point [`tenebrae_tome::locate::find_cell`] resolves to that cell: of the
    /// samples of [`Report::cell_samples`] that resolve to it, the one nearest their mean (ties:
    /// the first). `None` when no sample does: with [`Params::view_mask`], a cell wholly outside
    /// the view volume (the camera cannot enter it), or one that reaches into it only where none
    /// of its samples lies. Without the mask every global cell has a point.
    pub cell_points: Vec<Option<Vec3>>,
    /// Global cells (`+0x80`).
    pub global_cells: usize,
    /// Global portals (`+0x84`).
    pub global_portals: usize,
    /// Objects in the tome.
    pub objects: usize,
    /// Coarse LOD tiles on inner top-tree nodes ([`Params::lod_tiles`]); not in `tiles`.
    pub lod_tiles: usize,
    /// Cells of the coarse LOD tiles; not in `cells`.
    pub lod_cells: usize,
    /// Targets dropped because unreachable-cell removal took every cell they touched (0 with
    /// [`Params::keep_every_target`]).
    pub dropped_targets: usize,
    /// Targets [`Params::keep_every_target`] kept although unreachable-cell removal took every
    /// cell they touched: objects listed in no cell.
    pub sealed_targets: usize,
    /// Grid storage: the bake's global structures plus the brick storage of the
    /// [`Params::threads`] largest tiles, i.e. what can be in flight at once. Like the estimate
    /// checked against [`Params::memory_limit`], a practical measure rather than a strict bound
    /// on process memory (per-tile results and transient pass data are not counted, nor are the
    /// tile caches of a cached [`bake`]: neither the one passed in nor the one returned).
    pub grid_bytes: u64,
    /// Tiles whose result a cached [`bake`] took from its cache instead of recomputing: pass A
    /// (the flood; always 0 with [`SolidMode::Parity`], which has no pass A) and pass B (cells).
    /// With `cache_misses` and `cache_rejected`, the only fields a cached bake may report
    /// differently from an uncached one.
    pub cache_hits: [usize; 2],
    /// Tiles a cached [`bake`] recomputed, per pass as `cache_hits`; hits plus misses is the
    /// grid's tile count, except pass A with [`SolidMode::Parity`] (both 0). Both are 0 without a
    /// cache.
    pub cache_misses: [usize; 2],
    /// The cache bytes given to [`bake`] were malformed or of another cache version and were
    /// ignored: the bake was a full one.
    pub cache_rejected: bool,
}

/// Most open-voxel samples reported per cell.
pub const CELL_SAMPLES: usize = 64;

/// Why a bake failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The settings or the input are unusable.
    Params(&'static str),
    /// The grid would need more storage than [`Params::memory_limit`].
    TooLarge {
        /// Estimated bytes (`u64::MAX` when the size overflows).
        needed: u64,
        /// The limit.
        limit: u64,
    },
    /// The progress hook returned [`Control::Stop`].
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Params(what) => write!(f, "invalid bake input: {what}"),
            Error::TooLarge { needed, limit } if *needed == u64::MAX => {
                write!(f, "grid size overflows (limit {limit} bytes)")
            }
            Error::TooLarge { needed, limit } => {
                write!(f, "grid needs {needed} bytes, over the limit of {limit}")
            }
            Error::Cancelled => write!(f, "bake cancelled"),
        }
    }
}

impl std::error::Error for Error {}

/// A stage of a bake, in the order they run. A stage with nothing to do is not reported
/// ([`SolidMode::Parity`] has no flood or join, an uncached bake no keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stage {
    /// Validation, grid sizing and triangle bucketing; counts triangles. With
    /// [`SolidMode::Parity`] the occluder hierarchy is built after its last report, in one step.
    Prepare,
    /// Hashing every tile's cache key (a cached bake only); counts tiles. With
    /// [`SolidMode::Parity`] the whole mesh is hashed once before its first report.
    Keys,
    /// Pass A, the per-tile flood; counts tiles.
    Flood,
    /// Linking flood components across tiles; one unit.
    Join,
    /// Pass B, per-tile solid space, cells, portals and view trees; counts tiles.
    Cells,
    /// Assembly, unreachable-cell removal, leaf-tile collapse and emission; counts steps.
    Global,
}

/// Where a bake is: `done` of `total` units of `stage`. `total` is fixed for the stage and
/// `done` never decreases within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// The stage.
    pub stage: Stage,
    /// Units finished.
    pub done: u64,
    /// Units in the stage.
    pub total: u64,
}

/// What the progress hook wants next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Go on.
    Continue,
    /// Stop: the bake returns [`Error::Cancelled`] and nothing else.
    Stop,
}

/// The progress hook [`bake`] takes. It is called on the caller's thread only.
pub type Hook<'h> = &'h mut dyn FnMut(Progress) -> Control;

/// What a bake returns.
#[derive(Clone, Debug)]
pub struct Baked {
    /// The tome; [`tenebrae_tome::write`] gives its bytes.
    pub tome: Tome,
    /// Per target, its object index in the tome, or `None` when unreachable-cell removal took
    /// every cell it touched (never with [`Params::keep_every_target`], where it is `Some(i)`
    /// for target `i`). Surviving targets keep input order.
    pub object_map: Vec<Option<u32>>,
    /// Counts and per-cell data, indexed as the tome is.
    pub report: Report,
    /// This bake's tile cache, when a cache was passed in (an empty slice starts one): pass it
    /// to the next bake of the same bubble. Its bytes are internal to the cache version.
    pub cache: Option<Vec<u8>>,
}

/// Calls the caller's hook and remembers a stop.
struct Progressor<'h> {
    hook: Option<Hook<'h>>,
}

impl Progressor<'_> {
    fn none() -> Progressor<'static> {
        Progressor { hook: None }
    }
    fn report(&mut self, stage: Stage, done: u64, total: u64) -> Result<(), Error> {
        match &mut self.hook {
            Some(h) => match h(Progress { stage, done, total }) {
                Control::Continue => Ok(()),
                Control::Stop => Err(Error::Cancelled),
            },
            None => Ok(()),
        }
    }
}

/// The voxel classification a bake starts from, for inspection.
pub struct Voxels {
    /// World position of voxel `[0, 0, 0]`'s minimum corner.
    pub origin: Vec3,
    /// Voxel edge.
    pub voxel: f32,
    /// Voxel counts per axis (whole tiles).
    pub dims: [usize; 3],
    /// Tiles per axis.
    pub tiles: [usize; 3],
    tile_size: usize,
    tile_at: Vec<usize>,
    /// Per tile, grid voxels per voxel of its fields ([`Params::coarsen_outside`]).
    scales: Vec<usize>,
    fields: Vec<(Bits, Bits)>,
}

impl Voxels {
    /// The tile holding grid voxel `v` and `v`'s voxel in that tile's fields (a coarse tile's
    /// voxel covering it).
    fn locate(&self, v: [usize; 3]) -> (usize, [usize; 3]) {
        let s = self.tile_size;
        let t = self.tile_at[((v[2] / s) * self.tiles[1] + v[1] / s) * self.tiles[0] + v[0] / s];
        let k = self.scales[t];
        (t, [v[0] % s / k, v[1] % s / k, v[2] % s / k])
    }
    /// An occluder triangle crosses voxel `v`'s interior. In a coarse tile
    /// ([`Params::coarsen_outside`]) it answers for the coarse voxel covering grid voxel `v`.
    pub fn surface(&self, v: [usize; 3]) -> bool {
        let (t, l) = self.locate(v);
        self.fields[t].0.get(l)
    }
    /// Voxel `v` occludes (no cell). In a coarse tile it answers for the coarse voxel covering
    /// grid voxel `v`.
    pub fn solid(&self, v: [usize; 3]) -> bool {
        let (t, l) = self.locate(v);
        self.fields[t].1.get(l)
    }
}

fn finite(v: &Vec3) -> bool {
    v.iter().all(|x| x.is_finite())
}

fn valid_box(b: &Aabb) -> bool {
    finite(&b.min) && finite(&b.max) && (0..3).all(|a| b.min[a] <= b.max[a])
}

/// Checks everything a bake reads before anything is allocated.
fn validate(input: &Input<'_>) -> Result<(), Error> {
    let p = &input.params;
    let positive = |x: f32| x.is_finite() && x > 0.0;
    if !positive(p.voxel) || !positive(p.tile) {
        return Err(Error::Params("tile and voxel must be positive"));
    }
    let per_tile = (p.tile / p.voxel).round();
    if per_tile < 1.0 || (per_tile * p.voxel - p.tile).abs() > 1e-3 {
        return Err(Error::Params("tile must be a positive multiple of voxel"));
    }
    if !positive(p.base_distance) {
        return Err(Error::Params("base distance must be positive"));
    }
    let block = match p.cells {
        CellMode::Boxes => 0.0,
        CellMode::Components { block } => block,
        CellMode::Merged { fill, block } => {
            if !(0.0..=1.0).contains(&fill) {
                return Err(Error::Params("merge fill must be in [0, 1]"));
            }
            block
        }
    };
    if !(block.is_finite() && block >= 0.0) {
        return Err(Error::Params("block must be finite and not negative"));
    }
    if p.threads == 0 {
        return Err(Error::Params("threads must be at least 1"));
    }
    if !(0.0..=1.0).contains(&p.reachability_threshold) {
        return Err(Error::Params("reachability threshold must be in [0, 1]"));
    }
    if !(p.cell_merge.is_finite() && p.cell_merge >= 0.0) {
        return Err(Error::Params(
            "cell merge size must be finite and not negative",
        ));
    }
    if !(p.domain_margin.is_finite() && p.domain_margin >= 0.0) {
        return Err(Error::Params(
            "domain margin must be finite and not negative",
        ));
    }
    if !(p.group_cost.is_finite() && p.group_cost >= 0.0) {
        return Err(Error::Params("group cost must be finite and not negative"));
    }
    if !(p.collapse_size.is_finite() && p.collapse_size >= 0.0) {
        return Err(Error::Params(
            "collapse size must be finite and not negative",
        ));
    }
    for vv in std::iter::once(&input.view_volume).chain(&p.extra_view_volumes) {
        if !valid_box(vv) {
            return Err(Error::Params("view volume must be finite with min <= max"));
        }
        if (0..3).any(|a| vv.max[a] <= vv.min[a]) {
            return Err(Error::Params("empty view volume"));
        }
    }
    let meshes = match input.occluders {
        Occluders::Soup(m) => std::slice::from_ref(m),
        Occluders::Instanced { meshes, instances } => {
            if instances.iter().any(|i| i.mesh as usize >= meshes.len()) {
                return Err(Error::Params("instance mesh index out of range"));
            }
            if !instances
                .iter()
                .all(|i| i.transform.iter().flatten().all(|x| x.is_finite()))
            {
                return Err(Error::Params("non-finite instance transform"));
            }
            meshes
        }
    };
    for mesh in meshes {
        if !mesh.vertices.iter().all(finite) {
            return Err(Error::Params("non-finite occluder vertex"));
        }
        let nv = mesh.vertices.len();
        if mesh
            .triangles
            .iter()
            .any(|t| t.iter().any(|&i| i as usize >= nv))
        {
            return Err(Error::Params("triangle vertex index out of range"));
        }
        if !mesh.owners.is_empty() && mesh.owners.len() != mesh.triangles.len() {
            return Err(Error::Params("owners and triangles differ in length"));
        }
    }
    if !input.targets.iter().all(|t| valid_box(&t.bounds)) {
        return Err(Error::Params("target box must be finite with min <= max"));
    }
    if input.targets.iter().any(|t| t.users.is_empty()) {
        return Err(Error::Params("target without a user"));
    }
    Ok(())
}

/// Triangles bucketed between two progress reports.
const PREPARE_STEP: u32 = 1 << 16;

/// Estimated bytes every tile keeps from its passes until the tome is written (boundary layers,
/// cells, faces, view tree, global-stage records) when it holds no geometry.
const TILE_BYTES: u64 = 2048;

/// A validated, bucketed bake, ready for the tile passes.
struct Prepared<'a> {
    ctx: Ctx<'a>,
    top: Vec<global::Top>,
    /// Bytes of the global structures so far (buckets, touched-brick sets).
    global_bytes: u64,
    /// Per tile, the brick-bytes bound the estimate used.
    #[cfg_attr(not(test), allow(dead_code))]
    tile_bounds: Vec<u64>,
    /// The estimate checked against the memory limit.
    #[cfg_attr(not(test), allow(dead_code))]
    estimate: u64,
}

fn too_large(needed: u64, limit: u64) -> Error {
    Error::TooLarge { needed, limit }
}

/// Validates, sizes the grid with checked arithmetic, buckets triangles per tile and refuses a
/// grid whose estimate exceeds the memory limit, all before any brick is allocated.
fn prepare<'a>(input: &'a Input<'a>, progress: &mut Progressor<'_>) -> Result<Prepared<'a>, Error> {
    validate(input)?;
    let tris =
        Tris::new(input.occluders).ok_or(Error::Params("more than u32::MAX occluder triangles"))?;
    let p = &input.params;
    let limit = p.memory_limit;
    let overflow = || too_large(u64::MAX, limit);
    let per_tile = (p.tile / p.voxel).round() as usize;
    let volumes: Vec<Aabb> = std::iter::once(input.view_volume)
        .chain(p.extra_view_volumes.iter().copied())
        .collect();
    // The domain: the box around the view volumes, grown to cover the targets up to
    // `domain_margin` past it.
    let mut vv = input.view_volume;
    for v in &volumes[1..] {
        for a in 0..3 {
            vv.min[a] = vv.min[a].min(v.min[a]);
            vv.max[a] = vv.max[a].max(v.max[a]);
        }
    }
    if p.domain_margin > 0.0 {
        let view = vv;
        for t in input.targets {
            for a in 0..3 {
                vv.min[a] = vv.min[a].min(t.bounds.min[a].max(view.min[a] - p.domain_margin));
                vv.max[a] = vv.max[a].max(t.bounds.max[a].min(view.max[a] + p.domain_margin));
            }
        }
    }
    let mut tiles_n = [0usize; 3];
    let mut origin = [0.0f32; 3];
    let mut dims = [0usize; 3];
    for a in 0..3 {
        origin[a] = (vv.min[a] / p.tile).floor() * p.tile;
        let n = ((vv.max[a] - origin[a]) / p.tile).ceil();
        if !n.is_finite() || n >= usize::MAX as f32 {
            return Err(overflow());
        }
        tiles_n[a] = (n as usize).max(1);
        dims[a] = tiles_n[a].checked_mul(per_tile).ok_or_else(overflow)?;
    }
    let total = dims[0]
        .checked_mul(dims[1])
        .and_then(|x| x.checked_mul(dims[2]))
        .ok_or_else(overflow)?;
    let ntiles = tiles_n[0]
        .checked_mul(tiles_n[1])
        .and_then(|x| x.checked_mul(tiles_n[2]))
        .ok_or_else(overflow)?;
    let _ = total;
    let block = match p.cells {
        CellMode::Boxes => None,
        CellMode::Components { block } | CellMode::Merged { block, .. } => {
            let b = (block / p.voxel).round();
            if b >= usize::MAX as f32 {
                return Err(overflow());
            }
            let b = b as usize;
            // A block under half a voxel rounds to none: greedy boxes (merged) or the tile.
            match p.cells {
                CellMode::Merged { .. } if b == 0 => None,
                _ => Some(b),
            }
        }
    };
    // Bricks: the largest power of two up to 16 voxels dividing both the tile and the block, so a
    // uniform brick never straddles a seed block.
    let layout = Bricks::for_tile(per_tile, block);
    let flood = p.solid == SolidMode::Flood;
    let dense_cells = !flood || block.is_none();
    // Refuse before bucketing when even empty tiles do not fit.
    let threads = p.threads.min(ntiles) as u64;
    let empty_tile = tile_bytes_bound(layout, 0, 0, flood, dense_cells) as u64;
    let floor = (ntiles as u64)
        .checked_mul(TILE_BYTES)
        .and_then(|x| x.checked_add(threads.checked_mul(empty_tile)?))
        .ok_or_else(overflow)?;
    if floor > limit {
        return Err(too_large(floor, limit));
    }
    let (top, tile_ranges) = global::top_tree(tiles_n, origin, p.tile);
    let mut tile_at = vec![0usize; ntiles];
    for (t, r) in tile_ranges.iter().enumerate() {
        tile_at[(r[2][0] * tiles_n[1] + r[1][0]) * tiles_n[0] + r[0][0]] = t;
    }
    let grid = Grid {
        origin,
        voxel: p.voxel,
        dims,
    };
    let domain = Aabb {
        min: origin,
        max: [
            grid.coord(0, dims[0]),
            grid.coord(1, dims[1]),
            grid.coord(2, dims[2]),
        ],
    };
    let seeds: Vec<[usize; 3]> = input
        .seeds
        .iter()
        .filter_map(|s| {
            let k: Vec<f32> = (0..3)
                .map(|a| ((s[a] - origin[a]) / p.voxel).floor())
                .collect();
            (0..3)
                .all(|a| k[a] >= 0.0 && (k[a] as usize) < dims[a])
                .then(|| [k[0] as usize, k[1] as usize, k[2] as usize])
        })
        .collect();
    if flood && !input.seeds.is_empty() && seeds.is_empty() {
        return Err(Error::Params("no seed inside the domain"));
    }
    // Level shift and voxel space per tile (`Params::coarsen_outside`).
    let coarse = p.coarsen_outside && p.cell_merge > 0.0;
    let shifts: Vec<u32> = tile_ranges
        .iter()
        .map(|r| {
            if coarse {
                let frame = tile::tile_frame(&grid, p.tile, r);
                tile::level_shift(&volumes, &frame, p.cell_merge, p.voxel, per_tile)
            } else {
                0
            }
        })
        .collect();
    let spaces: Vec<Space> = tile_ranges
        .iter()
        .zip(&shifts)
        .map(|(r, &k)| space_of(&grid, layout, block, r, k))
        .collect();
    // Bucket triangles per tile and mark the bricks they may touch, in each tile's own space.
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); ntiles];
    let mut touched: Vec<Option<Vec<u64>>> = vec![None; ntiles];
    let ntris = tris.len();
    progress.report(Stage::Prepare, 0, u64::from(ntris))?;
    for ti in 0..ntris {
        if ti % PREPARE_STEP == PREPARE_STEP - 1 {
            progress.report(Stage::Prepare, u64::from(ti), u64::from(ntris))?;
        }
        let c = tris.corners(ti);
        let (_, e) = tri_ranges(&grid, &c);
        if e.iter().any(|(a, b)| a >= b) {
            continue;
        }
        let t0 = e.map(|(a, _)| a / per_tile);
        let t1 = e.map(|(_, b)| (b - 1) / per_tile);
        for tz in t0[2]..=t1[2] {
            for ty in t0[1]..=t1[1] {
                for tx in t0[0]..=t1[0] {
                    let t = tile_at[(tz * tiles_n[1] + ty) * tiles_n[0] + tx];
                    let sp = &spaces[t];
                    // A coarse tile's ranges are its own voxels'; its voxels test only what
                    // reaches the tile's box, which the grid's ranges above already cover.
                    let e = if sp.s == 1 {
                        e
                    } else {
                        tri_ranges(&sp.grid, &c).1
                    };
                    let Some(cl) = clip(e, sp.lo, sp.layout.tile()) else {
                        continue;
                    };
                    let words = sp.layout.count().div_ceil(64);
                    let set = touched[t].get_or_insert_with(|| vec![0u64; words]);
                    let mut any = false;
                    touched_bricks(&sp.grid, sp.layout, sp.lo, &c, cl, |b, _| {
                        set[b / 64] |= 1 << (b % 64);
                        any = true;
                    });
                    // A triangle that touches no brick of the tile tests no voxel there either.
                    if any {
                        buckets[t].push(ti);
                    }
                }
            }
        }
    }
    // Estimate: global structures, then the `threads` largest tiles' brick bound.
    let mut global_bytes = buckets
        .iter()
        .map(|b| (b.capacity() * 4 + size_of::<Vec<u32>>()) as u64)
        .sum::<u64>()
        + touched
            .iter()
            .map(|t| (t.as_ref().map_or(0, |w| w.len() * 8) + size_of::<Option<Vec<u64>>>()) as u64)
            .sum::<u64>();
    let mut bounds = Vec::with_capacity(ntiles);
    let mut layer_bound = 0u64;
    for (set, sp) in touched.iter().zip(&spaces) {
        // Each tile at its own resolution; its layers leave it at the grid's, `s` rows per row.
        let layout = sp.layout;
        let edge = layout.edge() as u64;
        let is = |b: [usize; 3]| {
            set.as_ref()
                .is_some_and(|w| w[layout.index(b) / 64] >> (layout.index(b) % 64) & 1 == 1)
        };
        let (mut tcount, mut near) = (0usize, 0usize);
        if set.is_some() {
            for b in 0..layout.count() {
                let bc = layout.coords(b);
                if is(bc) {
                    tcount += 1;
                    near += 1;
                    continue;
                }
                let close = (0..3).any(|a| {
                    (bc[a] > 0 && {
                        let mut n = bc;
                        n[a] -= 1;
                        is(n)
                    }) || (bc[a] + 1 < layout.n && {
                        let mut n = bc;
                        n[a] += 1;
                        is(n)
                    })
                });
                if close {
                    near += 1;
                }
            }
        }
        bounds.push(tile_bytes_bound(layout, tcount, near, flood, dense_cells) as u64);
        // Runs per face: a voxel-level run per face voxel of a touched brick, plus one run per
        // face. Untouched bricks are uniform, so they only break runs where cells or labels
        // change between bricks; that term is not bounded here, which keeps the estimate from
        // charging every empty tile a run per brick row.
        layer_bound += 6 * size_of::<grid::Layer>() as u64 + 6 * 8;
        for axis in 0..3 {
            for side in 0..2 {
                let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
                for k2 in 0..layout.n {
                    for k1 in 0..layout.n {
                        let mut b = [0usize; 3];
                        b[axis] = if side == 1 { layout.n - 1 } else { 0 };
                        b[a1] = k1;
                        b[a2] = k2;
                        if is(b) {
                            layer_bound += 8 * edge * edge * sp.s as u64;
                        }
                    }
                }
            }
        }
    }
    let sets = if flood { 2 } else { 1 };
    let parity_bytes = if flood {
        0
    } else {
        // The hierarchy, plus the world-space copy an instanced input materialises for it.
        let copy = match input.occluders {
            Occluders::Soup(_) => 0,
            Occluders::Instanced { meshes, instances } => instances
                .iter()
                .map(|i| {
                    let m = &meshes[i.mesh as usize];
                    m.vertices.len() as u64 * 12 + m.triangles.len() as u64 * 16
                })
                .sum(),
        };
        u64::from(ntris) * 80 + copy
    };
    global_bytes += sets * layer_bound + parity_bytes;
    let needed = global_bytes
        .saturating_add((ntiles as u64).saturating_mul(TILE_BYTES))
        .saturating_add(top_sum(&bounds, p.threads));
    if needed > limit {
        return Err(too_large(needed, limit));
    }
    progress.report(Stage::Prepare, u64::from(ntris), u64::from(ntris))?;
    let soup = (!flood).then(|| tris.soup());
    let parity = soup.as_deref().map(|mesh| {
        let bvh = Bvh::build(mesh);
        // Parity rays must cross the whole mesh, not just the domain: geometry reaching past the
        // domain still closes (or fails to close) the volumes inside it.
        let mut reach = Bounds {
            min: domain.min,
            max: domain.max,
        };
        for v in &mesh.vertices {
            reach.grow(*v);
        }
        (bvh, reach)
    });
    let merge = match p.cells {
        CellMode::Merged { fill, .. } => Some((fill, p.max_cells_per_tile)),
        CellMode::Components { .. } => Some((f32::INFINITY, p.max_cells_per_tile)),
        CellMode::Boxes => None,
    };
    Ok(Prepared {
        ctx: Ctx {
            input,
            grid,
            domain,
            layout,
            tiles_n,
            tile_ranges,
            tile_at,
            buckets,
            seeds,
            block,
            merge,
            cell_merge: p.cell_merge,
            parity,
            tris,
            soup,
            volumes,
            shifts,
        },
        top,
        global_bytes: global_bytes - sets * layer_bound - parity_bytes,
        tile_bounds: bounds,
        estimate: needed,
    })
}

/// Sum of the `threads` largest values.
fn top_sum(values: &[u64], threads: usize) -> u64 {
    let mut v = values.to_vec();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.iter()
        .take(threads)
        .fold(0u64, |a, b| a.saturating_add(*b))
}

/// Runs `f` for every tile index on up to `threads` workers and returns the results in tile
/// order. One thread runs on the caller's thread and spawns nothing. With a stage, each finished
/// tile is reported to `progress` on the caller's thread; a stop ends the run once the tiles in
/// flight finish, and returns [`Error::Cancelled`].
fn run_tiles<T: Send>(
    n: usize,
    threads: usize,
    progress: &mut Progressor<'_>,
    stage: Option<Stage>,
    f: impl Fn(usize) -> T + Sync,
) -> Result<Vec<T>, Error> {
    let total = n as u64;
    let mut tell = |done: usize| match stage {
        Some(stage) => progress.report(stage, done as u64, total),
        None => Ok(()),
    };
    tell(0)?;
    let workers = threads.min(n);
    if workers <= 1 {
        let mut out = Vec::with_capacity(n);
        for t in 0..n {
            out.push(f(t));
            tell(t + 1)?;
        }
        return Ok(out);
    }
    // Workers take the next tile from a shared counter; each tile writes only its own slot and
    // then tells the caller's thread, which reports progress and may raise the stop flag.
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let slots: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let mut cancelled = Ok(());
    std::thread::scope(|s| {
        let (tx, rx) = mpsc::channel::<()>();
        for _ in 0..workers {
            let tx = tx.clone();
            let (next, stop, slots, f) = (&next, &stop, &slots, &f);
            s.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let t = next.fetch_add(1, Ordering::Relaxed);
                    if t >= n {
                        break;
                    }
                    let r = f(t);
                    *slots[t].lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
                    // The receiver lives until every worker has ended.
                    let _ = tx.send(());
                }
            });
        }
        drop(tx);
        let mut done = 0usize;
        while rx.recv().is_ok() {
            done += 1;
            if cancelled.is_ok() {
                cancelled = tell(done);
                if cancelled.is_err() {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    cancelled?;
    Ok(slots
        .into_iter()
        .map(|m| {
            m.into_inner()
                .unwrap_or_else(PoisonError::into_inner)
                .unwrap_or_else(|| unreachable!("every tile ran"))
        })
        .collect())
}

/// Pass A and the join: per tile, which flood labels are live (flood mode only).
fn flood<'a>(prep: &Prepared<'a>) -> (Vec<tile::Flood>, Vec<Vec<bool>>) {
    let ctx = &prep.ctx;
    if ctx.parity.is_some() {
        return (Vec::new(), Vec::new());
    }
    let n = ctx.tile_ranges.len();
    let floods = run_tiles(
        n,
        ctx.input.params.threads,
        &mut Progressor::none(),
        None,
        |t| tile::flood_pass(ctx, t),
    )
    .unwrap_or_else(|_| unreachable!("no hook, no cancel"));
    let live = global::join(ctx, &floods);
    (floods, live)
}

/// Bakes one tome from one view volume.
///
/// * `cache`: `None` bakes without a cache. `Some(bytes)` takes every tile pass whose inputs are
///   unchanged from the cache an earlier bake returned in [`Baked::cache`] and recomputes the
///   rest (the global stage always runs in full); the result is byte-identical to an uncached
///   bake except [`Report::cache_hits`], [`Report::cache_misses`] and [`Report::cache_rejected`].
///   An empty slice starts a
///   cache. Bytes that are malformed or of another cache version are ignored
///   ([`Report::cache_rejected`]); a cache of another grid or other tile settings simply misses.
///   Neither is an error.
/// * `progress`: called on the caller's thread, per stage and per finished tile.
///   [`Control::Stop`] returns [`Error::Cancelled`].
///
/// Blocks the caller; with [`Params::threads`] above 1 the tile passes run on scoped workers.
/// The output is the same for every thread count.
pub fn bake(
    input: &Input<'_>,
    cache: Option<&[u8]>,
    progress: Option<Hook<'_>>,
) -> Result<Baked, Error> {
    let cache = cache.map(|b| {
        if b.is_empty() {
            TileCache::new()
        } else {
            TileCache::from_bytes(b)
        }
    });
    let mut progress = Progressor { hook: progress };
    let (tome, object_map, report, next) = bake_with(input, cache.as_ref(), &mut progress)?;
    Ok(Baked {
        tome,
        object_map,
        report,
        cache: next.map(|c| c.to_bytes()),
    })
}

/// A bake's results before the cache is serialised.
type Out = (Tome, Vec<Option<u32>>, Report, Option<TileCache>);

/// The bake. With a cache, tile results are looked up, and every tile's key and encoded result
/// go into the returned cache; without one, no key is hashed, nothing is encoded and no cache is
/// returned. The tome and report are the same either way (except the cache fields).
fn bake_with(
    input: &Input<'_>,
    cache: Option<&TileCache>,
    progress: &mut Progressor<'_>,
) -> Result<Out, Error> {
    let prep = prepare(input, progress)?;
    let ctx = &prep.ctx;
    let p = &input.params;
    let n = ctx.tile_ranges.len();
    let keys = cache.map(|_| cache::Keys::new(ctx));
    let a_keys = match &keys {
        Some(k) => run_tiles(n, p.threads, progress, Some(Stage::Keys), |t| k.a(ctx, t))?,
        None => Vec::new(),
    };
    let mut next = cache.map(|_| TileCache::default());
    let mut hits = [0usize; 2];
    // Pass A and the join (flood mode only): a hit decodes the cached flood, a miss runs the pass.
    let (floods, live) = if ctx.parity.is_some() {
        (Vec::new(), Vec::new())
    } else {
        let pass = run_tiles(n, p.threads, progress, Some(Stage::Flood), |t| {
            let Some(cache) = cache else {
                return (tile::flood_pass(ctx, t), Vec::new(), false);
            };
            if let Some(b) = cache.flood_entry(a_keys[t])
                && let Some(f) = cache::decode_flood(ctx, b)
            {
                return (f, b.to_vec(), true);
            }
            let f = tile::flood_pass(ctx, t);
            let b = cache::encode_flood(&f);
            (f, b, false)
        })?;
        let mut floods = Vec::with_capacity(n);
        for (t, (f, b, hit)) in pass.into_iter().enumerate() {
            hits[0] += usize::from(hit);
            if let Some(next) = &mut next {
                next.insert_flood(a_keys[t], b);
            }
            floods.push(f);
        }
        progress.report(Stage::Join, 0, 1)?;
        let live = global::join(ctx, &floods);
        progress.report(Stage::Join, 1, 1)?;
        (floods, live)
    };
    // Pass B: the key needs the post-join liveness, so it is computed here.
    let pass = run_tiles(n, p.threads, progress, Some(Stage::Cells), |t| {
        let overlap = tile::overlaps(ctx, t);
        let key = keys.as_ref().map(|k| k.b(ctx, t, &a_keys, &live, &overlap));
        if let (Some(cache), Some(key)) = (cache, key)
            && let Some(b) = cache.tile_entry(key)
            && let Some(out) = cache::decode_tile(ctx, b, overlap.len())
        {
            return (out, b.to_vec(), true, key, overlap);
        }
        let (surface, solid, peak) = tile::solid_pass(ctx, t, &live, &floods);
        let surface_count = surface.count();
        drop(surface);
        let out = tile::cell_pass(ctx, t, surface_count, solid, peak, &overlap);
        let b = match key {
            Some(_) => cache::encode_tile(&out),
            None => Vec::new(),
        };
        (out, b, false, key.unwrap_or(0), overlap)
    })?;
    let mut outs = Vec::with_capacity(n);
    for (mut out, b, hit, key, overlap) in pass {
        hits[1] += usize::from(hit);
        // Cached with the tile's overlap positions; the global stage takes input indices.
        if let Some(next) = &mut next {
            next.insert_tile(key, b);
        }
        tile::remap_objects(&mut out, &overlap);
        outs.push(out);
    }
    let mut global_bytes = prep.global_bytes;
    let mut peaks: Vec<u64> = outs.iter().map(|o| o.bytes as u64).collect();
    for (t, f) in floods.iter().enumerate() {
        peaks[t] = peaks[t].max(f.bytes as u64);
        global_bytes += f.layer_bytes + live[t].capacity() as u64;
    }
    drop(floods);
    drop(live);
    global_bytes += outs.iter().map(|o| o.layer_bytes).sum::<u64>();
    let dims = ctx.grid.dims;
    let mut report = Report {
        voxels: [
            dims[0] * dims[1] * dims[2],
            outs.iter().map(|o| o.surface).sum(),
            outs.iter().map(|o| o.solid).sum(),
        ],
        seeds: ctx.seeds.len(),
        grid_bytes: global_bytes + top_sum(&peaks, p.threads),
        cache_hits: hits,
        cache_misses: match cache {
            Some(_) if ctx.parity.is_some() => [0, n - hits[1]],
            Some(_) => [n - hits[0], n - hits[1]],
            None => [0, 0],
        },
        cache_rejected: cache.is_some_and(TileCache::rejected),
        ..Report::default()
    };
    progress.report(Stage::Global, 0, GLOBAL_STEPS)?;
    let mut scene = global::assemble(ctx, prep.top.clone(), outs);
    report.buried_targets = scene.touched.iter().filter(|t| !**t).count();
    progress.report(Stage::Global, 1, GLOBAL_STEPS)?;
    if p.empty_leaves {
        let populated: Vec<bool> = ctx.buckets.iter().map(|b| !b.is_empty()).collect();
        global::empty_leaves(ctx, &mut scene, &populated);
    }
    progress.report(Stage::Global, 2, GLOBAL_STEPS)?;
    let (map, lost) = global::reachability(ctx, &mut scene, p.reachability_threshold);
    if p.keep_every_target {
        report.sealed_targets = lost;
    } else {
        report.dropped_targets = lost;
    }
    progress.report(Stage::Global, 3, GLOBAL_STEPS)?;
    let fill = match p.cells {
        CellMode::Merged { fill, .. } => Some(fill),
        _ => None,
    };
    global::collapse(ctx, &mut scene, p.collapse_size, fill);
    progress.report(Stage::Global, 4, GLOBAL_STEPS)?;
    let mut tome = global::emit(ctx, scene, &map, &mut report);
    let mut map = map;
    if p.group_cost > 0.0 {
        let groups = group::group(&tome.objects, f64::from(p.group_cost));
        group::apply(&mut tome, &mut map, &groups, p.share_lists);
    }
    if p.lod_tiles {
        let counts = lod::add(&mut tome);
        report.lod_tiles = counts.tiles;
        report.lod_cells = counts.cells;
    }
    let locator = tenebrae_tome::locate::Locator::new(&tome);
    report.cell_points = report
        .cell_samples
        .iter()
        .enumerate()
        .map(|(g, s)| {
            let found: Vec<Vec3> = s
                .iter()
                .copied()
                .filter(|&p| locator.find_cell(p) == Some(g as u32))
                .collect();
            representative(&found)
        })
        .collect();
    report.global_cells = tome.global_cells.len();
    report.global_portals = tome.global_portals.len();
    report.objects = tome.objects.len();
    progress.report(Stage::Global, GLOBAL_STEPS, GLOBAL_STEPS)?;
    Ok((tome, map, report, next))
}

/// Steps of [`Stage::Global`]: assembly, empty leaves, reachability, collapse, emission.
const GLOBAL_STEPS: u64 = 5;

/// The sample nearest the samples' mean (ties: the first); `None` without samples.
fn representative(samples: &[Vec3]) -> Option<Vec3> {
    let n = samples.len() as f64;
    let mean = [0, 1, 2].map(|a| samples.iter().map(|s| f64::from(s[a])).sum::<f64>() / n);
    let d = |s: &Vec3| -> f64 { (0..3).map(|a| (f64::from(s[a]) - mean[a]).powi(2)).sum() };
    let mut best = *samples.first()?;
    for s in &samples[1..] {
        if d(s) < d(&best) {
            best = *s;
        }
    }
    Some(best)
}

/// The tome's user lists for `targets`, one list per object in order: no prefix when every
/// object has exactly one user.
fn users<'t>(targets: impl Iterator<Item = &'t Target<'t>> + Clone) -> Users {
    let single = targets.clone().all(|t| t.users.len() == 1);
    let ids: Vec<u32> = targets
        .clone()
        .flat_map(|t| t.users.iter().copied())
        .collect();
    let prefix = (!single).then(|| {
        let mut p = vec![0u32];
        let mut at = 0u32;
        for t in targets {
            at += t.users.len() as u32;
            p.push(at);
        }
        p
    });
    Users { prefix, ids }
}

/// Voxelises the occluders over the view volume's tile grid and decides solid space.
pub fn voxelize(input: &Input<'_>) -> Result<Voxels, Error> {
    let mut none = Progressor::none();
    let prep = prepare(input, &mut none)?;
    let ctx = &prep.ctx;
    let n = ctx.tile_ranges.len();
    let (floods, live) = flood(&prep);
    let fields = run_tiles(n, input.params.threads, &mut none, None, |t| {
        let (surface, solid, _) = tile::solid_pass(ctx, t, &live, &floods);
        (surface, solid)
    })?;
    Ok(Voxels {
        origin: ctx.grid.origin,
        voxel: ctx.grid.voxel,
        dims: ctx.grid.dims,
        tiles: ctx.tiles_n,
        tile_size: ctx.layout.tile(),
        tile_at: ctx.tile_at.clone(),
        scales: (0..n).map(|t| ctx.space(t).s).collect(),
        fields,
    })
}

#[cfg(test)]
mod tests;
