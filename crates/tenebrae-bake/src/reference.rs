//! The whole-grid baker as it was at `0ce18d0`, kept as the dense reference the tile-streamed
//! baker must match byte for byte. Test-only.

#![allow(clippy::all, dead_code)]

use super::{CELL_SAMPLES, CellMode, Error, Input, SolidMode};
use tenebrae_geom::{Bounds, Bvh, Vec3, triangle_box};
use tenebrae_tome::{
    Aabb, Gates, GlobalCell, Header, KdTree, PARAMS_SIZE, Portal, Run, RunList, Tile, TileCell,
    Tome, ViewTree,
};

/// The old report's fields.
#[derive(Clone, Debug, Default)]
pub(super) struct Report {
    pub(super) tiles: usize,
    pub(super) voxels: [usize; 3],
    pub(super) cells: usize,
    pub(super) portals: usize,
    pub(super) buried_targets: usize,
    pub(super) cell_samples: Vec<Vec<Vec3>>,
}

/// The old cell modes, blocks in voxels.
#[derive(Clone, Copy)]
enum Mode {
    Boxes,
    Components { block: usize },
    Merged { fill: f32, block: usize },
}

impl Mode {
    fn of(p: &super::Params) -> Mode {
        let v = |b: f32| (b / p.voxel).round() as usize;
        match p.cells {
            CellMode::Boxes => Mode::Boxes,
            CellMode::Components { block } => Mode::Components { block: v(block) },
            CellMode::Merged { fill, block } => Mode::Merged {
                fill,
                block: v(block),
            },
        }
    }
}

const SOLID: u32 = u32::MAX;

struct Grid {
    origin: Vec3,
    voxel: f32,
    dims: [usize; 3],
}

impl Grid {
    fn index(&self, v: [usize; 3]) -> usize {
        (v[2] * self.dims[1] + v[1]) * self.dims[0] + v[0]
    }
    fn centre(&self, v: [usize; 3]) -> Vec3 {
        [
            self.origin[0] + (v[0] as f32 + 0.5) * self.voxel,
            self.origin[1] + (v[1] as f32 + 0.5) * self.voxel,
            self.origin[2] + (v[2] as f32 + 0.5) * self.voxel,
        ]
    }
    fn coord(&self, axis: usize, k: usize) -> f32 {
        self.origin[axis] + k as f32 * self.voxel
    }
    /// Voxel range `[lo, hi)` covering world interval `[a, b]` on `axis`, clamped.
    fn range(&self, axis: usize, a: f32, b: f32) -> (usize, usize) {
        let lo = ((a - self.origin[axis]) / self.voxel).floor().max(0.0) as usize;
        let hi = (((b - self.origin[axis]) / self.voxel).floor() as i64 + 1)
            .clamp(0, self.dims[axis] as i64) as usize;
        (lo.min(self.dims[axis]), hi)
    }
}

/// A cell: a box of voxels `[lo, hi)` in grid coordinates, inside one tile.
#[derive(Clone, Copy, Debug)]
struct CellBox {
    lo: [usize; 3],
    hi: [usize; 3],
    tile: usize,
    local: u32,
}

fn bits_for(max_value: u32) -> u32 {
    32 - max_value.leading_zeros()
}

/// Quantises `x` within `[lo, hi]` to u16; `up` rounds outward toward the maximum.
fn quant(x: f32, lo: f32, hi: f32, up: bool) -> u32 {
    let f = ((x - lo) / (hi - lo) * 65535.0).clamp(0.0, 65535.0);
    (if up { f.ceil() } else { f.floor() }) as u32
}

fn pack_box(b: &Aabb, frame: &Aabb) -> [u32; 3] {
    let q = |a: usize, v: f32, up: bool| quant(v, frame.min[a], frame.max[a], up);
    let (x0, y0, z0) = (
        q(0, b.min[0], false),
        q(1, b.min[1], false),
        q(2, b.min[2], false),
    );
    let (x1, y1, z1) = (
        q(0, b.max[0], true),
        q(1, b.max[1], true),
        q(2, b.max[2], true),
    );
    [(x0 << 16) | y0, (z0 << 16) | x1, (y1 << 16) | z1]
}

/// One portal before encoding: `rect` is `[a1 min, a1 max, a2 min, a2 max]` with
/// `a1 = (axis+1)%3`, `a2 = (axis+2)%3`.
struct PortalSpec {
    axis: usize,
    side: u32,
    boundary: bool,
    target: u32,
    local: u32,
    plane: f32,
    rect: [f32; 4],
}

/// Portal words, coordinates quantised against `frame`.
fn portal(s: &PortalSpec, frame: &Aabb) -> Portal {
    let (axis, side) = (s.axis, s.side);
    let a1 = (axis + 1) % 3;
    let a2 = (axis + 2) % 3;
    let d0 = ((axis as u32) << 30)
        | (side << 29)
        | (u32::from(s.boundary) << 28)
        | (s.target & 0x3FF_FFFF);
    let coord = quant(s.plane, frame.min[axis], frame.max[axis], side == 1).min(65535);
    let d1 = coord | (s.local << 16);
    let d2 = (quant(s.rect[0], frame.min[a1], frame.max[a1], false) << 16)
        | quant(s.rect[1], frame.min[a1], frame.max[a1], true);
    let d3 = (quant(s.rect[2], frame.min[a2], frame.max[a2], false) << 16)
        | quant(s.rect[3], frame.min[a2], frame.max[a2], true);
    Portal([d0, d1, d2, d3])
}

/// Level-order KD tree over integer ranges. `split` decides a region: `None` makes it a leaf
/// (with `leaf` called for its value), `Some((axis, at))` splits it at integer coordinate `at`.
fn build_kd<R: Copy>(
    root: R,
    mut split: impl FnMut(&R) -> Option<(usize, usize, R, R)>,
    mut leaf: impl FnMut(&R) -> u32,
    mut world: impl FnMut(usize, usize) -> f32,
) -> (KdTree, Vec<u32>) {
    let mut codes = Vec::new();
    let mut splits = Vec::new();
    let mut leaves = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root);
    while let Some(region) = queue.pop_front() {
        match split(&region) {
            Some((axis, at, below, above)) => {
                codes.push(axis as u8);
                splits.push(world(axis, at));
                queue.push_back(below);
                queue.push_back(above);
            }
            None => {
                codes.push(3);
                splits.push(0.0);
                leaves.push(leaf(&region));
            }
        }
    }
    (KdTree { codes, splits }, leaves)
}

/// The voxel classification a bake starts from, for inspection.
pub(super) struct Voxels {
    /// World position of voxel `[0, 0, 0]`'s minimum corner.
    pub(super) origin: Vec3,
    /// Voxel edge.
    pub(super) voxel: f32,
    /// Voxel counts per axis (whole tiles).
    pub(super) dims: [usize; 3],
    /// Tiles per axis.
    pub(super) tiles: [usize; 3],
    /// Per voxel, x fastest: an occluder triangle crosses its interior.
    pub(super) surface: Vec<bool>,
    /// Per voxel: occluding (no cell).
    pub(super) solid: Vec<bool>,
}

impl Voxels {
    /// Linear index of voxel `v`.
    pub(super) fn index(&self, v: [usize; 3]) -> usize {
        (v[2] * self.dims[1] + v[1]) * self.dims[0] + v[0]
    }
}

/// Chooses a view-tree split for a region holding open voxels of several cells: a face of one
/// cell's extent inside the region, cutting through as few other cells as possible and then
/// dividing them as evenly as possible. Splitting where cells meet keeps the tree near one leaf
/// per cell piece instead of carving every cell boundary voxel by voxel.
fn choose_split(rg: &[[usize; 2]; 3], cell: impl Fn([usize; 3]) -> u32) -> (usize, usize) {
    // Extent of each cell's open voxels inside the region.
    let mut ext: Vec<(u32, [usize; 3], [usize; 3])> = Vec::new();
    for z in rg[2][0]..rg[2][1] {
        for y in rg[1][0]..rg[1][1] {
            for x in rg[0][0]..rg[0][1] {
                let c = cell([x, y, z]);
                if c == SOLID {
                    continue;
                }
                let v = [x, y, z];
                let e = match ext.iter_mut().find(|e| e.0 == c) {
                    Some(e) => e,
                    None => {
                        ext.push((c, v, v));
                        ext.last_mut().unwrap_or_else(|| unreachable!())
                    }
                };
                for (k, &x) in v.iter().enumerate() {
                    e.1[k] = e.1[k].min(x);
                    e.2[k] = e.2[k].max(x + 1);
                }
            }
        }
    }
    let mut best: Option<((usize, usize), (usize, usize))> = None;
    for (axis, span) in rg.iter().enumerate() {
        for e in &ext {
            for at in [e.1[axis], e.2[axis]] {
                if at <= span[0] || at >= span[1] {
                    continue;
                }
                let (mut below, mut above, mut cut) = (0usize, 0usize, 0usize);
                for f in &ext {
                    if f.2[axis] <= at {
                        below += 1;
                    } else if f.1[axis] >= at {
                        above += 1;
                    } else {
                        cut += 1;
                    }
                }
                let score = (cut, below.abs_diff(above));
                if best.is_none_or(|(s, _)| score < s) {
                    best = Some((score, (axis, at)));
                }
            }
        }
    }
    best.map(|(_, s)| s).unwrap_or_else(|| {
        let size = [
            rg[0][1] - rg[0][0],
            rg[1][1] - rg[1][0],
            rg[2][1] - rg[2][0],
        ];
        let axis = (0..3).max_by_key(|&a| (size[a], 3 - a)).unwrap_or(0);
        (axis, rg[axis][0] + size[axis] / 2)
    })
}

/// Bakes a tome. Returns the tome and a report.
pub(super) fn bake(input: &Input<'_>) -> Result<(Tome, Report), Error> {
    let vox = voxelize(input)?;
    let p = &input.params;
    let per_tile = (p.tile / p.voxel).round() as usize;
    let tiles_n = vox.tiles;
    let origin = vox.origin;
    let grid = Grid {
        origin,
        voxel: p.voxel,
        dims: vox.dims,
    };
    let domain = Aabb {
        min: origin,
        max: [
            grid.coord(0, grid.dims[0]),
            grid.coord(1, grid.dims[1]),
            grid.coord(2, grid.dims[2]),
        ],
    };
    let total = grid.dims[0] * grid.dims[1] * grid.dims[2];
    let mut report = Report {
        voxels: [
            total,
            vox.surface.iter().filter(|s| **s).count(),
            vox.solid.iter().filter(|s| **s).count(),
        ],
        ..Report::default()
    };
    let solid = vox.solid;
    drop(vox.surface);
    bake_cells(
        input,
        &grid,
        &domain,
        tiles_n,
        per_tile,
        &solid,
        &mut report,
    )
    .map(|tome| (tome, report))
}

/// Voxelises the occluders over the view volume's tile grid and decides solid space.
pub(super) fn voxelize(input: &Input<'_>) -> Result<Voxels, Error> {
    let p = &input.params;
    let per_tile = (p.tile / p.voxel).round() as usize;
    if p.voxel <= 0.0 || per_tile == 0 || ((per_tile as f32) * p.voxel - p.tile).abs() > 1e-3 {
        return Err(Error::Params("tile must be a positive multiple of voxel"));
    }
    let vv = input.view_volume;
    let mut tiles_n = [0usize; 3];
    let mut origin = [0.0f32; 3];
    for a in 0..3 {
        if vv.max[a] <= vv.min[a] {
            return Err(Error::Params("empty view volume"));
        }
        origin[a] = (vv.min[a] / p.tile).floor() * p.tile;
        tiles_n[a] = (((vv.max[a] - origin[a]) / p.tile).ceil() as usize).max(1);
    }
    let grid = Grid {
        origin,
        voxel: p.voxel,
        dims: [
            tiles_n[0] * per_tile,
            tiles_n[1] * per_tile,
            tiles_n[2] * per_tile,
        ],
    };
    let domain = Aabb {
        min: origin,
        max: [
            grid.coord(0, grid.dims[0]),
            grid.coord(1, grid.dims[1]),
            grid.coord(2, grid.dims[2]),
        ],
    };
    let total = grid.dims[0] * grid.dims[1] * grid.dims[2];

    // 1. Surface voxels (a triangle crosses the interior) and blocked faces (a triangle covers
    // the face between a voxel and its +axis neighbour; used by the flood).
    let mut surface = vec![false; total];
    let mut blocked: [Vec<bool>; 3] = [vec![false; total], vec![false; total], vec![false; total]];
    let mesh = input.occluders.mesh();
    for t in &mesh.triangles {
        let c = [
            mesh.vertices[t[0] as usize],
            mesh.vertices[t[1] as usize],
            mesh.vertices[t[2] as usize],
        ];
        let mut tb = Bounds::EMPTY;
        for v in &c {
            tb.grow(*v);
        }
        let (x0, x1) = grid.range(0, tb.min[0], tb.max[0]);
        let (y0, y1) = grid.range(1, tb.min[1], tb.max[1]);
        let (z0, z1) = grid.range(2, tb.min[2], tb.max[2]);
        let eps = p.voxel * 1e-3;
        // Faces: the +axis face of every voxel from one before the range to its end.
        for z in z0.saturating_sub(1)..z1 {
            for y in y0.saturating_sub(1)..y1 {
                for x in x0.saturating_sub(1)..x1 {
                    let v = [x, y, z];
                    let lo = [grid.coord(0, x), grid.coord(1, y), grid.coord(2, z)];
                    for (axis, faces) in blocked.iter_mut().enumerate() {
                        if v[axis] + 1 >= grid.dims[axis] || faces[grid.index(v)] {
                            continue;
                        }
                        let mut fb = Bounds {
                            min: [lo[0] + eps, lo[1] + eps, lo[2] + eps],
                            max: [
                                lo[0] + p.voxel - eps,
                                lo[1] + p.voxel - eps,
                                lo[2] + p.voxel - eps,
                            ],
                        };
                        let plane = lo[axis] + p.voxel;
                        fb.min[axis] = plane - eps;
                        fb.max[axis] = plane + eps;
                        if triangle_box(&c, &fb) {
                            faces[grid.index(v)] = true;
                        }
                    }
                }
            }
        }
        for z in z0..z1 {
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = grid.index([x, y, z]);
                    if surface[i] {
                        continue;
                    }
                    let lo = [grid.coord(0, x), grid.coord(1, y), grid.coord(2, z)];
                    // The voxel's interior, shrunk by a hair: a triangle lying on a voxel face does not make
                    // the voxel partly open (it is wholly on one side of it).
                    let vb = Bounds {
                        min: [lo[0] + eps, lo[1] + eps, lo[2] + eps],
                        max: [
                            lo[0] + p.voxel - eps,
                            lo[1] + p.voxel - eps,
                            lo[2] + p.voxel - eps,
                        ],
                    };
                    if triangle_box(&c, &vb) {
                        surface[i] = true;
                    }
                }
            }
        }
    }
    // 2. Solid space.
    let solid: Vec<bool> = match p.solid {
        SolidMode::Parity => {
            // 2. Inside votes: crossing parity along every grid row, per axis.
            let bvh = Bvh::build(input.occluders.mesh());
            // Parity rays must cross the whole mesh, not just the domain: geometry reaching past the
            // domain still closes (or fails to close) the volumes inside it.
            let mut reach = Bounds {
                min: domain.min,
                max: domain.max,
            };
            for v in &input.occluders.mesh().vertices {
                reach.grow(*v);
            }
            let mut votes = vec![0u8; total];
            let mut hits = Vec::new();
            for axis in 0..3 {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                for j in 0..grid.dims[v] {
                    for i in 0..grid.dims[u] {
                        let mut a = [0.0f32; 3];
                        a[u] = grid.coord(u, i) + 0.5 * p.voxel;
                        a[v] = grid.coord(v, j) + 0.5 * p.voxel;
                        let mut b = a;
                        a[axis] = reach.min[axis] - 1.0;
                        b[axis] = reach.max[axis] + 1.0;
                        bvh.crossings(a, b, &mut hits);
                        if hits.is_empty() {
                            continue;
                        }
                        hits.sort_by(f32::total_cmp);
                        let span = b[axis] - a[axis];
                        let mut next = 0usize;
                        for k in 0..grid.dims[axis] {
                            let mut vox = [0usize; 3];
                            vox[axis] = k;
                            vox[u] = i;
                            vox[v] = j;
                            let s = (grid.centre(vox)[axis] - a[axis]) / span;
                            while next < hits.len() && hits[next] < s {
                                next += 1;
                            }
                            // Inside only if the crossings on BOTH sides are odd: a closed surface agrees, an
                            // open sheet (a floor without a bottom) does not, and then nothing is solid.
                            if next % 2 == 1 && (hits.len() - next) % 2 == 1 {
                                votes[grid.index(vox)] += 1;
                            }
                        }
                    }
                }
            }
            (0..total).map(|i| !surface[i] && votes[i] == 3).collect()
        }
        SolidMode::Flood => {
            // Open space = reachable from the seeds (without seeds: from the domain boundary)
            // through faces no triangle covers. Surface voxels are reached but never propagate,
            // so the flood cannot slip through a diagonal wall inside one voxel.
            let mut reached = vec![false; total];
            let mut stack: Vec<[usize; 3]> = Vec::new();
            let enter = |v: [usize; 3], reached: &mut Vec<bool>, stack: &mut Vec<[usize; 3]>| {
                let i = grid.index(v);
                if !reached[i] {
                    reached[i] = true;
                    if !surface[i] {
                        stack.push(v);
                    }
                }
            };
            if input.seeds.is_empty() {
                for z in 0..grid.dims[2] {
                    for y in 0..grid.dims[1] {
                        for x in 0..grid.dims[0] {
                            let edge = x == 0
                                || y == 0
                                || z == 0
                                || x + 1 == grid.dims[0]
                                || y + 1 == grid.dims[1]
                                || z + 1 == grid.dims[2];
                            if edge {
                                enter([x, y, z], &mut reached, &mut stack);
                            }
                        }
                    }
                }
            }
            for s in input.seeds {
                let k: Vec<f32> = (0..3)
                    .map(|a| ((s[a] - origin[a]) / p.voxel).floor())
                    .collect();
                if (0..3).all(|a| k[a] >= 0.0 && (k[a] as usize) < grid.dims[a]) {
                    enter(
                        [k[0] as usize, k[1] as usize, k[2] as usize],
                        &mut reached,
                        &mut stack,
                    );
                }
            }
            while let Some(v) = stack.pop() {
                for a in 0..3 {
                    if v[a] + 1 < grid.dims[a] && !blocked[a][grid.index(v)] {
                        let mut n = v;
                        n[a] += 1;
                        enter(n, &mut reached, &mut stack);
                    }
                    if v[a] > 0 {
                        let mut n = v;
                        n[a] -= 1;
                        if !blocked[a][grid.index(n)] {
                            enter(n, &mut reached, &mut stack);
                        }
                    }
                }
            }
            if p.surface_occludes {
                // Surface voxels occlude unless the camera is known to be in one.
                let mut seeded = vec![false; total];
                for s in input.seeds {
                    let k: Vec<f32> = (0..3)
                        .map(|a| ((s[a] - origin[a]) / p.voxel).floor())
                        .collect();
                    if (0..3).all(|a| k[a] >= 0.0 && (k[a] as usize) < grid.dims[a]) {
                        seeded[grid.index([k[0] as usize, k[1] as usize, k[2] as usize])] = true;
                    }
                }
                (0..total)
                    .map(|i| !reached[i] || (surface[i] && !seeded[i]))
                    .collect()
            } else {
                (0..total).map(|i| !surface[i] && !reached[i]).collect()
            }
        }
    };
    let _ = domain;
    Ok(Voxels {
        origin,
        voxel: p.voxel,
        dims: grid.dims,
        tiles: tiles_n,
        surface,
        solid,
    })
}

fn bake_cells(
    input: &Input<'_>,
    grid: &Grid,
    domain: &Aabb,
    tiles_n: [usize; 3],
    per_tile: usize,
    solid: &[bool],
    report: &mut Report,
) -> Result<Tome, Error> {
    let p = &input.params;
    let mode = Mode::of(p);
    let origin = grid.origin;
    let total = grid.dims[0] * grid.dims[1] * grid.dims[2];
    let domain = *domain;

    // 3. Top tree over the tile grid; leaves in level order are the tiles.
    let mut tile_ranges: Vec<[[usize; 2]; 3]> = Vec::new();
    let (top, _) = build_kd(
        [[0usize, tiles_n[0]], [0, tiles_n[1]], [0, tiles_n[2]]],
        |r| {
            let ext = [r[0][1] - r[0][0], r[1][1] - r[1][0], r[2][1] - r[2][0]];
            let axis = (0..3).max_by_key(|&a| (ext[a], 3 - a)).unwrap_or(0);
            if ext[axis] <= 1 {
                return None;
            }
            let at = r[axis][0] + ext[axis] / 2;
            let (mut lo, mut hi) = (*r, *r);
            lo[axis][1] = at;
            hi[axis][0] = at;
            Some((axis, at, lo, hi))
        },
        |r| {
            tile_ranges.push(*r);
            (tile_ranges.len() - 1) as u32
        },
        |axis, at| origin[axis] + at as f32 * p.tile,
    );
    // Node index of each tile (leaf ordinal order = level order).
    let tile_node: Vec<usize> = top
        .codes
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == 3)
        .map(|(n, _)| n)
        .collect();

    // 4. Cells: per tile, either greedy boxes of empty voxels or connected components of them.
    let mut cell_of = vec![SOLID; total];
    let mut cells: Vec<CellBox> = Vec::new();
    let mut tile_cells: Vec<Vec<usize>> = vec![Vec::new(); tile_ranges.len()];
    for (t, r) in tile_ranges.iter().enumerate() {
        let lo = [r[0][0] * per_tile, r[1][0] * per_tile, r[2][0] * per_tile];
        let hi = [lo[0] + per_tile, lo[1] + per_tile, lo[2] + per_tile];
        let free = |v: [usize; 3], cell_of: &[u32]| {
            !solid[grid.index(v)] && cell_of[grid.index(v)] == SOLID
        };
        for z in lo[2]..hi[2] {
            for y in lo[1]..hi[1] {
                for x in lo[0]..hi[0] {
                    if !free([x, y, z], &cell_of) {
                        continue;
                    }
                    let id = cells.len() as u32;
                    let local = tile_cells[t].len() as u32;
                    let (blo, bhi) = match mode {
                        Mode::Boxes | Mode::Merged { block: 0, .. } => {
                            let mut x1 = x + 1;
                            while x1 < hi[0] && free([x1, y, z], &cell_of) {
                                x1 += 1;
                            }
                            let mut y1 = y + 1;
                            'grow_y: while y1 < hi[1] {
                                for xx in x..x1 {
                                    if !free([xx, y1, z], &cell_of) {
                                        break 'grow_y;
                                    }
                                }
                                y1 += 1;
                            }
                            let mut z1 = z + 1;
                            'grow_z: while z1 < hi[2] {
                                for yy in y..y1 {
                                    for xx in x..x1 {
                                        if !free([xx, yy, z1], &cell_of) {
                                            break 'grow_z;
                                        }
                                    }
                                }
                                z1 += 1;
                            }
                            for zz in z..z1 {
                                for yy in y..y1 {
                                    for xx in x..x1 {
                                        cell_of[grid.index([xx, yy, zz])] = id;
                                    }
                                }
                            }
                            ([x, y, z], [x1, y1, z1])
                        }
                        Mode::Components { block } | Mode::Merged { block, .. } => {
                            // Flood fill (6-connected) inside the block of `block` voxels (the
                            // whole tile when 0) that holds the seed voxel.
                            let (lo, hi) = {
                                let mut blo = lo;
                                let mut bhi = hi;
                                for (a, k) in [x, y, z].into_iter().enumerate() {
                                    if let Some(n) = (k - lo[a]).checked_div(block) {
                                        blo[a] = lo[a] + n * block;
                                        bhi[a] = (blo[a] + block).min(hi[a]);
                                    }
                                }
                                (blo, bhi)
                            };
                            let mut blo = [x, y, z];
                            let mut bhi = [x + 1, y + 1, z + 1];
                            let mut stack = vec![[x, y, z]];
                            cell_of[grid.index([x, y, z])] = id;
                            while let Some(v) = stack.pop() {
                                for a in 0..3 {
                                    blo[a] = blo[a].min(v[a]);
                                    bhi[a] = bhi[a].max(v[a] + 1);
                                }
                                for a in 0..3 {
                                    for up in [false, true] {
                                        let mut n = v;
                                        if up {
                                            if v[a] + 1 >= hi[a] {
                                                continue;
                                            }
                                            n[a] += 1;
                                        } else {
                                            if v[a] <= lo[a] {
                                                continue;
                                            }
                                            n[a] -= 1;
                                        }
                                        if free(n, &cell_of) {
                                            cell_of[grid.index(n)] = id;
                                            stack.push(n);
                                        }
                                    }
                                }
                            }
                            (blo, bhi)
                        }
                    };
                    cells.push(CellBox {
                        lo: blo,
                        hi: bhi,
                        tile: t,
                        local,
                    });
                    tile_cells[t].push(id as usize);
                }
            }
        }
    }
    if let Mode::Merged { fill, .. } = mode {
        // Union neighbouring boxes of one tile that are mostly open to each other.
        let mut parent: Vec<u32> = (0..cells.len() as u32).collect();
        fn root(parent: &mut [u32], mut c: u32) -> u32 {
            while parent[c as usize] != c {
                parent[c as usize] = parent[parent[c as usize] as usize];
                c = parent[c as usize];
            }
            c
        }
        let mut shared: std::collections::HashMap<(u32, u32, u8), u32> =
            std::collections::HashMap::new();
        for z in 0..grid.dims[2] {
            for y in 0..grid.dims[1] {
                for x in 0..grid.dims[0] {
                    let v = [x, y, z];
                    let c = cell_of[grid.index(v)];
                    if c == SOLID {
                        continue;
                    }
                    for axis in 0..3 {
                        if v[axis] + 1 >= grid.dims[axis] {
                            continue;
                        }
                        let mut n = v;
                        n[axis] += 1;
                        let d = cell_of[grid.index(n)];
                        if d == SOLID || d == c || cells[d as usize].tile != cells[c as usize].tile
                        {
                            continue;
                        }
                        *shared.entry((c.min(d), c.max(d), axis as u8)).or_insert(0) += 1;
                    }
                }
            }
        }
        // Best-first agglomeration per tile: always merge the neighbouring pair whose union is
        // the most box-like (highest share of its bounding box that is open voxels of the two
        // cells), larger opening first on ties. A tile stops merging when its best union falls
        // below `fill` and it holds at most `max_cells_per_tile` cells.
        // Open voxels per seed cell (components do not fill their boxes).
        let mut voxels = vec![0usize; cells.len()];
        for &c in &cell_of {
            if c != SOLID {
                voxels[c as usize] += 1;
            }
        }
        let mut bounds: Vec<([usize; 3], [usize; 3])> =
            cells.iter().map(|c| (c.lo, c.hi)).collect();
        let mut per_tile = vec![0usize; tile_ranges.len()];
        for c in &cells {
            per_tile[c.tile] += 1;
        }
        // Opening (shared voxel faces) per neighbouring root pair.
        let mut adjacent: Vec<std::collections::HashMap<u32, u32>> =
            vec![std::collections::HashMap::new(); cells.len()];
        for ((a, b, _), n) in shared {
            *adjacent[a as usize].entry(b).or_insert(0) += n;
            *adjacent[b as usize].entry(a).or_insert(0) += n;
        }
        let union_fill =
            |bounds: &[([usize; 3], [usize; 3])], voxels: &[usize], a: usize, b: usize| {
                let mut lo = bounds[a].0;
                let mut hi = bounds[a].1;
                for k in 0..3 {
                    lo[k] = lo[k].min(bounds[b].0[k]);
                    hi[k] = hi[k].max(bounds[b].1[k]);
                }
                let volume = (hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2]);
                ((voxels[a] + voxels[b]) as f64 / volume as f64, lo, hi)
            };
        // Heap entries: (fill in millionths, opening, a, b, version of a, version of b).
        let mut version = vec![0u32; cells.len()];
        let mut heap = std::collections::BinaryHeap::new();
        for (a, adj) in adjacent.iter().enumerate() {
            for (&b, &n) in adj {
                if (a as u32) < b {
                    let (f, _, _) = union_fill(&bounds, &voxels, a, b as usize);
                    heap.push(((f * 1e6) as u64, n, a as u32, b, 0u32, 0u32));
                }
            }
        }
        while let Some((f, _, a, b, va, vb)) = heap.pop() {
            let (a, b) = (a as usize, b as usize);
            if version[a] != va
                || version[b] != vb
                || parent[a] != a as u32
                || parent[b] != b as u32
            {
                continue;
            }
            let tile = cells[a].tile;
            if (f as f64) < f64::from(fill) * 1e6 && per_tile[tile] <= p.max_cells_per_tile {
                continue;
            }
            let (_, lo, hi) = union_fill(&bounds, &voxels, a, b);
            let (keep, gone) = (a.min(b), a.max(b));
            parent[gone] = keep as u32;
            voxels[keep] = voxels[a] + voxels[b];
            bounds[keep] = (lo, hi);
            per_tile[tile] -= 1;
            version[keep] += 1;
            let moved = std::mem::take(&mut adjacent[gone]);
            for (n, k) in moved {
                let n = n as usize;
                adjacent[n].remove(&(gone as u32));
                if n != keep {
                    *adjacent[keep].entry(n as u32).or_insert(0) += k;
                    *adjacent[n].entry(keep as u32).or_insert(0) += k;
                }
            }
            adjacent[keep].remove(&(keep as u32));
            for (&n, &k) in &adjacent[keep] {
                let n = n as usize;
                let (f, _, _) = union_fill(&bounds, &voxels, keep, n);
                let (x, y) = (keep.min(n), keep.max(n));
                heap.push((
                    (f * 1e6) as u64,
                    k,
                    x as u32,
                    y as u32,
                    version[x],
                    version[y],
                ));
            }
        }
        // Relabel: one cell per root, bounding box of its boxes, local indices per tile.
        let mut new_id = vec![u32::MAX; cells.len()];
        let mut merged: Vec<CellBox> = Vec::new();
        let mut merged_tiles: Vec<Vec<usize>> = vec![Vec::new(); tile_ranges.len()];
        for c in 0..cells.len() {
            let r = root(&mut parent, c as u32) as usize;
            if new_id[r] == u32::MAX {
                new_id[r] = merged.len() as u32;
                let t = cells[r].tile;
                merged.push(CellBox {
                    lo: cells[r].lo,
                    hi: cells[r].hi,
                    tile: t,
                    local: merged_tiles[t].len() as u32,
                });
                merged_tiles[t].push(merged.len() - 1);
            }
            let m = &mut merged[new_id[r] as usize];
            for a in 0..3 {
                m.lo[a] = m.lo[a].min(cells[c].lo[a]);
                m.hi[a] = m.hi[a].max(cells[c].hi[a]);
            }
            new_id[c] = new_id[r];
        }
        for id in cell_of.iter_mut() {
            if *id != SOLID {
                *id = new_id[*id as usize];
            }
        }
        cells = merged;
        tile_cells = merged_tiles;
    }
    report.tiles = tile_ranges.len();
    report.cells = cells.len();
    // Open-voxel samples per cell: every stride-th open voxel in grid order.
    let mut open = vec![0usize; cells.len()];
    for &c in &cell_of {
        if c != SOLID {
            open[c as usize] += 1;
        }
    }
    let mut seen = vec![0usize; cells.len()];
    report.cell_samples = vec![Vec::new(); cells.len()];
    for z in 0..grid.dims[2] {
        for y in 0..grid.dims[1] {
            for x in 0..grid.dims[0] {
                let c = cell_of[grid.index([x, y, z])];
                if c == SOLID {
                    continue;
                }
                let c = c as usize;
                let stride = open[c].div_ceil(CELL_SAMPLES).max(1);
                if seen[c].is_multiple_of(stride) {
                    report.cell_samples[c].push(grid.centre([x, y, z]));
                }
                seen[c] += 1;
            }
        }
    }

    let world_box = |c: &CellBox| Aabb {
        min: [
            grid.coord(0, c.lo[0]),
            grid.coord(1, c.lo[1]),
            grid.coord(2, c.lo[2]),
        ],
        max: [
            grid.coord(0, c.hi[0]),
            grid.coord(1, c.hi[1]),
            grid.coord(2, c.hi[2]),
        ],
    };
    let tile_box = |t: usize| {
        let r = tile_ranges[t];
        Aabb {
            min: [
                origin[0] + r[0][0] as f32 * p.tile,
                origin[1] + r[1][0] as f32 * p.tile,
                origin[2] + r[2][0] as f32 * p.tile,
            ],
            max: [
                origin[0] + r[0][1] as f32 * p.tile,
                origin[1] + r[1][1] as f32 * p.tile,
                origin[2] + r[2][1] as f32 * p.tile,
            ],
        }
    };

    // 5. Portals from voxel adjacency: one per (cell, axis, side, neighbour, plane), its rectangle
    // the bounds of the shared voxel faces (covering, never less than, the true opening).
    struct Link {
        axis: usize,
        side: u32,
        neighbour: Option<usize>,
        plane: f32,
        rect: [f32; 4],
    }
    type FaceKey = (u32, u8, u8, u32, u32);
    let mut faces: std::collections::BTreeMap<FaceKey, [usize; 4]> =
        std::collections::BTreeMap::new();
    for z in 0..grid.dims[2] {
        for y in 0..grid.dims[1] {
            for x in 0..grid.dims[0] {
                let v = [x, y, z];
                let c = cell_of[grid.index(v)];
                if c == SOLID {
                    continue;
                }
                for axis in 0..3 {
                    let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
                    for side in 0..2u8 {
                        let (neighbour, k) = if side == 1 {
                            if v[axis] + 1 >= grid.dims[axis] {
                                (u32::MAX, v[axis] + 1)
                            } else {
                                let mut n = v;
                                n[axis] += 1;
                                (cell_of[grid.index(n)], v[axis] + 1)
                            }
                        } else if v[axis] == 0 {
                            (u32::MAX, 0)
                        } else {
                            let mut n = v;
                            n[axis] -= 1;
                            (cell_of[grid.index(n)], v[axis])
                        };
                        // Same cell, or solid inside the grid: no portal.
                        if neighbour == c || (neighbour == SOLID && k > 0 && k < grid.dims[axis]) {
                            continue;
                        }
                        let rect = faces
                            .entry((c, axis as u8, side, neighbour, k as u32))
                            .or_insert([v[a1], v[a1] + 1, v[a2], v[a2] + 1]);
                        rect[0] = rect[0].min(v[a1]);
                        rect[1] = rect[1].max(v[a1] + 1);
                        rect[2] = rect[2].min(v[a2]);
                        rect[3] = rect[3].max(v[a2] + 1);
                    }
                }
            }
        }
    }
    let mut links: Vec<Vec<Link>> = (0..cells.len()).map(|_| Vec::new()).collect();
    for ((c, axis, side, neighbour, k), r) in faces {
        let axis = axis as usize;
        let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
        links[c as usize].push(Link {
            axis,
            side: u32::from(side),
            neighbour: (neighbour != u32::MAX).then_some(neighbour as usize),
            plane: grid.coord(axis, k as usize),
            rect: [
                grid.coord(a1, r[0]),
                grid.coord(a1, r[1]),
                grid.coord(a2, r[2]),
                grid.coord(a2, r[3]),
            ],
        });
    }

    // 6. Objects per cell.
    // Cells with a boundary portal, per domain face (axis * 2 + side).
    let mut boundary_cells: [Vec<u32>; 6] = Default::default();
    for (c, list) in links.iter().enumerate() {
        for l in list {
            if l.neighbour.is_none() {
                let face = &mut boundary_cells[l.axis * 2 + l.side as usize];
                if face.last() != Some(&(c as u32)) {
                    face.push(c as u32);
                }
            }
        }
    }
    let mut cell_objects: Vec<Vec<u32>> = vec![Vec::new(); cells.len()];
    for (o, t) in input.targets.iter().enumerate() {
        let mut any = false;
        // The part inside the domain: every cell within one voxel of its box. The object's
        // surface lies on or inside its box, often in solid (surface) voxels; a sight line to it
        // arrives through an open neighbour of such a voxel, which can lie outside the box.
        let e = p.voxel;
        let overlaps_domain =
            (0..3).all(|k| t.bounds.max[k] >= domain.min[k] && t.bounds.min[k] <= domain.max[k]);
        if overlaps_domain {
            let (x0, x1) = grid.range(0, t.bounds.min[0] - e, t.bounds.max[0] + e);
            let (y0, y1) = grid.range(1, t.bounds.min[1] - e, t.bounds.max[1] + e);
            let (z0, z1) = grid.range(2, t.bounds.min[2] - e, t.bounds.max[2] + e);
            for z in z0..z1 {
                for y in y0..y1 {
                    for x in x0..x1 {
                        let n = cell_of[grid.index([x, y, z])];
                        if n != SOLID {
                            let list = &mut cell_objects[n as usize];
                            if list.last() != Some(&(o as u32)) {
                                list.push(o as u32);
                            }
                            any = true;
                        }
                    }
                }
            }
            // And every cell whose box overlaps that grown box (`tile::objects`).
            let (r0, r1) = ([x0, y0, z0], [x1, y1, z1]);
            for (c, cb) in cells.iter().enumerate() {
                if (0..3).all(|k| cb.lo[k] < r1[k] && r0[k] < cb.hi[k]) {
                    let list = &mut cell_objects[c];
                    if list.last() != Some(&(o as u32)) {
                        list.push(o as u32);
                    }
                    any = true;
                }
            }
        }
        // The part beyond a domain face: any sight line to it leaves the domain through that
        // face, so it is listed in every cell with a boundary portal there.
        for axis in 0..3 {
            for (side, beyond) in [
                (0usize, t.bounds.min[axis] < domain.min[axis]),
                (1, t.bounds.max[axis] > domain.max[axis]),
            ] {
                if !beyond {
                    continue;
                }
                for &c in &boundary_cells[axis * 2 + side] {
                    let list = &mut cell_objects[c as usize];
                    if list.last() != Some(&(o as u32)) {
                        list.push(o as u32);
                    }
                    any = true;
                }
            }
        }
        if !any {
            report.buried_targets += 1;
        }
    }

    // 7. Object run list.
    let max_run = 31u32;
    let index_bits = bits_for(input.targets.len().saturating_sub(1) as u32).max(1);
    let mut object_runs = RunList {
        index_bits,
        run_bits: bits_for(max_run),
        entries: Vec::new(),
    };
    let mut cell_list: Vec<(u32, u32)> = Vec::with_capacity(cells.len());
    for list in &mut cell_objects {
        list.sort_unstable();
        list.dedup();
        let start = object_runs.entries.len() as u32;
        let mut k = 0;
        while k < list.len() {
            let mut run = 1;
            while k + run < list.len()
                && list[k + run] == list[k] + run as u32
                && (run as u32) < max_run
            {
                run += 1;
            }
            object_runs.entries.push(Run {
                index: list[k],
                run: run as u32,
            });
            k += run;
        }
        cell_list.push((start, list.len() as u32));
    }

    // What the view trees resolve. Solid voxels touching open space are owned by a neighbouring
    // cell of their tile: the one directly above (a floor's surface belongs to the space a
    // character stands in), else the most frequent horizontal neighbour, else the one below.
    // Points within a voxel of a surface (characters' positions) then find the cell on their own
    // side instead of whatever cell the KD build merged the solid voxel into (parity capture 2:
    // a dark character beside a wall resolved to the cell below the ground). Deeper solid
    // voxels stay "don't care".
    let mut tile_at = vec![usize::MAX; tiles_n[0] * tiles_n[1] * tiles_n[2]];
    for (t, r) in tile_ranges.iter().enumerate() {
        tile_at[(r[2][0] * tiles_n[1] + r[1][0]) * tiles_n[0] + r[0][0]] = t;
    }
    let tile_of = |v: [usize; 3]| {
        tile_at[((v[2] / per_tile) * tiles_n[1] + v[1] / per_tile) * tiles_n[0] + v[0] / per_tile]
    };
    let mut view_of = cell_of.clone();
    for z in 0..grid.dims[2] {
        for y in 0..grid.dims[1] {
            for x in 0..grid.dims[0] {
                let v = [x, y, z];
                if cell_of[grid.index(v)] != SOLID {
                    continue;
                }
                let t = tile_of(v);
                let open = |n: [usize; 3]| {
                    let c = cell_of[grid.index(n)];
                    (c != SOLID && cells[c as usize].tile == t).then_some(c)
                };
                let above = (z + 1 < grid.dims[2])
                    .then(|| open([x, y, z + 1]))
                    .flatten();
                let owner = above
                    .or_else(|| {
                        let mut side: Vec<u32> = Vec::new();
                        for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                            if nx < 0
                                || ny < 0
                                || nx as usize >= grid.dims[0]
                                || ny as usize >= grid.dims[1]
                            {
                                continue;
                            }
                            if let Some(c) = open([nx as usize, ny as usize, z]) {
                                side.push(c);
                            }
                        }
                        side.sort_unstable();
                        side.chunk_by(|a, b| a == b)
                            .max_by_key(|run| (run.len(), core::cmp::Reverse(run[0])))
                            .map(|run| run[0])
                    })
                    .or_else(|| (z > 0).then(|| open([x, y, z - 1])).flatten());
                if let Some(c) = owner {
                    view_of[grid.index(v)] = c;
                }
            }
        }
    }

    // 8. Tiles and global cells.
    let mut tiles: Vec<Option<Tile>> = vec![None; top.codes.len()];
    let mut global_cells = Vec::with_capacity(cells.len());
    let mut global_portals = Vec::new();
    for (t, r) in tile_ranges.iter().enumerate() {
        let frame = tile_box(t);
        let mut tile_portals = Vec::new();
        let mut records = Vec::new();
        for &cid in &tile_cells[t] {
            let c = &cells[cid];
            let start = tile_portals.len() as u32;
            // Portals within the tile first, as `global.rs` orders them.
            let mut ordered: Vec<_> = links[cid].iter().collect();
            ordered.sort_by_key(|l| !matches!(l.neighbour, Some(n) if cells[n].tile == t));
            for l in ordered {
                let (target, local, boundary) = match l.neighbour {
                    Some(n) => (tile_node[cells[n].tile] as u32, cells[n].local, false),
                    None => (0x3FF_FFFF, 0, true),
                };
                tile_portals.push(portal(
                    &PortalSpec {
                        axis: l.axis,
                        side: l.side,
                        boundary,
                        target,
                        local,
                        plane: l.plane,
                        rect: l.rect,
                    },
                    &frame,
                ));
            }
            let count = tile_portals.len() as u32 - start;
            let wb = world_box(c);
            let packed = pack_box(&wb, &frame);
            let (ostart, ocount) = cell_list[cid];
            records.push(TileCell([
                start, count, ostart, ocount, cid as u32, 0, packed[0], packed[1], packed[2],
            ]));
            // Global cell and its portals (no boundary portals in the global graph).
            let gstart = global_portals.len() as u32;
            for l in &links[cid] {
                if let Some(n) = l.neighbour {
                    global_portals.push(portal(
                        &PortalSpec {
                            axis: l.axis,
                            side: l.side,
                            boundary: false,
                            target: n as u32,
                            local: 0xFFFF,
                            plane: l.plane,
                            rect: l.rect,
                        },
                        &domain,
                    ));
                }
            }
            global_cells.push(GlobalCell {
                portal_start: gstart,
                portal_count: global_portals.len() as u32 - gstart,
                bounds: pack_box(&wb, &domain),
            });
        }
        report.portals += tile_portals.len();
        // View tree over the tile's voxels.
        let lo = [r[0][0] * per_tile, r[1][0] * per_tile, r[2][0] * per_tile];
        let region = [
            [lo[0], lo[0] + per_tile],
            [lo[1], lo[1] + per_tile],
            [lo[2], lo[2] + per_tile],
        ];
        // A region is a leaf when its open and surface-owned voxels all belong to one cell.
        // Other solid voxels are "don't care": nothing is ever inside them, so they take the
        // cell around them; only a region without any owned voxel becomes a solid leaf.
        let uniform = |rg: &[[usize; 2]; 3]| -> Option<u32> {
            let mut first = SOLID;
            for z in rg[2][0]..rg[2][1] {
                for y in rg[1][0]..rg[1][1] {
                    for x in rg[0][0]..rg[0][1] {
                        let c = view_of[grid.index([x, y, z])];
                        if c == SOLID || c == first {
                            continue;
                        }
                        if first != SOLID {
                            return None;
                        }
                        first = c;
                    }
                }
            }
            Some(first)
        };
        let ncells = tile_cells[t].len() as u32;
        let leaf_bits = 1 + bits_for(ncells.saturating_sub(1));
        let solid_value = (1u32 << leaf_bits) - 1;
        let (tree, leaf_values) = build_kd(
            region,
            |rg| {
                if uniform(rg).is_some() {
                    return None;
                }
                let (axis, at) = choose_split(rg, |v| view_of[grid.index(v)]);
                let (mut b, mut a) = (*rg, *rg);
                b[axis][1] = at;
                a[axis][0] = at;
                Some((axis, at, b, a))
            },
            |rg| match uniform(rg) {
                Some(SOLID) | None => solid_value,
                Some(cid) => cells[cid as usize].local,
            },
            |axis, at| grid.coord(axis, at),
        );
        tiles[tile_node[t]] = Some(Tile {
            bounds: frame,
            flags: 1,
            level: super::tile_level(input.params.voxel, input.params.tile, &frame),
            cell_word_high: 1,
            cells: records,
            portals: tile_portals,
            view: Some(ViewTree {
                tree,
                leaf_bits,
                leaf_values,
            }),
            bsp: None,
        });
    }

    let node_distances: Vec<f32> = top
        .codes
        .iter()
        .map(|c| {
            if *c == 3 {
                p.tile / p.base_distance
            } else {
                -1.0
            }
        })
        .chain(std::iter::once(-1.0))
        .collect();
    let mut params = [0u8; PARAMS_SIZE];
    let text = format!("tenebrae 0.1 T {:.1} V {:.3}", p.tile, p.voxel);
    params[..text.len().min(PARAMS_SIZE - 1)]
        .copy_from_slice(&text.as_bytes()[..text.len().min(PARAMS_SIZE - 1)]);
    let tome = Tome {
        header: Header {
            flags: 0xD600,
            hash: 0,
            base_distance: p.base_distance,
            word_10: 0,
            bounds: domain,
            word_34: 0,
            list_widths_high: 0,
            words_a4: [0; 3],
            words_b0: [0; 2],
            params,
        },
        top,
        node_distances,
        objects: input.targets.iter().map(|t| t.bounds).collect(),
        users: super::users(input.targets.iter()),
        object_runs,
        cell_runs: RunList {
            index_bits: bits_for(cells.len().saturating_sub(1) as u32).max(1),
            run_bits: 1,
            entries: Vec::new(),
        },
        global_cells,
        global_portals,
        gates: Gates::default(),
        tiles,
    };
    Ok(tome)
}
