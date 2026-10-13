//! Per-tile passes of the bake. Every pass reads only shared, immutable context and writes only its
//! own tile's result, so tiles can run in any order on any thread.
//!
//! * Pass A ([`flood_pass`], flood mode only): voxelise, label the components of non-surface voxels
//!   over unblocked faces, mark start voxels, emit the six boundary layers.
//! * Pass B ([`solid_pass`] then [`cell_pass`]): voxelise again, decide solid space (from the joined
//!   flood or by crossing parity), then cells, merge, samples, faces, objects and the view tree.

use std::collections::HashMap;

use super::grid::{
    Bits, Brick, Bricks, Field, Grid, Layer, SOLID, UnionFind, bits_bytes, coalesce, face_layer,
    field_bytes, run_rects,
};
use super::{CELL_SAMPLES, Input};
use tenebrae_geom::{Bounds, Bvh, Vec3, triangle_box};
use tenebrae_tome::Aabb;

/// A non-surface voxel's label is below this; surface voxels carry [`SURF`].
pub(super) const SURF: u32 = u32::MAX;
/// Label bits of a pass-A layer value; bit 31 is the face's blocked bit (`+` faces only).
pub(super) const LABEL: u32 = 0x7FFF_FFFF;
/// No neighbour cell: a domain boundary face.
pub(super) const BOUNDARY: u32 = u32::MAX;

/// Everything a tile pass reads.
pub(super) struct Ctx<'a> {
    pub(super) input: &'a Input<'a>,
    pub(super) grid: Grid,
    pub(super) domain: Aabb,
    pub(super) layout: Bricks,
    pub(super) tiles_n: [usize; 3],
    /// Tile-grid cell of each tile (leaf of the top tree, level order).
    pub(super) tile_ranges: Vec<[[usize; 2]; 3]>,
    /// Tile index per tile-grid cell, x fastest.
    pub(super) tile_at: Vec<usize>,
    /// Triangles whose voxel ranges (grown by one voxel below) reach each tile.
    pub(super) buckets: Vec<Vec<u32>>,
    /// Seed voxels inside the domain.
    pub(super) seeds: Vec<[usize; 3]>,
    /// Seed cells: components inside blocks of this many voxels (0: the tile), or greedy boxes.
    pub(super) block: Option<usize>,
    /// Merge fill and per-tile cell cap, when merging (an infinite fill merges only over the cap).
    pub(super) merge: Option<(f32, usize)>,
    /// Per-tile Umbra cell merge threshold in metres (lever L2); 0 = off.
    pub(super) cell_merge: f32,
    /// Parity mode: the hierarchy and the extent rays must cross.
    pub(super) parity: Option<(Bvh, Bounds)>,
    /// The occluder triangles.
    pub(super) tris: super::Tris<'a>,
    /// Parity mode: the occluders as one world-space mesh (the cache hashes it).
    pub(super) soup: Option<std::borrow::Cow<'a, tenebrae_geom::Mesh>>,
}

impl Ctx<'_> {
    /// Global voxel of the tile's minimum corner.
    pub(super) fn lo(&self, t: usize) -> [usize; 3] {
        let pt = self.layout.tile();
        let r = &self.tile_ranges[t];
        [r[0][0] * pt, r[1][0] * pt, r[2][0] * pt]
    }
    /// Umbra's level shift of tile `t`: 0 inside the view volume, else [`level_shift`].
    pub(super) fn level_shift(&self, t: usize) -> u32 {
        if self.cell_merge <= 0.0 {
            return 0;
        }
        let frame = tile_frame(&self.grid, self.input.params.tile, &self.tile_ranges[t]);
        level_shift(&self.input.view_volume, &frame, self.cell_merge)
    }
    /// Lever L2's threshold `R` for tile `t` in metres: `cell_merge`, doubled per level shift.
    pub(super) fn feature(&self, t: usize) -> f32 {
        self.cell_merge * (1u32 << self.level_shift(t)) as f32
    }
    /// The seed block of tile `t` in voxels: [`Ctx::block`], or the block Umbra's cell level
    /// gives the raised feature of a tile outside the view volume, whichever is larger.
    pub(super) fn block_of(&self, t: usize) -> Option<usize> {
        let block = self.block?;
        let k = self.level_shift(t);
        if k == 0 || block == 0 {
            return Some(block);
        }
        let p = &self.input.params;
        let raised = cell_level_block(p.tile, self.feature(t), self.grid.voxel);
        Some(raised.clamp(block, self.layout.tile()))
    }
    /// The tile across face `(axis, side)`, if inside the domain.
    pub(super) fn neighbour(&self, t: usize, axis: usize, side: usize) -> Option<usize> {
        let mut c = [
            self.tile_ranges[t][0][0],
            self.tile_ranges[t][1][0],
            self.tile_ranges[t][2][0],
        ];
        if side == 1 {
            c[axis] += 1;
            if c[axis] >= self.tiles_n[axis] {
                return None;
            }
        } else {
            c[axis] = c[axis].checked_sub(1)?;
        }
        Some(self.tile_at[(c[2] * self.tiles_n[1] + c[1]) * self.tiles_n[0] + c[0]])
    }
    /// The cost per rectangle of Umbra's portal coalescing ([`super::grid::coalesce`]), in square
    /// voxels: `R² / 2` for portals inside a tile and on the domain boundary, `(R / 4)²` across
    /// a tile seam, `R` being lever L2's threshold (Umbra's smallest occluder). `None` (one
    /// bounding rectangle per neighbour and plane) with L2 off.
    pub(super) fn portal_cost(&self, seam: bool) -> Option<f64> {
        (self.cell_merge > 0.0).then(|| {
            let r = f64::from(self.cell_merge) / f64::from(self.grid.voxel);
            if seam { (r / 4.0).powi(2) } else { r * r / 2.0 }
        })
    }
    /// Whether face `(axis, side)` of the tile is on the domain boundary.
    fn on_boundary(&self, t: usize, axis: usize, side: usize) -> bool {
        if side == 1 {
            self.tile_ranges[t][axis][1] == self.tiles_n[axis]
        } else {
            self.tile_ranges[t][axis][0] == 0
        }
    }
}

/// Voxel ranges `[lo, hi)` per axis.
pub(super) type Ranges = [(usize, usize); 3];

/// A triangle's voxel ranges: the surface range, and the face range (one voxel lower).
pub(super) fn tri_ranges(grid: &Grid, c: &[Vec3; 3]) -> (Ranges, Ranges) {
    let mut tb = Bounds::EMPTY;
    for v in c {
        tb.grow(*v);
    }
    let r = [
        grid.range(0, tb.min[0], tb.max[0]),
        grid.range(1, tb.min[1], tb.max[1]),
        grid.range(2, tb.min[2], tb.max[2]),
    ];
    let e = r.map(|(a, b)| (a.saturating_sub(1), b));
    (r, e)
}

/// `true` when the triangle may touch a voxel interior or a voxel's `+` face inside the voxel box
/// `[lo, hi)`: the box grown by one voxel, so the test never rejects what the voxel tests accept.
pub(super) fn may_touch(grid: &Grid, c: &[Vec3; 3], lo: [usize; 3], hi: [usize; 3]) -> bool {
    let v = grid.voxel;
    let b = Bounds {
        min: [
            grid.coord(0, lo[0]) - v,
            grid.coord(1, lo[1]) - v,
            grid.coord(2, lo[2]) - v,
        ],
        max: [
            grid.coord(0, hi[0]) + v,
            grid.coord(1, hi[1]) + v,
            grid.coord(2, hi[2]) + v,
        ],
    };
    triangle_box(c, &b)
}

/// Calls `f(brick, brick voxel lo)` for every brick of the tile at `lo` inside the clipped range
/// `cl` that the triangle may touch.
pub(super) fn touched_bricks(
    grid: &Grid,
    layout: Bricks,
    lo: [usize; 3],
    c: &[Vec3; 3],
    cl: [(usize, usize); 3],
    mut f: impl FnMut(usize, [usize; 3]),
) {
    let s = layout.shift;
    let edge = layout.edge();
    let b0 = [0, 1, 2].map(|a| (cl[a].0 - lo[a]) >> s);
    let b1 = [0, 1, 2].map(|a| (cl[a].1 - 1 - lo[a]) >> s);
    for bz in b0[2]..=b1[2] {
        for by in b0[1]..=b1[1] {
            for bx in b0[0]..=b1[0] {
                let vlo = [lo[0] + bx * edge, lo[1] + by * edge, lo[2] + bz * edge];
                let vhi = vlo.map(|k| k + edge);
                if may_touch(grid, c, vlo, vhi) {
                    f(layout.index([bx, by, bz]), vlo);
                }
            }
        }
    }
}

/// Clips a face range to the tile at `lo`; `None` when they do not meet.
pub(super) fn clip(
    e: [(usize, usize); 3],
    lo: [usize; 3],
    size: usize,
) -> Option<[(usize, usize); 3]> {
    let mut cl = [(0usize, 0usize); 3];
    for a in 0..3 {
        cl[a] = (e[a].0.max(lo[a]), e[a].1.min(lo[a] + size));
        if cl[a].0 >= cl[a].1 {
            return None;
        }
    }
    Some(cl)
}

/// A tile's voxelisation.
pub(super) struct Vox {
    /// An occluder triangle crosses the voxel's interior.
    pub(super) surface: Bits,
    /// Per axis: a triangle covers the voxel's `+axis` face.
    pub(super) blocked: [Bits; 3],
}

/// Voxelises the tile's triangles exactly as the whole-grid baker did, voxel by voxel, skipping
/// bricks and 4-voxel sub-blocks the triangle cannot touch.
pub(super) fn voxelize_tile(ctx: &Ctx<'_>, t: usize) -> Vox {
    let g = &ctx.grid;
    let layout = ctx.layout;
    let size = layout.tile();
    let lo = ctx.lo(t);
    let voxel = g.voxel;
    let eps = voxel * 1e-3;
    let edge = layout.edge();
    let sub = edge.min(4);
    let mut surface = Bits::new(layout, false);
    let mut blocked = [
        Bits::new(layout, false),
        Bits::new(layout, false),
        Bits::new(layout, false),
    ];
    for &ti in &ctx.buckets[t] {
        let c = ctx.tris.corners(ti);
        let (r, e) = tri_ranges(g, &c);
        let Some(cl) = clip(e, lo, size) else {
            continue;
        };
        touched_bricks(g, layout, lo, &c, cl, |_, vlo| {
            for sz in (vlo[2]..vlo[2] + edge).step_by(sub) {
                for sy in (vlo[1]..vlo[1] + edge).step_by(sub) {
                    for sx in (vlo[0]..vlo[0] + edge).step_by(sub) {
                        let slo = [sx, sy, sz];
                        let shi = slo.map(|k| k + sub);
                        let q = [0, 1, 2].map(|a| (slo[a].max(cl[a].0), shi[a].min(cl[a].1)));
                        if q.iter().any(|(a, b)| a >= b) {
                            continue;
                        }
                        if sub < edge && !may_touch(g, &c, slo, shi) {
                            continue;
                        }
                        for z in q[2].0..q[2].1 {
                            for y in q[1].0..q[1].1 {
                                for x in q[0].0..q[0].1 {
                                    let v = [x, y, z];
                                    let lv = [x - lo[0], y - lo[1], z - lo[2]];
                                    let wl = [g.coord(0, x), g.coord(1, y), g.coord(2, z)];
                                    for (axis, faces) in blocked.iter_mut().enumerate() {
                                        if v[axis] + 1 >= g.dims[axis] || faces.get(lv) {
                                            continue;
                                        }
                                        let mut fb = Bounds {
                                            min: [wl[0] + eps, wl[1] + eps, wl[2] + eps],
                                            max: [
                                                wl[0] + voxel - eps,
                                                wl[1] + voxel - eps,
                                                wl[2] + voxel - eps,
                                            ],
                                        };
                                        let plane = wl[axis] + voxel;
                                        fb.min[axis] = plane - eps;
                                        fb.max[axis] = plane + eps;
                                        if triangle_box(&c, &fb) {
                                            faces.set(lv, true);
                                        }
                                    }
                                    let inside = (0..3).all(|a| v[a] >= r[a].0 && v[a] < r[a].1);
                                    if !inside || surface.get(lv) {
                                        continue;
                                    }
                                    // The voxel's interior, shrunk by a hair: a triangle lying on
                                    // a voxel face does not make the voxel partly open.
                                    let vb = Bounds {
                                        min: [wl[0] + eps, wl[1] + eps, wl[2] + eps],
                                        max: [
                                            wl[0] + voxel - eps,
                                            wl[1] + voxel - eps,
                                            wl[2] + voxel - eps,
                                        ],
                                    };
                                    if triangle_box(&c, &vb) {
                                        surface.set(lv, true);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }
    surface.compact();
    for b in &mut blocked {
        b.compact();
    }
    Vox { surface, blocked }
}

/// Components of non-surface voxels over unblocked faces inside the tile. Labels are numbered in
/// order of each component's first brick-level node, so the same voxelisation always gives the
/// same labels. Returns the labels (surface voxels: [`SURF`]) and the label count.
pub(super) fn label(vox: &Vox) -> (Field<u32>, usize) {
    let layout = vox.surface.layout;
    let edge = layout.edge();
    let volume = layout.volume();
    let mut labels = Field::new(layout, SURF);
    let mut simple = vec![false; layout.count()];
    let mut nodes = 0u32;
    let stride = [1usize, edge, edge * edge];
    for (b, flag) in simple.iter_mut().enumerate() {
        let surface = vox.surface.uniform(b);
        if surface == Some(true) {
            continue;
        }
        if surface == Some(false) && vox.blocked.iter().all(|f| f.uniform(b) == Some(false)) {
            labels.bricks[b] = Brick::Uniform(nodes);
            nodes += 1;
            *flag = true;
            continue;
        }
        let mut arr = vec![SURF; volume];
        let mut stack = Vec::new();
        for l in 0..volume {
            if arr[l] != SURF || vox.surface.at(b, l) {
                continue;
            }
            arr[l] = nodes;
            stack.push(l);
            while let Some(u) = stack.pop() {
                let lu = layout.local(u);
                for a in 0..3 {
                    if lu[a] + 1 < edge && !vox.blocked[a].at(b, u) {
                        let w = u + stride[a];
                        if arr[w] == SURF && !vox.surface.at(b, w) {
                            arr[w] = nodes;
                            stack.push(w);
                        }
                    }
                    if lu[a] > 0 {
                        let w = u - stride[a];
                        if !vox.blocked[a].at(b, w) && arr[w] == SURF && !vox.surface.at(b, w) {
                            arr[w] = nodes;
                            stack.push(w);
                        }
                    }
                }
            }
            nodes += 1;
        }
        labels.bricks[b] = Brick::of(arr);
    }
    let mut uf = UnionFind::new(nodes as usize);
    for b in 0..layout.count() {
        let bc = layout.coords(b);
        for a in 0..3 {
            if bc[a] + 1 >= layout.n {
                continue;
            }
            let mut nc = bc;
            nc[a] += 1;
            let n = layout.index(nc);
            if simple[b] && simple[n] {
                if let (Some(x), Some(y)) = (labels.uniform(b), labels.uniform(n)) {
                    uf.union(x, y);
                }
                continue;
            }
            if labels.uniform(b) == Some(SURF) || labels.uniform(n) == Some(SURF) {
                continue;
            }
            let (a1, a2) = ((a + 1) % 3, (a + 2) % 3);
            for j2 in 0..edge {
                for j1 in 0..edge {
                    let mut top = [0usize; 3];
                    top[a] = edge - 1;
                    top[a1] = j1;
                    top[a2] = j2;
                    let mut bot = top;
                    bot[a] = 0;
                    let (lt, lb) = (layout.inner(top), layout.inner(bot));
                    let (x, y) = (labels.at(b, lt), labels.at(n, lb));
                    if x != SURF && y != SURF && !vox.blocked[a].at(b, lt) {
                        uf.union(x, y);
                    }
                }
            }
        }
    }
    let mut id = vec![u32::MAX; nodes as usize];
    let mut count = 0u32;
    for k in 0..nodes {
        let r = uf.find(k) as usize;
        if id[r] == u32::MAX {
            id[r] = count;
            count += 1;
        }
        id[k as usize] = id[r];
    }
    labels.map(|x| if x == SURF { SURF } else { id[x as usize] });
    (labels, count as usize)
}

/// Seed voxels inside the tile (tile-local linear indices, sorted), and whether the flood starts
/// from the domain boundary instead (no seeds given).
pub(super) fn starts(ctx: &Ctx<'_>, t: usize) -> (Vec<usize>, bool) {
    let size = ctx.layout.tile();
    let lo = ctx.lo(t);
    let mut seeds: Vec<usize> = ctx
        .seeds
        .iter()
        .filter(|s| (0..3).all(|a| s[a] >= lo[a] && s[a] < lo[a] + size))
        .map(|s| ((s[2] - lo[2]) * size + (s[1] - lo[1])) * size + (s[0] - lo[0]))
        .collect();
    seeds.sort_unstable();
    seeds.dedup();
    (seeds, ctx.input.seeds.is_empty())
}

/// Pass A's result for one tile.
pub(super) struct Flood {
    /// Label count.
    pub(super) labels: usize,
    /// Labels holding a non-surface start voxel.
    pub(super) seeded: Vec<u32>,
    /// Per face `axis * 2 + side`: labels of the face's voxels (`LABEL` for surface voxels), with
    /// the voxel's `+axis` face blocked bit in bit 31 on `+` faces.
    pub(super) layers: [Layer; 6],
    /// Peak brick bytes.
    pub(super) bytes: usize,
    /// Bytes of `layers` as built ([`Layer::bytes`] counts run capacity, which a cached copy
    /// does not keep).
    pub(super) layer_bytes: u64,
}

fn label_layer(vox: &Vox, labels: &Field<u32>, axis: usize, side: usize) -> Layer {
    let layout = labels.layout;
    let whole = |b: usize| {
        let l = labels.uniform(b)? & LABEL;
        if side == 1 {
            Some(l | (u32::from(vox.blocked[axis].uniform(b)?) << 31))
        } else {
            Some(l)
        }
    };
    let voxel = |v: [usize; 3]| {
        let l = labels.get(v) & LABEL;
        if side == 1 {
            l | (u32::from(vox.blocked[axis].get(v)) << 31)
        } else {
            l
        }
    };
    face_layer(layout, axis, side, whole, voxel)
}

/// Pass A: one tile's flood components and boundary layers.
pub(super) fn flood_pass(ctx: &Ctx<'_>, t: usize) -> Flood {
    let vox = voxelize_tile(ctx, t);
    let (labels, count) = label(&vox);
    let bytes = vox_bytes(&vox) + labels.bytes();
    let layout = ctx.layout;
    let size = layout.tile();
    let (seeds, edge) = starts(ctx, t);
    let mut seeded = Vec::new();
    for &s in &seeds {
        let v = [s % size, (s / size) % size, s / (size * size)];
        let l = labels.get(v);
        if l != SURF {
            seeded.push(l);
        }
    }
    let layers: [Layer; 6] = std::array::from_fn(|f| label_layer(&vox, &labels, f / 2, f % 2));
    if edge {
        for (f, layer) in layers.iter().enumerate() {
            if ctx.on_boundary(t, f / 2, f % 2) {
                layer.each(|_, _, x| {
                    if x & LABEL != LABEL {
                        seeded.push(x & LABEL);
                    }
                });
            }
        }
    }
    seeded.sort_unstable();
    seeded.dedup();
    let layer_bytes = layers.iter().map(|l| l.bytes() as u64).sum();
    Flood {
        labels: count,
        seeded,
        layers,
        bytes,
        layer_bytes,
    }
}

fn vox_bytes(vox: &Vox) -> usize {
    vox.surface.bytes() + vox.blocked.iter().map(Bits::bytes).sum::<usize>()
}

/// Pass B, first half: the tile's surface and solid voxels, and the peak brick bytes.
pub(super) fn solid_pass(
    ctx: &Ctx<'_>,
    t: usize,
    live: &[Vec<bool>],
    floods: &[Flood],
) -> (Bits, Bits, usize) {
    let vox = voxelize_tile(ctx, t);
    let (solid, bytes) = match &ctx.parity {
        Some(parity) => parity_solid(ctx, t, &vox, parity),
        None => flood_solid(ctx, t, &vox, live, floods),
    };
    (vox.surface, solid, bytes)
}

fn flood_solid(
    ctx: &Ctx<'_>,
    t: usize,
    vox: &Vox,
    live: &[Vec<bool>],
    floods: &[Flood],
) -> (Bits, usize) {
    let (labels, _) = label(vox);
    let layout = ctx.layout;
    let size = layout.tile();
    let edge_len = layout.edge();
    let lo = ctx.lo(t);
    let dims = ctx.grid.dims;
    let occlude = ctx.input.params.surface_occludes;
    let (seeds, edge) = starts(ctx, t);
    let lin = |v: [usize; 3]| (v[2] * size + v[1]) * size + v[0];
    let is_seed = |v: [usize; 3]| seeds.binary_search(&lin(v)).is_ok();
    let own = &live[t];
    let alive = |l: u32| l != SURF && own[l as usize];
    let neighbours: [Option<usize>; 6] = std::array::from_fn(|f| ctx.neighbour(t, f / 2, f % 2));
    // A surface voxel is reached when a live non-surface voxel lies across an unblocked face.
    let touches_live = |v: [usize; 3]| {
        for a in 0..3 {
            let (a1, a2) = ((a + 1) % 3, (a + 2) % 3);
            let p = (v[a2] * size + v[a1]) as u32;
            if v[a] + 1 < size {
                let mut w = v;
                w[a] += 1;
                if !vox.blocked[a].get(v) && alive(labels.get(w)) {
                    return true;
                }
            } else if let Some(n) = neighbours[a * 2 + 1]
                && !vox.blocked[a].get(v)
            {
                let x = floods[n].layers[a * 2].at(p) & LABEL;
                if x != LABEL && live[n][x as usize] {
                    return true;
                }
            }
            if v[a] > 0 {
                let mut w = v;
                w[a] -= 1;
                if !vox.blocked[a].get(w) && alive(labels.get(w)) {
                    return true;
                }
            } else if let Some(n) = neighbours[a * 2] {
                let x = floods[n].layers[a * 2 + 1].at(p);
                let l = x & LABEL;
                if x >> 31 == 0 && l != LABEL && live[n][l as usize] {
                    return true;
                }
            }
        }
        false
    };
    let mut solid = Bits::new(layout, false);
    for b in 0..layout.count() {
        let origin = layout.coords(b).map(|k| k * edge_len);
        if vox.surface.uniform(b) == Some(false) {
            if let Some(l) = labels.uniform(b) {
                solid.bricks[b] = super::grid::BitBrick::Uniform(!alive(l));
                continue;
            }
            for l in 0..layout.volume() {
                if !alive(labels.at(b, l)) {
                    solid.set_at(b, l, true);
                }
            }
            continue;
        }
        for l in 0..layout.volume() {
            let lv = layout.local(l);
            let v = [origin[0] + lv[0], origin[1] + lv[1], origin[2] + lv[2]];
            let surf = vox.surface.at(b, l);
            let reached = if surf {
                let start = if edge {
                    (0..3).any(|a| lo[a] + v[a] == 0 || lo[a] + v[a] + 1 == dims[a])
                } else {
                    is_seed(v)
                };
                start || touches_live(v)
            } else {
                alive(labels.at(b, l))
            };
            let s = if occlude {
                !reached || (surf && !is_seed(v))
            } else {
                !surf && !reached
            };
            if s {
                solid.set_at(b, l, true);
            }
        }
    }
    solid.compact();
    let bytes = vox_bytes(vox) + labels.bytes() + solid.bytes();
    (solid, bytes)
}

fn parity_solid(ctx: &Ctx<'_>, t: usize, vox: &Vox, parity: &(Bvh, Bounds)) -> (Bits, usize) {
    let (bvh, reach) = parity;
    let g = &ctx.grid;
    let layout = ctx.layout;
    let size = layout.tile();
    let lo = ctx.lo(t);
    let mut votes = vec![0u8; size * size * size];
    let mut hits = Vec::new();
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for j in lo[v]..lo[v] + size {
            for i in lo[u]..lo[u] + size {
                let mut a = [0.0f32; 3];
                a[u] = g.coord(u, i) + 0.5 * g.voxel;
                a[v] = g.coord(v, j) + 0.5 * g.voxel;
                let mut b = a;
                a[axis] = reach.min[axis] - 1.0;
                b[axis] = reach.max[axis] + 1.0;
                bvh.crossings(a, b, &mut hits);
                if hits.is_empty() {
                    continue;
                }
                hits.sort_by(f32::total_cmp);
                let span = b[axis] - a[axis];
                for k in lo[axis]..lo[axis] + size {
                    let mut w = [0usize; 3];
                    w[axis] = k;
                    w[u] = i;
                    w[v] = j;
                    let s = (g.centre(w)[axis] - a[axis]) / span;
                    let next = hits.partition_point(|h| *h < s);
                    // Inside only if the crossings on BOTH sides are odd: a closed surface agrees,
                    // an open sheet (a floor without a bottom) does not.
                    if next % 2 == 1 && (hits.len() - next) % 2 == 1 {
                        let lw = [w[0] - lo[0], w[1] - lo[1], w[2] - lo[2]];
                        votes[(lw[2] * size + lw[1]) * size + lw[0]] += 1;
                    }
                }
            }
        }
    }
    let mut solid = Bits::new(layout, false);
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                let v = [x, y, z];
                if votes[(z * size + y) * size + x] == 3 && !vox.surface.get(v) {
                    solid.set(v, true);
                }
            }
        }
    }
    solid.compact();
    let bytes = vox_bytes(vox) + votes.len() + solid.bytes();
    (solid, bytes)
}

/// One cell of a tile before the global stage.
#[derive(Clone, Debug)]
pub(super) struct CellOut {
    /// Global voxel box `[lo, hi)`.
    pub(super) lo: [usize; 3],
    pub(super) hi: [usize; 3],
    /// Open voxels.
    pub(super) voxels: u64,
    /// Up to [`CELL_SAMPLES`] open-voxel centres.
    pub(super) samples: Vec<Vec3>,
}

/// One portal-to-be: cell `c`'s voxel faces on plane `k` toward `nb`, bounded by `rect`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Face {
    pub(super) c: u32,
    pub(super) axis: u8,
    pub(super) side: u8,
    /// Neighbour cell (tile-local here), or [`BOUNDARY`].
    pub(super) nb: u32,
    /// Global plane index on `axis`.
    pub(super) k: u32,
    /// Global voxel bounds `[a1 min, a1 max, a2 min, a2 max]`.
    pub(super) rect: [u32; 4],
}

/// A view-tree node. Nodes are stored parents before children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Node {
    /// A cell (tile-local) or [`SOLID`].
    Leaf(u32),
    /// Children below and above `at` (world) on `axis`.
    Split { axis: u8, at: f32, kids: [u32; 2] },
}

/// Pass B's result for one tile.
pub(super) struct TileOut {
    pub(super) cells: Vec<CellOut>,
    /// In-tile and domain-boundary faces, sorted by `(c, axis, side, nb, k)`.
    pub(super) faces: Vec<Face>,
    /// Per face `axis * 2 + side`: the face voxels' cells.
    pub(super) layers: [Layer; 6],
    pub(super) tree: Vec<Node>,
    /// Per [`FLEX`] leaf value `FLEX | i`: the cells the leaf may take, preferred first (lever
    /// L4's trees only). The global stage gives it the first cell it keeps.
    pub(super) choices: Vec<Vec<u32>>,
    /// `(cell, target)` pairs: the target's box, grown by a voxel, overlaps the cell. As built,
    /// and in the tile cache, the target is its position in the tile's [`overlaps`] list;
    /// [`remap_objects`] turns it into the input index before the global stage.
    pub(super) objects: Vec<(u32, u32)>,
    pub(super) surface: usize,
    pub(super) solid: usize,
    /// Peak brick bytes.
    pub(super) bytes: usize,
    /// Bytes of `layers` as built (see [`Flood::layer_bytes`]).
    pub(super) layer_bytes: u64,
}

/// Pass B, second half: everything the global stage needs from the tile. `overlap` is the tile's
/// [`overlaps`] list.
pub(super) fn cell_pass(
    ctx: &Ctx<'_>,
    t: usize,
    surface_count: usize,
    solid: Bits,
    peak: usize,
    overlap: &[u32],
) -> TileOut {
    let layout = ctx.layout;
    let lo = ctx.lo(t);
    let solid_count = solid.count();
    let (mut cell_of, mut boxes) = match ctx.block_of(t) {
        Some(block) => component_cells(layout, &solid, block),
        None => box_cells(layout, &solid),
    };
    let mut bytes = peak.max(solid.bytes() + cell_of.bytes());
    if let Some((fill, cap)) = ctx.merge
        && (fill.is_finite() || boxes.len() > cap)
    {
        merge(&mut cell_of, &mut boxes, fill, cap);
    }
    if ctx.cell_merge > 0.0 {
        umbra_merge(&mut cell_of, &mut boxes, ctx.grid.voxel, ctx.feature(t));
    }
    drop(solid);
    cell_of.compact();
    let n = boxes.len();
    let mut open = vec![0u64; n];
    for b in 0..layout.count() {
        cell_of.counts(b, |c, k| {
            if c != SOLID {
                open[c as usize] += k as u64;
            }
        });
    }
    let samples = samples(&ctx.grid, &cell_of, lo, &open);
    let layers: [Layer; 6] = std::array::from_fn(|f| {
        face_layer(
            layout,
            f / 2,
            f % 2,
            |b| cell_of.uniform(b),
            |v| cell_of.get(v),
        )
    });
    let faces = faces(ctx, t, &cell_of, &layers);
    let objects = objects(ctx, &cell_of, &boxes, lo, overlap);
    let midpoint = ctx.input.params.midpoint_views;
    let (view, choices) = view_of(&cell_of, midpoint);
    bytes = bytes.max(cell_of.bytes() + view.bytes());
    drop(cell_of);
    let (tree, choices) = if midpoint {
        let frame = tile_frame(&ctx.grid, ctx.input.params.tile, &ctx.tile_ranges[t]);
        let mut view = view;
        if let Some(rg) = view_range(ctx, t) {
            mask_view(&mut view, &rg);
        }
        midpoint_view_tree(&ctx.grid, &view, &choices, lo, frame)
    } else {
        (view_tree(&ctx.grid, &view, lo), Vec::new())
    };
    let cells = boxes
        .iter()
        .zip(open)
        .zip(samples)
        .map(|((&(blo, bhi), voxels), samples)| CellOut {
            lo: [blo[0] + lo[0], blo[1] + lo[1], blo[2] + lo[2]],
            hi: [bhi[0] + lo[0], bhi[1] + lo[1], bhi[2] + lo[2]],
            voxels,
            samples,
        })
        .collect();
    let layer_bytes = layers.iter().map(|l| l.bytes() as u64).sum();
    TileOut {
        cells,
        faces,
        layers,
        tree,
        choices,
        objects,
        surface: surface_count,
        solid: solid_count,
        bytes,
        layer_bytes,
    }
}

/// A tile-local voxel box `[lo, hi)`.
type CellBox = ([usize; 3], [usize; 3]);
type CellBoxes = Vec<CellBox>;

/// A component node: box lo, box hi, first voxel (tile-local linear), open voxels.
type NodeInfo = ([usize; 3], [usize; 3], usize, u64);

/// Connected components of open voxels inside each block of `block` voxels (0: the tile), as
/// tile-local cells numbered in order of their first voxel in z, y, x order.
fn component_cells(layout: Bricks, solid: &Bits, block: usize) -> (Field<u32>, CellBoxes) {
    let edge = layout.edge();
    let size = layout.tile();
    let volume = layout.volume();
    let stride = [1usize, edge, edge * edge];
    let mut field = Field::new(layout, SOLID);
    // Per node: box lo, box hi, first voxel (tile-local linear), open voxels.
    let mut nodes: Vec<NodeInfo> = Vec::new();
    let lin = |v: [usize; 3]| (v[2] * size + v[1]) * size + v[0];
    for b in 0..layout.count() {
        let origin = layout.coords(b).map(|k| k * edge);
        match solid.uniform(b) {
            Some(true) => {}
            Some(false) => {
                field.bricks[b] = Brick::Uniform(nodes.len() as u32);
                nodes.push((origin, origin.map(|k| k + edge), lin(origin), volume as u64));
            }
            None => {
                let mut arr = vec![SOLID; volume];
                let mut stack = Vec::new();
                for l in 0..volume {
                    if arr[l] != SOLID || solid.at(b, l) {
                        continue;
                    }
                    let id = nodes.len() as u32;
                    let first = layout.local(l);
                    let mut nlo = first;
                    let mut nhi = first.map(|k| k + 1);
                    let mut count = 0u64;
                    arr[l] = id;
                    stack.push(l);
                    while let Some(u) = stack.pop() {
                        count += 1;
                        let lu = layout.local(u);
                        for a in 0..3 {
                            nlo[a] = nlo[a].min(lu[a]);
                            nhi[a] = nhi[a].max(lu[a] + 1);
                        }
                        for a in 0..3 {
                            if lu[a] + 1 < edge {
                                let w = u + stride[a];
                                if arr[w] == SOLID && !solid.at(b, w) {
                                    arr[w] = id;
                                    stack.push(w);
                                }
                            }
                            if lu[a] > 0 {
                                let w = u - stride[a];
                                if arr[w] == SOLID && !solid.at(b, w) {
                                    arr[w] = id;
                                    stack.push(w);
                                }
                            }
                        }
                    }
                    let add = |p: [usize; 3]| [0, 1, 2].map(|a| origin[a] + p[a]);
                    let f = add(first);
                    nodes.push((add(nlo), add(nhi), lin(f), count));
                }
                field.bricks[b] = Brick::of(arr);
            }
        }
    }
    let mut uf = UnionFind::new(nodes.len());
    let block_of = |k: usize| k.checked_div(block).unwrap_or(0);
    for b in 0..layout.count() {
        let bc = layout.coords(b);
        for a in 0..3 {
            if bc[a] + 1 >= layout.n || block_of(bc[a] * edge) != block_of((bc[a] + 1) * edge) {
                continue;
            }
            let mut nc = bc;
            nc[a] += 1;
            let n = layout.index(nc);
            match (field.uniform(b), field.uniform(n)) {
                (Some(SOLID), _) | (_, Some(SOLID)) => continue,
                (Some(x), Some(y)) => {
                    uf.union(x, y);
                    continue;
                }
                _ => {}
            }
            let (a1, a2) = ((a + 1) % 3, (a + 2) % 3);
            for j2 in 0..edge {
                for j1 in 0..edge {
                    let mut top = [0usize; 3];
                    top[a] = edge - 1;
                    top[a1] = j1;
                    top[a2] = j2;
                    let mut bot = top;
                    bot[a] = 0;
                    let (x, y) = (
                        field.at(b, layout.inner(top)),
                        field.at(n, layout.inner(bot)),
                    );
                    if x != SOLID && y != SOLID {
                        uf.union(x, y);
                    }
                }
            }
        }
    }
    // Aggregate per root, then number roots by first voxel.
    let mut agg: Vec<Option<NodeInfo>> = vec![None; nodes.len()];
    for (k, node) in nodes.iter().enumerate() {
        let r = uf.find(k as u32) as usize;
        agg[r] = Some(match agg[r] {
            None => *node,
            Some((alo, ahi, af, ac)) => (
                [0, 1, 2].map(|a| alo[a].min(node.0[a])),
                [0, 1, 2].map(|a| ahi[a].max(node.1[a])),
                af.min(node.2),
                ac + node.3,
            ),
        });
    }
    let mut roots: Vec<(usize, usize)> = agg
        .iter()
        .enumerate()
        .filter_map(|(k, a)| a.map(|a| (a.2, k)))
        .collect();
    roots.sort_unstable();
    let mut id = vec![u32::MAX; nodes.len()];
    let mut boxes = Vec::with_capacity(roots.len());
    for (cell, &(_, r)) in roots.iter().enumerate() {
        id[r] = cell as u32;
        let a = agg[r].unwrap_or_else(|| unreachable!());
        boxes.push((a.0, a.1));
    }
    for k in 0..nodes.len() {
        let r = uf.find(k as u32) as usize;
        id[k] = id[r];
    }
    field.map(|x| if x == SOLID { SOLID } else { id[x as usize] });
    (field, boxes)
}

/// Greedy boxes of open voxels in z, y, x order, as the whole-grid baker grew them.
fn box_cells(layout: Bricks, solid: &Bits) -> (Field<u32>, CellBoxes) {
    let size = layout.tile();
    let mut cell_of = Field::new(layout, SOLID);
    let mut boxes = Vec::new();
    let free = |v: [usize; 3], cell_of: &Field<u32>| !solid.get(v) && cell_of.get(v) == SOLID;
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                if !free([x, y, z], &cell_of) {
                    continue;
                }
                let id = boxes.len() as u32;
                let mut x1 = x + 1;
                while x1 < size && free([x1, y, z], &cell_of) {
                    x1 += 1;
                }
                let mut y1 = y + 1;
                'grow_y: while y1 < size {
                    for xx in x..x1 {
                        if !free([xx, y1, z], &cell_of) {
                            break 'grow_y;
                        }
                    }
                    y1 += 1;
                }
                let mut z1 = z + 1;
                'grow_z: while z1 < size {
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
                            cell_of.set([xx, yy, zz], id);
                        }
                    }
                }
                boxes.push(([x, y, z], [x1, y1, z1]));
            }
        }
    }
    cell_of.compact();
    (cell_of, boxes)
}

/// Calls `f(c, d, axis, k, rect, count)` for every pair of different, non-solid neighbouring
/// voxels inside the tile: `c` below, `d` above on `axis`, `k` the upper voxel's tile-local index
/// on `axis`, `rect` the faces' tile-local bounds on `(a1, a2)`, `count` the faces covered.
fn pairs(field: &Field<u32>, mut f: impl FnMut(u32, u32, usize, usize, [usize; 4], usize)) {
    let layout = field.layout;
    let edge = layout.edge();
    let stride = [1usize, edge, edge * edge];
    for b in 0..layout.count() {
        let origin = layout.coords(b).map(|k| k * edge);
        if field.uniform(b).is_none() {
            for l in 0..layout.volume() {
                let c = field.at(b, l);
                if c == SOLID {
                    continue;
                }
                let lv = layout.local(l);
                for a in 0..3 {
                    if lv[a] + 1 >= edge {
                        continue;
                    }
                    let e = field.at(b, l + stride[a]);
                    if e != SOLID && e != c {
                        let (a1, a2) = ((a + 1) % 3, (a + 2) % 3);
                        let (p1, p2) = (origin[a1] + lv[a1], origin[a2] + lv[a2]);
                        f(c, e, a, origin[a] + lv[a] + 1, [p1, p1 + 1, p2, p2 + 1], 1);
                    }
                }
            }
        }
        let bc = layout.coords(b);
        for a in 0..3 {
            if bc[a] + 1 >= layout.n {
                continue;
            }
            let mut nc = bc;
            nc[a] += 1;
            let n = layout.index(nc);
            let (a1, a2) = ((a + 1) % 3, (a + 2) % 3);
            let k = origin[a] + edge;
            match (field.uniform(b), field.uniform(n)) {
                (Some(SOLID), _) | (_, Some(SOLID)) => continue,
                (Some(c), Some(e)) => {
                    if c != e {
                        let rect = [origin[a1], origin[a1] + edge, origin[a2], origin[a2] + edge];
                        f(c, e, a, k, rect, edge * edge);
                    }
                    continue;
                }
                _ => {}
            }
            for j2 in 0..edge {
                for j1 in 0..edge {
                    let mut top = [0usize; 3];
                    top[a] = edge - 1;
                    top[a1] = j1;
                    top[a2] = j2;
                    let mut bot = top;
                    bot[a] = 0;
                    let (c, e) = (
                        field.at(b, layout.inner(top)),
                        field.at(n, layout.inner(bot)),
                    );
                    if c != SOLID && e != SOLID && c != e {
                        let (p1, p2) = (origin[a1] + j1, origin[a2] + j2);
                        f(c, e, a, k, [p1, p1 + 1, p2, p2 + 1], 1);
                    }
                }
            }
        }
    }
}

/// Best-first agglomeration of the tile's seed cells (see [`super::CellMode::Merged`]), then
/// renumbering in order of each merged cell's first seed cell.
fn merge(cell_of: &mut Field<u32>, cells: &mut CellBoxes, fill: f32, max_cells: usize) {
    let n = cells.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(parent: &mut [u32], mut c: u32) -> u32 {
        while parent[c as usize] != c {
            parent[c as usize] = parent[parent[c as usize] as usize];
            c = parent[c as usize];
        }
        c
    }
    // Opening (shared voxel faces) per neighbouring pair.
    let mut adjacent: Vec<HashMap<u32, u32>> = vec![HashMap::new(); n];
    pairs(cell_of, |c, d, _, _, _, count| {
        *adjacent[c as usize].entry(d).or_insert(0) += count as u32;
        *adjacent[d as usize].entry(c).or_insert(0) += count as u32;
    });
    let mut voxels = vec![0usize; n];
    for b in 0..cell_of.layout.count() {
        cell_of.counts(b, |c, k| {
            if c != SOLID {
                voxels[c as usize] += k;
            }
        });
    }
    let mut bounds: Vec<([usize; 3], [usize; 3])> = cells.clone();
    let mut live = n;
    let union_fill = |bounds: &[([usize; 3], [usize; 3])], voxels: &[usize], a: usize, b: usize| {
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
    let mut version = vec![0u32; n];
    let mut heap = std::collections::BinaryHeap::new();
    for (a, adj) in adjacent.iter().enumerate() {
        for (&b, &k) in adj {
            if (a as u32) < b {
                let (f, _, _) = union_fill(&bounds, &voxels, a, b as usize);
                heap.push(((f * 1e6) as u64, k, a as u32, b, 0u32, 0u32));
            }
        }
    }
    while let Some((f, _, a, b, va, vb)) = heap.pop() {
        let (a, b) = (a as usize, b as usize);
        if version[a] != va || version[b] != vb || parent[a] != a as u32 || parent[b] != b as u32 {
            continue;
        }
        if (f as f64) < f64::from(fill) * 1e6 && live <= max_cells {
            continue;
        }
        let (_, lo, hi) = union_fill(&bounds, &voxels, a, b);
        let (keep, gone) = (a.min(b), a.max(b));
        parent[gone] = keep as u32;
        voxels[keep] = voxels[a] + voxels[b];
        bounds[keep] = (lo, hi);
        live -= 1;
        version[keep] += 1;
        let moved = std::mem::take(&mut adjacent[gone]);
        for (m, k) in moved {
            let m = m as usize;
            adjacent[m].remove(&(gone as u32));
            if m != keep {
                *adjacent[keep].entry(m as u32).or_insert(0) += k;
                *adjacent[m].entry(keep as u32).or_insert(0) += k;
            }
        }
        adjacent[keep].remove(&(keep as u32));
        for (&m, &k) in &adjacent[keep] {
            let m = m as usize;
            let (f, _, _) = union_fill(&bounds, &voxels, keep, m);
            let (x, y) = (keep.min(m), keep.max(m));
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
    // Relabel: one cell per root, bounding box of its boxes.
    let mut new_id = vec![u32::MAX; n];
    let mut merged: CellBoxes = Vec::new();
    for c in 0..n {
        let r = root(&mut parent, c as u32) as usize;
        if new_id[r] == u32::MAX {
            new_id[r] = merged.len() as u32;
            merged.push(cells[r]);
        }
        let m = &mut merged[new_id[r] as usize];
        for a in 0..3 {
            m.0[a] = m.0[a].min(cells[c].0[a]);
            m.1[a] = m.1[a].max(cells[c].1[a]);
        }
        new_id[c] = new_id[r];
    }
    cell_of.map(|c| {
        if c == SOLID {
            SOLID
        } else {
            new_id[c as usize]
        }
    });
    *cells = merged;
}

/// Per cell, every stride-th open voxel's centre in z, y, x order (the whole-grid order restricted
/// to the tile).
fn samples(grid: &Grid, cell_of: &Field<u32>, lo: [usize; 3], open: &[u64]) -> Vec<Vec<Vec3>> {
    let layout = cell_of.layout;
    let size = layout.tile();
    let edge = layout.edge();
    let s = layout.shift;
    let mut seen = vec![0u64; open.len()];
    let mut out = vec![Vec::new(); open.len()];
    let stride = |c: usize| open[c].div_ceil(CELL_SAMPLES as u64).max(1);
    for z in 0..size {
        for y in 0..size {
            for bx in 0..layout.n {
                let b = layout.index([bx, y >> s, z >> s]);
                match cell_of.uniform(b) {
                    Some(SOLID) => {}
                    Some(c) => {
                        let c = c as usize;
                        let st = stride(c);
                        let s0 = seen[c];
                        let mut m = s0.div_ceil(st) * st;
                        while m < s0 + edge as u64 {
                            let x = bx * edge + (m - s0) as usize;
                            out[c].push(grid.centre([lo[0] + x, lo[1] + y, lo[2] + z]));
                            m += st;
                        }
                        seen[c] += edge as u64;
                    }
                    None => {
                        for x in bx * edge..(bx + 1) * edge {
                            let c = cell_of.get([x, y, z]);
                            if c == SOLID {
                                continue;
                            }
                            let c = c as usize;
                            if seen[c].is_multiple_of(stride(c)) {
                                out[c].push(grid.centre([lo[0] + x, lo[1] + y, lo[2] + z]));
                            }
                            seen[c] += 1;
                        }
                    }
                }
            }
        }
    }
    out
}

/// The tile's faces toward other cells of the tile and toward the domain boundary.
fn faces(ctx: &Ctx<'_>, t: usize, cell_of: &Field<u32>, layers: &[Layer; 6]) -> Vec<Face> {
    let lo = ctx.lo(t);
    let size = ctx.layout.tile();
    // (cell, axis, side, neighbour, plane) -> face pieces.
    type Key = (u32, u8, u8, u32, u32);
    let mut map: HashMap<Key, Vec<[u32; 4]>> = HashMap::new();
    let mut add = |key: Key, r: [u32; 4]| {
        map.entry(key).or_default().push(r);
    };
    pairs(cell_of, |c, d, a, k, rect, _| {
        let (a1, a2) = ((a + 1) % 3, (a + 2) % 3);
        let r = [
            (rect[0] + lo[a1]) as u32,
            (rect[1] + lo[a1]) as u32,
            (rect[2] + lo[a2]) as u32,
            (rect[3] + lo[a2]) as u32,
        ];
        let k = (k + lo[a]) as u32;
        add((c, a as u8, 1, d, k), r);
        add((d, a as u8, 0, c, k), r);
    });
    for (f, layer) in layers.iter().enumerate() {
        let (a, side) = (f / 2, f % 2);
        if !ctx.on_boundary(t, a, side) {
            continue;
        }
        let (a1, a2) = ((a + 1) % 3, (a + 2) % 3);
        let k = if side == 1 { ctx.grid.dims[a] } else { 0 } as u32;
        layer.each(|s, e, c| {
            if c == SOLID {
                return;
            }
            for r in run_rects(s, e, size as u32) {
                let r = [
                    r[0] + lo[a1] as u32,
                    r[1] + lo[a1] as u32,
                    r[2] + lo[a2] as u32,
                    r[3] + lo[a2] as u32,
                ];
                add((c, a as u8, side as u8, BOUNDARY, k), r);
            }
        });
    }
    let cost = ctx.portal_cost(false);
    let mut out: Vec<Face> = map
        .into_iter()
        .flat_map(|((c, axis, side, nb, k), mut rects)| {
            rects.sort_unstable();
            coalesce(&rects, cost).into_iter().map(move |rect| Face {
                c,
                axis,
                side,
                nb,
                k,
                rect,
            })
        })
        .collect();
    out.sort_unstable_by_key(|f| (f.c, f.axis, f.side, f.nb, f.k, f.rect));
    out
}

/// The tile-local voxel range of target `o`'s box, grown by one voxel and clipped to the domain
/// and to the tile at `lo`; `None` when it does not reach the tile.
fn target_range(ctx: &Ctx<'_>, o: usize, lo: [usize; 3]) -> Option<[(usize, usize); 3]> {
    let g = &ctx.grid;
    let domain = &ctx.domain;
    let e = g.voxel;
    let t = &ctx.input.targets[o];
    let overlaps =
        (0..3).all(|k| t.bounds.max[k] >= domain.min[k] && t.bounds.min[k] <= domain.max[k]);
    if !overlaps {
        return None;
    }
    let r = [0, 1, 2].map(|a| g.range(a, t.bounds.min[a] - e, t.bounds.max[a] + e));
    let cl = clip(r, lo, ctx.layout.tile())?;
    Some([0, 1, 2].map(|a| (cl[a].0 - lo[a], cl[a].1 - lo[a])))
}

/// The targets whose box, grown by one voxel, reaches tile `t` inside the domain, as input
/// indices in input order. These are the only targets the tile's object list depends on.
pub(super) fn overlaps(ctx: &Ctx<'_>, t: usize) -> Vec<u32> {
    let lo = ctx.lo(t);
    (0..ctx.input.targets.len())
        .filter(|&o| target_range(ctx, o, lo).is_some())
        .map(|o| o as u32)
        .collect()
}

/// Turns a [`TileOut`]'s object positions in the tile's [`overlaps`] list into input indices.
pub(super) fn remap_objects(out: &mut TileOut, overlap: &[u32]) {
    for o in &mut out.objects {
        o.1 = overlap[o.1 as usize];
    }
}

/// `(cell, position)` for every cell with open voxels within one voxel of a target's box inside
/// the tile, and every cell whose box overlaps that grown box, where `position` indexes the tile's
/// [`overlaps`] list `overlap`. The box rule keeps objects whose surroundings the voxels closed:
/// a bottle on a shelf whose grown box is all surface voxels was listed in no cell (never visible)
/// and a cell that wraps an object without touching it did not list it; on Bistro both culled
/// objects visible through openings of the smallest hole (`plan-bench-gaps.md` item 3). A target
/// in a sealed pocket is now listed in a cell around the pocket when their boxes overlap.
fn objects(
    ctx: &Ctx<'_>,
    cell_of: &Field<u32>,
    boxes: &CellBoxes,
    lo: [usize; 3],
    overlap: &[u32],
) -> Vec<(u32, u32)> {
    let layout = cell_of.layout;
    let edge = layout.edge();
    let s = layout.shift;
    let mut out = Vec::new();
    // Per cell, the last target listed in it (+1), so each pair is pushed once.
    let mut last = vec![0u32; boxes.len()];
    for (o, &target) in overlap.iter().enumerate() {
        let mut add = |c: u32| {
            if last[c as usize] != o as u32 + 1 {
                last[c as usize] = o as u32 + 1;
                out.push((c, o as u32));
            }
        };
        let Some(q) = target_range(ctx, target as usize, lo) else {
            continue;
        };
        for bz in q[2].0 >> s..=(q[2].1 - 1) >> s {
            for by in q[1].0 >> s..=(q[1].1 - 1) >> s {
                for bx in q[0].0 >> s..=(q[0].1 - 1) >> s {
                    let b = layout.index([bx, by, bz]);
                    match cell_of.uniform(b) {
                        Some(SOLID) => {}
                        Some(c) => add(c),
                        None => {
                            let bo = [bx * edge, by * edge, bz * edge];
                            for z in q[2].0.max(bo[2])..q[2].1.min(bo[2] + edge) {
                                for y in q[1].0.max(bo[1])..q[1].1.min(bo[1] + edge) {
                                    for x in q[0].0.max(bo[0])..q[0].1.min(bo[0] + edge) {
                                        let c = cell_of.get([x, y, z]);
                                        if c != SOLID {
                                            add(c);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for (c, b) in boxes.iter().enumerate() {
            if (0..3).all(|k| b.0[k] < q[k].1 && q[k].0 < b.1[k]) {
                add(c as u32);
            }
        }
    }
    out.shrink_to_fit();
    out
}

/// A view value with this bit (and not [`SOLID`]) is a solid voxel that may resolve to any cell of
/// `choices[value & !FLEX]`, and a leaf value with it one of several cells
/// ([`TileOut::choices`]); tile cell ids stay below it.
pub(super) const FLEX: u32 = 1 << 30;
/// How far (voxels, per axis) a [`FLEX`] surface voxel's choices reach. Two would save Bistro at
/// 32 m another 7 % of the tome, at the cost of an eye inside a surface finding a cell a quarter
/// metre away.
const REACH: usize = 1;

/// What the view tree resolves, and the cell choices of [`FLEX`] values. Solid voxels sharing a
/// face with open space of their tile must resolve to a nearby cell, so an eye inside a surface
/// finds the space beside it. Without `flex` each has one owner: the cell directly above (a
/// floor's surface belongs to the space a character stands in), else the most frequent
/// horizontal neighbour, else the one below. With `flex` (lever L4's trees) the cell of any open
/// voxel among its 26 neighbours will do, which lets a leaf end at a surface between two cells
/// instead of tracing who owns each of its voxels (Bistro at 32 m: 1,168,832 B to 673,216 B).
/// Deeper solid voxels, those touching open space at an edge or corner only included, stay
/// "don't care".
fn view_of(cell_of: &Field<u32>, flex: bool) -> (Field<u32>, Vec<Vec<u32>>) {
    let layout = cell_of.layout;
    let size = layout.tile();
    let edge = layout.edge();
    let open = |n: [usize; 3]| {
        let c = cell_of.get(n);
        (c != SOLID).then_some(c)
    };
    let owner = |v: [usize; 3]| -> u32 {
        let [x, y, z] = v;
        let above = (z + 1 < size).then(|| open([x, y, z + 1])).flatten();
        above
            .or_else(|| {
                let mut side = [0u32; 4];
                let mut k = 0;
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx as usize >= size || ny as usize >= size {
                        continue;
                    }
                    if let Some(c) = open([nx as usize, ny as usize, z]) {
                        side[k] = c;
                        k += 1;
                    }
                }
                let side = &mut side[..k];
                side.sort_unstable();
                side.chunk_by(|a, b| a == b)
                    .max_by_key(|run| (run.len(), core::cmp::Reverse(run[0])))
                    .map(|run| run[0])
            })
            .or_else(|| (z > 0).then(|| open([x, y, z - 1])).flatten())
            .unwrap_or(SOLID)
    };
    // The distinct cells of a solid voxel's six neighbours inside the tile, sorted.
    let neighbours = |v: [usize; 3]| -> Vec<u32> {
        let mut out = Vec::new();
        let r = REACH as i64;
        for dz in -r..=r {
            for dy in -r..=r {
                for dx in -r..=r {
                    let n = [v[0] as i64 + dx, v[1] as i64 + dy, v[2] as i64 + dz];
                    if n.iter().any(|&k| k < 0 || k as usize >= size) {
                        continue;
                    }
                    if let Some(c) = open(n.map(|k| k as usize)) {
                        out.push(c);
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    };
    let mut choices: Vec<Vec<u32>> = Vec::new();
    let mut choice_at: HashMap<Vec<u32>, u32> = HashMap::new();
    let mut view = Field::new(layout, SOLID);
    for b in 0..layout.count() {
        let bc = layout.coords(b);
        match cell_of.uniform(b) {
            Some(SOLID) => {
                let quiet = (0..3).all(|a| {
                    let below = bc[a] == 0 || {
                        let mut nc = bc;
                        nc[a] -= 1;
                        cell_of.uniform(layout.index(nc)) == Some(SOLID)
                    };
                    let above = bc[a] + 1 >= layout.n || {
                        let mut nc = bc;
                        nc[a] += 1;
                        cell_of.uniform(layout.index(nc)) == Some(SOLID)
                    };
                    below && above
                });
                if quiet {
                    continue;
                }
            }
            Some(c) => {
                view.bricks[b] = Brick::Uniform(c);
                continue;
            }
            None => {}
        }
        let origin = bc.map(|k| k * edge);
        let mut arr = vec![SOLID; layout.volume()];
        for (l, out) in arr.iter_mut().enumerate() {
            let lv = layout.local(l);
            let v = [origin[0] + lv[0], origin[1] + lv[1], origin[2] + lv[2]];
            let c = cell_of.at(b, l);
            *out = if c != SOLID {
                c
            } else if !flex {
                owner(v)
            } else if owner(v) == SOLID {
                SOLID
            } else {
                let set = neighbours(v);
                match set.len() {
                    0 => SOLID,
                    1 => set[0],
                    _ => {
                        let next = choices.len() as u32;
                        let i = *choice_at.entry(set).or_insert_with_key(|set| {
                            choices.push(set.clone());
                            next
                        });
                        FLEX | i
                    }
                }
            };
        }
        view.bricks[b] = Brick::of(arr);
    }
    view.compact();
    (view, choices)
}

/// The bricks overlapping tile-local region `rg`, each with its clipped part.
fn region_bricks(layout: Bricks, rg: &[[usize; 2]; 3], mut f: impl FnMut(usize, [[usize; 2]; 3])) {
    let s = layout.shift;
    let edge = layout.edge();
    for bz in rg[2][0] >> s..=(rg[2][1] - 1) >> s {
        for by in rg[1][0] >> s..=(rg[1][1] - 1) >> s {
            for bx in rg[0][0] >> s..=(rg[0][1] - 1) >> s {
                let bo = [bx * edge, by * edge, bz * edge];
                let c = [0, 1, 2].map(|a| [rg[a][0].max(bo[a]), rg[a][1].min(bo[a] + edge)]);
                f(layout.index([bx, by, bz]), c);
            }
        }
    }
}

/// The cell a region's leaf takes, or `None` when no cell suits every owned voxel. A [`FLEX`]
/// voxel accepts any cell of its `choices` entry. The leaf takes the cell of the region's open
/// voxels, else the lowest cell every FLEX voxel accepts, else `SOLID` (nothing owned); with the
/// other cells every FLEX voxel accepts, ascending, which the leaf may fall back to when the
/// global stage removes its cell.
fn region_uniform(
    view: &Field<u32>,
    choices: &[Vec<u32>],
    rg: &[[usize; 2]; 3],
) -> Option<(u32, Vec<u32>)> {
    let mut first = SOLID;
    // The cells every FLEX voxel seen so far accepts (all contain `first` once it is set).
    let mut common: Option<Vec<u32>> = None;
    let mut last_flex = SOLID;
    let mut mixed = false;
    region_bricks(view.layout, rg, |b, c| {
        if mixed {
            return;
        }
        let mut see = |x: u32| {
            if x == SOLID || x == first {
                return true;
            }
            if x & FLEX != 0 {
                if x == last_flex {
                    return true;
                }
                last_flex = x;
                let set = &choices[(x & !FLEX) as usize];
                let keep = common.get_or_insert_with(|| set.clone());
                keep.retain(|c| set.binary_search(c).is_ok());
                mixed = keep.is_empty() || (first != SOLID && keep.binary_search(&first).is_err());
                return !mixed;
            }
            if first != SOLID
                || common
                    .as_ref()
                    .is_some_and(|k| k.binary_search(&x).is_err())
            {
                mixed = true;
                return false;
            }
            first = x;
            true
        };
        match view.uniform(b) {
            Some(x) => {
                see(x);
            }
            None => {
                for z in c[2][0]..c[2][1] {
                    for y in c[1][0]..c[1][1] {
                        for x in c[0][0]..c[0][1] {
                            if !see(view.get([x, y, z])) {
                                return;
                            }
                        }
                    }
                }
            }
        }
    });
    if mixed {
        return None;
    }
    let mut others = common.unwrap_or_default();
    if first == SOLID && !others.is_empty() {
        first = others.remove(0);
    } else {
        others.retain(|&c| c != first);
    }
    Some((first, others))
}

/// Chooses a view-tree split for a region holding open voxels of several cells: a face of one
/// cell's extent inside the region, cutting through as few other cells as possible and then
/// dividing them as evenly as possible. Candidates are visited in order of each cell's first
/// voxel in z, y, x order, as the whole-grid baker found them.
fn choose_split(view: &Field<u32>, rg: &[[usize; 2]; 3]) -> (usize, usize) {
    let size = view.layout.tile();
    // (cell, extent lo, extent hi, first voxel).
    let mut ext: Vec<(u32, [usize; 3], [usize; 3], usize)> = Vec::new();
    let mut at: HashMap<u32, usize> = HashMap::new();
    let lin = |v: [usize; 3]| (v[2] * size + v[1]) * size + v[0];
    let mut see = |c: u32, lo: [usize; 3], hi: [usize; 3]| {
        if c == SOLID {
            return;
        }
        let i = *at.entry(c).or_insert_with(|| {
            ext.push((c, lo, hi, lin(lo)));
            ext.len() - 1
        });
        let e = &mut ext[i];
        for k in 0..3 {
            e.1[k] = e.1[k].min(lo[k]);
            e.2[k] = e.2[k].max(hi[k]);
        }
        e.3 = e.3.min(lin(lo));
    };
    region_bricks(view.layout, rg, |b, c| match view.uniform(b) {
        Some(x) => see(x, [c[0][0], c[1][0], c[2][0]], [c[0][1], c[1][1], c[2][1]]),
        None => {
            for z in c[2][0]..c[2][1] {
                for y in c[1][0]..c[1][1] {
                    for x in c[0][0]..c[0][1] {
                        see(view.get([x, y, z]), [x, y, z], [x + 1, y + 1, z + 1]);
                    }
                }
            }
        }
    });
    ext.sort_unstable_by_key(|e| e.3);
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

/// The tile's view tree, built breadth first: a region is a leaf when its owned voxels all belong
/// to one cell (or none: a solid leaf), else it is split where cells meet.
fn view_tree(grid: &Grid, view: &Field<u32>, lo: [usize; 3]) -> Vec<Node> {
    let size = view.layout.tile();
    let mut nodes = vec![Node::Leaf(SOLID)];
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((0usize, [[0usize, size]; 3]));
    while let Some((i, rg)) = queue.pop_front() {
        if let Some((c, _)) = region_uniform(view, &[], &rg) {
            nodes[i] = Node::Leaf(c);
            continue;
        }
        let (axis, at) = choose_split(view, &rg);
        let (mut below, mut above) = (rg, rg);
        below[axis][1] = at;
        above[axis][0] = at;
        let k = nodes.len() as u32;
        nodes[i] = Node::Split {
            axis: axis as u8,
            at: grid.coord(axis, lo[axis] + at),
            kids: [k, k + 1],
        };
        nodes.push(Node::Leaf(SOLID));
        nodes.push(Node::Leaf(SOLID));
        queue.push_back((k as usize, below));
        queue.push_back((k as usize + 1, above));
    }
    nodes
}

/// Umbra's level shift `k` of a tile whose box `frame` overlaps no view volume
/// (`TileGrid_makeTileRecord`): with `e`
/// the smallest extent of the view volume `view` and `r` the smallest occluder,
/// `k = floor(log2(e / (4 r) - 1)) + 1`, at least 1. Such a tile is built at the feature
/// `r * 2^k` (12 m for the stock 64 m-thick playable layer at `r` 6), which is why a stock tome has
/// about 3 cells per 32 m tile under and over the layer against 30 inside it. A tile the volume
/// cuts through counts as inside (0). 0 without a view volume to speak of.
pub(super) fn level_shift(view: &Aabb, frame: &Aabb, r: f32) -> u32 {
    let overlaps = (0..3).all(|a| frame.min[a] < view.max[a] && frame.max[a] > view.min[a]);
    if overlaps || r <= 0.0 {
        return 0;
    }
    let e = (0..3)
        .map(|a| view.max[a] - view.min[a])
        .fold(f32::INFINITY, f32::min);
    if !e.is_finite() {
        return 0;
    }
    let q = e / (4.0 * r) - 1.0;
    if q < 1.0 {
        1
    } else {
        q.log2().floor() as u32 + 1
    }
}

/// Umbra's cell level for a tile of edge `tile` metres at feature `feature`, as a block edge
/// in voxels: `tile / 2^round(log2(tile / feature))`, at least one voxel.
pub(super) fn cell_level_block(tile: f32, feature: f32, voxel: f32) -> usize {
    let level = (tile / feature).log2().round().max(0.0);
    ((tile / 2f32.powf(level)) / voxel).round().max(1.0) as usize
}

/// The world box of a tile over tile-grid range `r`, as the tome stores it (and as the runtime
/// starts a view-tree descent from).
pub(super) fn tile_frame(grid: &Grid, tile: f32, r: &[[usize; 2]; 3]) -> Aabb {
    Aabb {
        min: [0, 1, 2].map(|a| grid.origin[a] + r[a][0] as f32 * tile),
        max: [0, 1, 2].map(|a| grid.origin[a] + r[a][1] as f32 * tile),
    }
}

/// The runtime's implicit split of box `b` on `axis`: f32 `(min + max) * 0.5`.
pub(super) fn midpoint(b: &Aabb, axis: usize) -> f32 {
    (b.min[axis] + b.max[axis]) * 0.5
}

/// The tile-local voxels `[lo, hi)` on `axis` that world interval `(min, max)` overlaps with
/// positive length, `base` being the tile's first global voxel on that axis.
fn overlapped(
    grid: &Grid,
    axis: usize,
    base: usize,
    size: usize,
    min: f32,
    max: f32,
) -> [usize; 2] {
    // The first index in `0..size` where `below` turns false (`below` is monotone).
    let first = |below: &dyn Fn(usize) -> bool| {
        let (mut a, mut b) = (0usize, size);
        while a < b {
            let m = a + (b - a) / 2;
            if below(m) {
                a = m + 1;
            } else {
                b = m;
            }
        }
        a
    };
    let lo = first(&|k| grid.coord(axis, base + k + 1) <= min);
    let hi = first(&|k| grid.coord(axis, base + k) < max);
    [lo, hi.max(lo)]
}

/// Splits the box of node `i` at `at` on `axis`, appending its two children (solid leaves for now)
/// and queueing them with their boxes.
fn split_node(
    nodes: &mut Vec<Node>,
    queue: &mut std::collections::VecDeque<(usize, Aabb)>,
    i: usize,
    b: Aabb,
    axis: usize,
) {
    let at = midpoint(&b, axis);
    assert!(
        b.min[axis] < at && at < b.max[axis],
        "midpoint {at} not inside ({}, {}) on axis {axis}",
        b.min[axis],
        b.max[axis]
    );
    let (mut below, mut above) = (b, b);
    below.max[axis] = at;
    above.min[axis] = at;
    let k = nodes.len() as u32;
    nodes[i] = Node::Split {
        axis: axis as u8,
        at,
        kids: [k, k + 1],
    };
    nodes.push(Node::Leaf(SOLID));
    nodes.push(Node::Leaf(SOLID));
    queue.push_back((k as usize, below));
    queue.push_back((k as usize + 1, above));
}

/// The distinct cells of the open voxels of tile-local region `rg` (not [`SOLID`] or [`FLEX`]),
/// into `out` (cleared first).
fn region_cells(view: &Field<u32>, rg: &[[usize; 2]; 3], out: &mut Vec<u32>) {
    out.clear();
    let mut see = |x: u32| {
        if x & FLEX == 0 && !out.contains(&x) {
            out.push(x);
        }
    };
    region_bricks(view.layout, rg, |b, c| match view.uniform(b) {
        Some(x) => see(x),
        None => {
            for z in c[2][0]..c[2][1] {
                for y in c[1][0]..c[1][1] {
                    for x in c[0][0]..c[0][1] {
                        see(view.get([x, y, z]));
                    }
                }
            }
        }
    });
}

/// The axis to split a lever-L4 node on, its box `b` overlapping the voxels `rg` (more than one
/// cell). Every axis that overlaps two or more voxels is tried at its midpoint; the best leaves the
/// fewest children holding several cells, then the fewest cells over both children, then is
/// [`choose_split`]'s axis, then the lowest axis.
fn midpoint_axis(
    grid: &Grid,
    view: &Field<u32>,
    choices: &[Vec<u32>],
    lo: [usize; 3],
    b: &Aabb,
    rg: &[[usize; 2]; 3],
) -> usize {
    let size = view.layout.tile();
    let preferred = choose_split(view, rg).0;
    let mut best: Option<((usize, usize, bool), usize)> = None;
    let mut cells = Vec::new();
    for axis in 0..3 {
        if rg[axis][1] - rg[axis][0] < 2 {
            continue;
        }
        let at = midpoint(b, axis);
        let mut mixed = 0usize;
        let mut total = 0usize;
        for (min, max) in [(b.min[axis], at), (at, b.max[axis])] {
            let mut half = *rg;
            half[axis] = overlapped(grid, axis, lo[axis], size, min, max);
            if half[axis][0] < half[axis][1] {
                region_cells(view, &half, &mut cells);
                mixed += usize::from(region_uniform(view, choices, &half).is_none());
                total += cells.len();
            }
        }
        let key = (mixed, total, axis != preferred);
        if best.is_none_or(|(k, _)| key < k) {
            best = Some((key, axis));
        }
    }
    best.map_or(preferred, |(_, a)| a)
}

/// Lever L4: the tile's view tree with every split at the implicit midpoint of its node's box,
/// built breadth first from the tile's box `frame`. The axis is [`midpoint_axis`]'s over the voxels
/// the box overlaps; a box is a leaf when those voxels, partly covered ones included, all belong
/// to one cell (solid voxels being don't-care), so no point of an open voxel reaches another
/// cell's leaf. Where a box's span is not a power of two voxels the midpoint falls inside a voxel,
/// and the boxes on either side both overlap it.
///
/// Termination: a box that is not a leaf overlaps two or more voxels on the chosen axis, so a voxel
/// plane (a float) lies strictly inside it, and then the f32 midpoint does too; every split
/// therefore shrinks the box, and a box overlapping one voxel per axis is always a leaf.
fn midpoint_view_tree(
    grid: &Grid,
    view: &Field<u32>,
    choices: &[Vec<u32>],
    lo: [usize; 3],
    frame: Aabb,
) -> (Vec<Node>, Vec<Vec<u32>>) {
    let size = view.layout.tile();
    let mut nodes = vec![Node::Leaf(SOLID)];
    let mut leaf_choices: Vec<Vec<u32>> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((0usize, frame));
    while let Some((i, b)) = queue.pop_front() {
        let rg: [[usize; 2]; 3] =
            std::array::from_fn(|a| overlapped(grid, a, lo[a], size, b.min[a], b.max[a]));
        if rg.iter().any(|r| r[0] >= r[1]) {
            // No voxel of the tile under the box (a frame reaching past its voxels): solid.
            continue;
        }
        if let Some((c, others)) = region_uniform(view, choices, &rg) {
            nodes[i] = Node::Leaf(if others.is_empty() {
                c
            } else {
                leaf_choices.push([c].into_iter().chain(others).collect());
                FLEX | (leaf_choices.len() as u32 - 1)
            });
            continue;
        }
        let axis = midpoint_axis(grid, view, choices, lo, &b, &rg);
        split_node(&mut nodes, &mut queue, i, b, axis);
    }
    (nodes, leaf_choices)
}

/// With [`super::Params::view_mask`] (and lever L4): the tile-local voxels `[lo, hi)` per axis
/// that the view volume reaches, closed and grown by one voxel each way so that a camera on the
/// volume's boundary, or on a voxel plane, is never outside them. `None` when the mask is off.
/// An empty range masks the whole tile.
pub(super) fn view_range(ctx: &Ctx<'_>, t: usize) -> Option<[[usize; 2]; 3]> {
    let p = &ctx.input.params;
    if !(p.view_mask && p.midpoint_views) {
        return None;
    }
    let g = &ctx.grid;
    let vv = &ctx.input.view_volume;
    let lo = ctx.lo(t);
    let size = ctx.layout.tile();
    Some(std::array::from_fn(|a| {
        let (l, h) = g.range(a, vv.min[a], vv.max[a]);
        let clip = |k: usize| k.clamp(lo[a], lo[a] + size) - lo[a];
        let (l, h) = (clip(l.saturating_sub(1)), clip(h + 1));
        [l, h.max(l)]
    }))
}

/// Makes every voxel outside the tile-local range `rg` "don't care", as Umbra's strict view
/// volumes (optimizer flag 4, which the stock tomes set) leave cells the camera cannot enter out of the view
/// tree (`viewTree_fromOctree_r` skips filtered cells). The view tree then
/// resolves cells only where the camera may be: on Bistro at 32 m the trees fall from 346k to
/// 65k nodes (Umbra 39k), since the 32 m tiles reach far above the street-level view volume.
/// Cells keep their portals and objects; a cell wholly outside the volume has no leaf.
fn mask_view(view: &mut Field<u32>, rg: &[[usize; 2]; 3]) {
    let layout = view.layout;
    let edge = layout.edge();
    let inside = |v: [usize; 3]| (0..3).all(|a| rg[a][0] <= v[a] && v[a] < rg[a][1]);
    for b in 0..layout.count() {
        let o = layout.coords(b).map(|k| k * edge);
        if (0..3).all(|a| rg[a][0] <= o[a] && o[a] + edge <= rg[a][1]) {
            continue;
        }
        if (0..3).any(|a| o[a] + edge <= rg[a][0] || rg[a][1] <= o[a]) {
            view.bricks[b] = Brick::Uniform(SOLID);
            continue;
        }
        let arr = (0..layout.volume())
            .map(|l| {
                let lv = layout.local(l);
                if inside([o[0] + lv[0], o[1] + lv[1], o[2] + lv[2]]) {
                    view.at(b, l)
                } else {
                    SOLID
                }
            })
            .collect();
        view.bricks[b] = Brick::of(arr);
    }
}

/// Whether every split of `tree` lies at the runtime's implicit midpoint of its node's box, the
/// boxes descending from `frame`.
pub(super) fn is_midpoint_tree(tree: &[Node], frame: Aabb) -> bool {
    let mut stack = vec![(0usize, frame)];
    while let Some((i, b)) = stack.pop() {
        if let Node::Split { axis, at, kids } = tree[i] {
            let a = axis as usize;
            if at.to_bits() != midpoint(&b, a).to_bits() {
                return false;
            }
            let (mut below, mut above) = (b, b);
            below.max[a] = at;
            above.min[a] = at;
            stack.push((kids[0] as usize, below));
            stack.push((kids[1] as usize, above));
        }
    }
    true
}

/// Upper bound of a tile's peak brick bytes, from its touched bricks (`touched`) and the bricks
/// within one brick of them (`near`).
pub(super) fn tile_bytes_bound(
    layout: Bricks,
    touched: usize,
    near: usize,
    flood: bool,
    dense_cells: bool,
) -> usize {
    let all = layout.count();
    let bits = |d: usize| bits_bytes(layout, d);
    let vals = |d: usize| field_bytes::<u32>(layout, d);
    let cells = if dense_cells { all } else { touched };
    let view = if dense_cells { all } else { near };
    // Saturating: a bound that overflows is refused as too large by the caller.
    let votes = layout.tile().saturating_pow(3);
    let first = if flood {
        bits(touched)
            .saturating_mul(5)
            .saturating_add(vals(touched))
    } else {
        bits(touched)
            .saturating_mul(4)
            .saturating_add(votes)
            .saturating_add(bits(all))
    };
    let solid = if flood { touched } else { all };
    let second = bits(solid).saturating_add(vals(cells));
    let third = vals(cells).saturating_add(vals(view));
    first.max(second).max(third)
}

/// A rectangle of a cell's boundary for [`umbra_merge`]'s costs, in tile-local voxel units: on
/// plane `at` of `axis`, over `[lo, hi]` on `(axis + 1) % 3, (axis + 2) % 3`. `inward` is the
/// side of the plane the cell lies on (+1 above, -1 below).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Opening {
    axis: usize,
    at: f64,
    inward: f64,
    lo: [f64; 2],
    hi: [f64; 2],
    /// The cell it belongs to.
    cell: u32,
}

impl Opening {
    fn area(&self) -> f64 {
        (self.hi[0] - self.lo[0]) * (self.hi[1] - self.lo[1])
    }
    /// The point at `(u, v)` of the rectangle.
    fn point(&self, u: f64, v: f64) -> [f64; 3] {
        let mut p = [0.0; 3];
        p[self.axis] = self.at;
        p[(self.axis + 1) % 3] = self.lo[0] + u * (self.hi[0] - self.lo[0]);
        p[(self.axis + 2) % 3] = self.lo[1] + v * (self.hi[1] - self.lo[1]);
        p
    }
    /// `p` lies strictly on the cell's side of the plane.
    fn faces(&self, p: &[f64; 3]) -> bool {
        (p[self.axis] - self.at) * self.inward > 0.0
    }
    /// Same orientation and plane.
    fn coplanar(&self, o: &Opening) -> bool {
        self.axis == o.axis && self.inward == o.inward && self.at == o.at
    }
    /// The segment `a`-`b` crosses the plane strictly between its ends, inside the rectangle
    /// (closed).
    fn crossed(&self, a: &[f64; 3], b: &[f64; 3]) -> bool {
        let (da, db) = (a[self.axis] - self.at, b[self.axis] - self.at);
        if da == 0.0 || db == 0.0 || (da > 0.0) == (db > 0.0) {
            return false;
        }
        let t = da / (da - db);
        (0..2).all(|k| {
            let ax = (self.axis + 1 + k) % 3;
            let x = a[ax] + t * (b[ax] - a[ax]);
            self.lo[k] <= x && x <= self.hi[k]
        })
    }
}

/// The radical inverse of `i` in base `b` (Umbra's Halton sequences have no seed).
fn halton(mut i: usize, b: usize) -> f64 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= b as f64;
        r += f * (i % b) as f64;
        i /= b;
    }
    r
}

/// The rectangle of `rects` at `u` of their cumulative area distribution `cdf`.
fn pick<'a>(rects: &[&'a Opening], cdf: &[f64], u: f64) -> &'a Opening {
    let total = cdf[cdf.len() - 1];
    let i = cdf.partition_point(|&c| c <= u * total);
    rects[i.min(rects.len() - 1)]
}

/// Cumulative areas of `rects`.
fn cdf(rects: &[&Opening]) -> Vec<f64> {
    let mut sum = 0.0;
    rects
        .iter()
        .map(|r| {
            sum += r.area();
            sum
        })
        .collect()
}

/// Rectangles `[a1 lo, a1 hi, a2 lo, a2 hi]` that exactly cover the unit squares of `runs`
/// (`(a2 row, a1 lo, a1 hi)`): runs touching within a row are joined, then equal runs of
/// consecutive rows are stacked.
fn decompose(mut runs: Vec<(usize, usize, usize)>) -> Vec<[usize; 4]> {
    runs.sort_unstable();
    let mut rows: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
    for (row, lo, hi) in runs {
        match rows.last_mut() {
            Some((r, list)) if *r == row => match list.last_mut() {
                Some(last) if last.1 >= lo => last.1 = last.1.max(hi),
                _ => list.push((lo, hi)),
            },
            _ => rows.push((row, vec![(lo, hi)])),
        }
    }
    let mut out = Vec::new();
    let mut open: Vec<((usize, usize), usize)> = Vec::new();
    let mut prev = usize::MAX;
    for (row, list) in rows {
        let mut next = Vec::with_capacity(list.len());
        for &k in &list {
            let start = open
                .iter()
                .find(|(o, _)| *o == k && prev.wrapping_add(1) == row)
                .map_or(row, |o| o.1);
            next.push((k, start));
        }
        for &((lo, hi), start) in &open {
            if !next.iter().any(|&(k, s)| k == (lo, hi) && s == start) {
                out.push([lo, hi, start, prev + 1]);
            }
        }
        open = next;
        prev = row;
    }
    for ((lo, hi), start) in open {
        out.push([lo, hi, start, prev + 1]);
    }
    out
}

/// A tile's cells as [`umbra_merge`] sees them: per cell its tile-face rectangles and its portals
/// to the tile's other cells (one bounding rectangle per neighbour and plane).
struct CellGraph {
    faces: Vec<Vec<Opening>>,
    /// Per cell, `(neighbour, rectangle)` in neighbour order, the rectangle facing the cell.
    portals: Vec<Vec<(u32, Opening)>>,
}

impl CellGraph {
    fn new(cell_of: &Field<u32>, n: usize) -> CellGraph {
        let layout = cell_of.layout;
        let size = layout.tile();
        let mut faces: Vec<Vec<Opening>> = vec![Vec::new(); n];
        for f in 0..6 {
            let (axis, side) = (f / 2, f % 2);
            let layer = face_layer(
                layout,
                axis,
                side,
                |b| cell_of.uniform(b),
                |v| cell_of.get(v),
            );
            let mut runs: std::collections::BTreeMap<u32, Vec<(usize, usize, usize)>> =
                Default::default();
            layer.each(|start, end, c| {
                if c == SOLID {
                    return;
                }
                let (mut p, end) = (start as usize, end as usize);
                while p < end {
                    let row_end = (p / size + 1) * size;
                    let e = end.min(row_end);
                    runs.entry(c)
                        .or_default()
                        .push((p / size, p % size, (e - 1) % size + 1));
                    p = e;
                }
            });
            for (c, list) in runs {
                for b in decompose(list) {
                    faces[c as usize].push(Opening {
                        axis,
                        at: if side == 1 { size as f64 } else { 0.0 },
                        inward: if side == 1 { -1.0 } else { 1.0 },
                        lo: [b[0] as f64, b[2] as f64],
                        hi: [b[1] as f64, b[3] as f64],
                        cell: c,
                    });
                }
            }
        }
        let mut rects: std::collections::BTreeMap<(u32, u32, usize, usize), Vec<_>> =
            Default::default();
        pairs(cell_of, |c, d, axis, k, r, _| {
            let list = rects.entry((c, d, axis, k)).or_default();
            for row in r[2]..r[3] {
                list.push((row, r[0], r[1]));
            }
        });
        let mut portals: Vec<Vec<(u32, Opening)>> = vec![Vec::new(); n];
        for ((c, d, axis, k), list) in rects {
            for b in decompose(list) {
                let o = |cell: u32, inward: f64| Opening {
                    axis,
                    at: k as f64,
                    inward,
                    lo: [b[0] as f64, b[2] as f64],
                    hi: [b[1] as f64, b[3] as f64],
                    cell,
                };
                portals[c as usize].push((d, o(c, -1.0)));
                portals[d as usize].push((c, o(d, 1.0)));
            }
        }
        for p in &mut portals {
            p.sort_by_key(|(d, _)| *d);
        }
        CellGraph { faces, portals }
    }

    /// Mode 1's cost of `group` (`merge_occludedOpeningCost`): the area of its
    /// openings (its cells' tile-face rectangles and portals to non-members) times the share of
    /// 200 segments between two openings that do not see each other through the group.
    fn group_cost(&self, member: &[bool], group: &[u32]) -> f64 {
        let mut rects: Vec<&Opening> = Vec::new();
        for &c in group {
            rects.extend(self.faces[c as usize].iter());
            for (d, o) in &self.portals[c as usize] {
                if !member[*d as usize] {
                    rects.push(o);
                }
            }
        }
        if rects.len() < 2 {
            return 0.0;
        }
        let cdf = cdf(&rects);
        let (mut hidden, mut visible) = (0u32, 0u32);
        for i in 1..=200usize {
            let ra = pick(&rects, &cdf, (i as f64 - 0.5) / 200.0);
            let rb = pick(&rects, &cdf, halton(i, 2));
            if std::ptr::eq(ra, rb) || ra.coplanar(rb) {
                continue;
            }
            let pa = ra.point(halton(i, 3), halton(i, 5));
            let pb = rb.point(halton(i, 7), halton(i, 11));
            if ra.faces(&pb) && rb.faces(&pa) && self.sees(member, ra.cell, rb.cell, &pa, &pb) {
                visible += 1;
            } else {
                hidden += 1;
            }
        }
        if hidden + visible == 0 {
            return 0.0;
        }
        cdf[cdf.len() - 1] * f64::from(hidden) / f64::from(hidden + visible)
    }

    /// Whether the segment `a`-`b` leads from cell `from` to cell `to` through member cells,
    /// crossing the portals it passes through (`merge_segmentVisibleInGroup`).
    fn sees(&self, member: &[bool], from: u32, to: u32, a: &[f64; 3], b: &[f64; 3]) -> bool {
        let mut seen = vec![from];
        let mut queue = vec![from];
        while let Some(c) = queue.pop() {
            if c == to {
                return true;
            }
            for (d, o) in &self.portals[c as usize] {
                if member[*d as usize] && !seen.contains(d) && o.crossed(a, b) {
                    seen.push(*d);
                    queue.push(*d);
                }
            }
        }
        false
    }

    /// Mode 2's cost of joining cells `c` and `d` (`cellMerge_pairCost`): the
    /// area of both cells' openings other than each other, times the share of 1,000 segments
    /// between them that miss their shared portals.
    fn pair_cost(&self, c: u32, d: u32) -> f64 {
        let side = |x: u32, y: u32| -> Vec<&Opening> {
            let mut r: Vec<&Opening> = self.faces[x as usize].iter().collect();
            r.extend(
                self.portals[x as usize]
                    .iter()
                    .filter(|(n, _)| *n != y)
                    .map(|(_, o)| o),
            );
            r
        };
        let (sa, sb) = (side(c, d), side(d, c));
        if sa.is_empty() || sb.is_empty() {
            return 0.0;
        }
        let shared: Vec<&Opening> = self.portals[c as usize]
            .iter()
            .filter(|(n, _)| *n == d)
            .map(|(_, o)| o)
            .collect();
        let (ca, cb) = (cdf(&sa), cdf(&sb));
        let (mut blocked, mut passes) = (0u32, 0u32);
        for i in 1..=1000usize {
            let ra = pick(&sa, &ca, (i as f64 - 0.5) / 1000.0);
            let rb = pick(&sb, &cb, halton(i, 2));
            if ra.coplanar(rb) {
                continue;
            }
            let pa = ra.point(halton(i, 3), halton(i, 5));
            let pb = rb.point(halton(i, 7), halton(i, 11));
            if shared.iter().any(|p| p.crossed(&pa, &pb)) {
                passes += 1;
            } else {
                blocked += 1;
            }
        }
        if blocked + passes == 0 {
            return 0.0;
        }
        (ca[ca.len() - 1] + cb[cb.len() - 1]) * f64::from(blocked) / f64::from(blocked + passes)
    }
}

/// Umbra's per-tile cell merging after the best-first merge (lever L2): `CellMerger` mode 1 then
/// mode 2 (`Builder_computeTileImpl`), threshold
/// `r` (Umbra's smallest occluder, SO). Both costs measure visibility from the cells' tile-face
/// rectangles and portals alone: a wall between two openings hides them from each other, so a
/// union around a wall costs its whole opening area and is refused however tall the tile.
///
/// * Mode 1 (`cellMerge_growBox`): each unassigned cell, in index order, seeds a
///   group with its box. Each round tries the six faces in order: the group becomes the
///   unassigned cells reached from the seed through portals inside the box or on that face
///   (`merge_selectCellsInBox`), and is kept, its box the union of its cells', when its cost is
///   at most `r²`. Rounds repeat until the group stops changing, at most 100.
/// * Mode 2: each unpaired cell of mode 1's result, in index order, joins its first unpaired
///   neighbour whose pair cost is under `r²`.
///
/// Cells are numbered in order of their first member, so the result is deterministic.
fn umbra_merge(cell_of: &mut Field<u32>, cells: &mut CellBoxes, voxel: f32, r: f32) {
    let n = cells.len();
    if n < 2 {
        return;
    }
    let r2 = (f64::from(r) / f64::from(voxel)).powi(2);
    let union = |a: CellBox, b: CellBox| -> CellBox {
        (
            [0, 1, 2].map(|k| a.0[k].min(b.0[k])),
            [0, 1, 2].map(|k| a.1[k].max(b.1[k])),
        )
    };
    // Mode 1.
    let graph = CellGraph::new(cell_of, n);
    let mut group_of = vec![u32::MAX; n];
    let mut ngroups = 0u32;
    let mut member = vec![false; n];
    let mut seen = vec![false; n];
    for seed in 0..n {
        if group_of[seed] != u32::MAX {
            continue;
        }
        let mut group = vec![seed as u32];
        let mut bx = cells[seed];
        for _ in 0..100 {
            let before = group.len();
            for face in 0..6 {
                let (axis, up) = (face / 2, face % 2 == 1);
                // Cells reached from the seed through portals inside the box, or on the face.
                let mut reached = vec![seed as u32];
                seen[seed] = true;
                let mut i = 0;
                while i < reached.len() {
                    let c = reached[i] as usize;
                    i += 1;
                    for (d, o) in &graph.portals[c] {
                        let du = *d as usize;
                        if seen[du] || group_of[du] != u32::MAX {
                            continue;
                        }
                        let a = o.axis;
                        let (lo, hi) = (bx.0[a] as f64, bx.1[a] as f64);
                        let on_face = a == axis && o.at == if up { hi } else { lo };
                        let inside = lo < o.at && o.at < hi;
                        let overlaps = (0..2).all(|k| {
                            let ax = (a + 1 + k) % 3;
                            o.lo[k] < bx.1[ax] as f64 && (bx.0[ax] as f64) < o.hi[k]
                        });
                        if overlaps && (inside || on_face) {
                            seen[du] = true;
                            reached.push(*d);
                        }
                    }
                }
                for &c in &reached {
                    seen[c as usize] = false;
                }
                if reached.len() == group.len() {
                    continue;
                }
                for &c in &reached {
                    member[c as usize] = true;
                }
                let cost = graph.group_cost(&member, &reached);
                for &c in &reached {
                    member[c as usize] = false;
                }
                if cost <= r2 {
                    bx = reached
                        .iter()
                        .fold(cells[seed], |b, &c| union(b, cells[c as usize]));
                    group = reached;
                }
            }
            if group.len() == before {
                break;
            }
        }
        for &c in &group {
            group_of[c as usize] = ngroups;
        }
        ngroups += 1;
    }
    let ng = ngroups as usize;
    let mut boxes: CellBoxes = vec![([usize::MAX; 3], [0; 3]); ng];
    for c in 0..n {
        let g = group_of[c] as usize;
        boxes[g] = union(boxes[g], cells[c]);
    }
    cell_of.map(|c| {
        if c == SOLID {
            SOLID
        } else {
            group_of[c as usize]
        }
    });
    // Mode 2, on mode 1's cells.
    let graph = CellGraph::new(cell_of, ng);
    let mut final_of = vec![u32::MAX; ng];
    let mut next = 0u32;
    let mut out: CellBoxes = Vec::new();
    for g in 0..ng {
        if final_of[g] != u32::MAX {
            continue;
        }
        final_of[g] = next;
        let mut bx = boxes[g];
        let mut last = u32::MAX;
        for (h, _) in &graph.portals[g] {
            let hu = *h as usize;
            if *h == last || final_of[hu] != u32::MAX {
                continue;
            }
            last = *h;
            if graph.pair_cost(g as u32, *h) < r2 {
                final_of[hu] = next;
                bx = union(bx, boxes[hu]);
                break;
            }
        }
        out.push(bx);
        next += 1;
    }
    cell_of.map(|c| {
        if c == SOLID {
            SOLID
        } else {
            final_of[c as usize]
        }
    });
    *cells = out;
}
