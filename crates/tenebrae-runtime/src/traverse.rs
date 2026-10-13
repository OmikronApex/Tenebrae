//! The portal query: which tiles stand in for the tome at this camera (level of detail), the
//! cells the camera starts in, the propagation of screen coverage through portals, and the
//! objects the final coverages show.
//!
//! The traversal is a worklist over cells whose coverage grew ("dirty" cells), processed tile
//! by tile from the front: a cell's portals that face the camera are rasterised into their
//! target cells' coverages, masked by the cell's own; a target that gains pixels becomes dirty.
//! Coverage only grows, so the loop terminates; a work bound guards pathological tomes
//! ([`Status::Budget`]).

use tenebrae_tome::Aabb;

use crate::camera::Camera;
use crate::coverage::{Pool, Rect};
use crate::decode::{PortalData, Runtime};
use crate::query::{Counters, Options, Status};

/// Cell visits allowed per cell of the tiles entered before the query gives up.
const VISITS_PER_CELL: u32 = 16;
/// Most cell slots a query holds.
const MAX_SLOTS: u32 = 1 << 16;
/// How many radii a portal rectangle grows by on each side.
const PORTAL_GROWTH: f32 = 1.75;

/// Distance from `p` to the box `b` (0 inside).
fn box_distance(p: [f32; 3], b: &Aabb) -> f32 {
    (0..3)
        .map(|k| {
            let d = if p[k] < b.min[k] {
                b.min[k] - p[k]
            } else if p[k] > b.max[k] {
                p[k] - b.max[k]
            } else {
                0.0
            };
            d * d
        })
        .sum::<f32>()
        .sqrt()
}

fn grown(b: &Aabb, by: f32) -> Aabb {
    Aabb {
        min: b.min.map(|v| v - by),
        max: b.max.map(|v| v + by),
    }
}

fn contains(b: &Aabb, p: [f32; 3]) -> bool {
    (0..3).all(|k| p[k] >= b.min[k] && p[k] <= b.max[k])
}

/// A portal's screen footprint in one query: its rectangle (`None`: behind the near plane or
/// off screen) and, per pixel row, the span it covers.
#[derive(Clone, Copy, Debug)]
struct PortalScreen {
    rect: Option<Rect>,
    /// The screen polygon (`n` vertices): its row spans are found when it is rasterised,
    /// only over the rows that matter then.
    n: u8,
    poly: [[f32; 2]; 8],
    /// The selected node and cell the portal leads into (`u32::MAX`: none).
    tnode: u32,
    tlocal: u32,
}

/// An object's screen rectangle in one query (`None`: outside the frustum).
#[derive(Clone, Copy, Debug, Default)]
struct ObjectScreen {
    stamp: u32,
    rect: Option<Rect>,
}

/// Scratch of the portal traversal, kept across queries.
#[derive(Clone, Debug, Default)]
pub(crate) struct Traversal {
    /// Selected top nodes, front to back.
    order: Vec<u32>,
    /// Per top node, its position in `order` + 1 (0: not selected).
    rank: Vec<u32>,
    /// Per cell of every tile (`Runtime::cell_offset`), its slot + 1 (0: none).
    slot_of: Vec<u32>,
    /// Per slot: `(node, local cell)`.
    slots: Vec<(u32, u32)>,
    /// Per slot: dirty.
    dirty: Vec<bool>,
    /// Per slot: the camera's distance to the cell's box.
    dist: Vec<f32>,
    /// The query's camera position.
    cam_pos: [f32; 3],
    /// Per slot: the rectangle of pixels added since its portals were last processed. Only
    /// those can newly pass through a portal, so a revisit looks at nothing else.
    fresh: Vec<Rect>,
    /// Per selected node (by rank): the dirty cells' slots, and whether it is queued.
    pending: Vec<Vec<u32>>,
    queued: Vec<bool>,
    /// Tiles with pending work, as a min-heap over their rank.
    heap: Vec<u32>,
    /// The rank of the tile being processed (`u32::MAX` between tiles).
    current: u32,
    /// Cells of tiles already passed that gained coverage: processed in a further sweep once
    /// the front-to-back pass is through, with all the coverage that pass gives them.
    feedback: Vec<u32>,
    pool: Pool,
    /// Per object: seen in this query.
    seen: Vec<bool>,
    /// Scratch polygon.
    poly: Vec<[f32; 3]>,
    poly2: Vec<[f32; 2]>,
    clipped: Vec<[f32; 3]>,
    touched: Vec<u32>,
    /// The query's stamp; cache entries made in this query carry it.
    stamp: u32,
    /// Per tile portal (`Runtime::portal_offset`), its entry in `screens` + 1 (0: none yet
    /// this query); `portal_touched` lists the entries set, for the reset. Small per-portal
    /// words keep the hot path's memory traffic low; the footprints themselves are dense.
    portal_ref: Vec<u32>,
    portal_touched: Vec<u32>,
    screens: Vec<PortalScreen>,
    /// Per object, its screen rectangle.
    object_cache: Vec<ObjectScreen>,
    /// Per top node, the stamp and the selected ancestor standing in for it (`u32::MAX`: none).
    anc_stamp: Vec<u32>,
    anc: Vec<u32>,
}

impl Traversal {
    /// The portal query. `out` receives the visible objects, ascending.
    pub(crate) fn run(
        &mut self,
        rt: &Runtime,
        camera: &Camera,
        opts: &Options,
        out: &mut Vec<u32>,
        counters: &mut Counters,
    ) -> Result<(), Status> {
        if camera.degenerate {
            return Err(Status::BadCamera);
        }
        let clock = std::time::Instant::now();
        let lap = |slot: usize, counters: &mut Counters| {
            counters.stage_ns[slot] =
                clock.elapsed().as_nanos() as u32 - counters.stage_ns[..slot].iter().sum::<u32>();
        };
        self.reset(rt);
        self.cam_pos = camera.position;
        let scale = if opts.accuracy > 0.0 {
            opts.accuracy.max(rt.base_distance)
        } else {
            rt.base_distance
        };
        // Nodes and objects are tested against the frustum of any eye within the radius.
        let wide = camera.grown(opts.radius);
        self.select(rt, &wide, opts.radius, scale);
        lap(0, counters);
        if !self.start(rt, camera, opts.radius) {
            return Err(Status::NoCell);
        }
        lap(1, counters);
        let mut visits = 0u32;
        let mut budget = 0u32;
        let mut entered = vec![false; self.order.len()];
        loop {
            // One front-to-back pass over the queued tiles.
            while let Some(r) = self.pop_tile() {
                let node = self.order[r as usize];
                let tile = rt.tile_of(node);
                if !entered[r as usize] {
                    entered[r as usize] = true;
                    counters.tiles += 1;
                    budget = budget.saturating_add(VISITS_PER_CELL * tile.cells.len() as u32);
                }
                // Nearest dirty cell first, chosen afresh after each cell: a cell dirtied by
                // a nearer one during this pass is processed after it, in this pass, so
                // chains through a tile resolve without revisits. The tile is not re-queued
                // meanwhile, and tiles already passed wait for the next sweep.
                self.queued[r as usize] = true;
                self.current = r;
                while let Some(slot) = self.pop_nearest(r) {
                    if !self.dirty[slot as usize] {
                        continue;
                    }
                    self.dirty[slot as usize] = false;
                    visits += 1;
                    if visits > budget {
                        return Err(Status::Budget);
                    }
                    counters.cells += 1;
                    self.process_cell(rt, camera, opts, slot, counters)?;
                }
                self.queued[r as usize] = false;
                self.current = u32::MAX;
            }
            if self.feedback.is_empty() {
                break;
            }
            // Cells of tiles already passed that gained coverage: queue them for a further
            // sweep, now that the pass gave them everything it had.
            let feedback = std::mem::take(&mut self.feedback);
            for &slot in &feedback {
                self.queue(slot);
            }
            self.feedback = feedback;
            self.feedback.clear();
        }
        lap(2, counters);
        self.output(rt, camera, &wide, opts.radius, out, counters);
        lap(3, counters);
        Ok(())
    }

    fn reset(&mut self, rt: &Runtime) {
        self.order.clear();
        self.rank.clear();
        self.rank.resize(rt.tiles.len(), 0);
        for &c in &self.touched {
            self.slot_of[c as usize] = 0;
        }
        self.touched.clear();
        self.slot_of.resize(rt.cell_total as usize, 0);
        self.slots.clear();
        self.dirty.clear();
        self.dist.clear();
        self.fresh.clear();
        self.pending.clear();
        self.queued.clear();
        self.heap.clear();
        self.current = u32::MAX;
        self.feedback.clear();
        self.pool.clear();
        self.seen.clear();
        self.seen.resize(rt.objects.len(), false);
        if self.portal_ref.len() != rt.portal_total as usize {
            self.portal_ref = vec![0; rt.portal_total as usize];
        }
        for &g in &self.portal_touched {
            self.portal_ref[g as usize] = 0;
        }
        self.portal_touched.clear();
        self.screens.clear();
        if self.object_cache.len() != rt.objects.len() {
            self.object_cache = vec![ObjectScreen::default(); rt.objects.len()];
        }
        if self.anc.len() != rt.tiles.len() {
            self.anc = vec![u32::MAX; rt.tiles.len()];
            self.anc_stamp = vec![0; rt.tiles.len()];
        }
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.object_cache
                .iter_mut()
                .for_each(|e| e.stamp = u32::MAX);
            self.anc_stamp.iter_mut().for_each(|s| *s = u32::MAX);
            self.stamp = 1;
        }
    }

    /// Level of detail: the nodes whose tiles stand in for the tome, front to back.
    fn select(&mut self, rt: &Runtime, camera: &Camera, radius: f32, scale: f32) {
        let mut stack = vec![0u32];
        while let Some(v) = stack.pop() {
            let b = &rt.node_boxes[v as usize];
            let tile = rt.tiles[v as usize].as_ref();
            let level = tile.map_or(0.0, |t| t.level);
            let g = grown(b, level + radius);
            let eye_inside = contains(&g, camera.position)
                || camera.near_corners.iter().any(|&c| contains(&g, c));
            if !eye_inside && !camera.box_in_frustum(g.min, g.max) {
                continue;
            }
            let [lo, hi] = rt.children[v as usize];
            let leaf = lo == u32::MAX;
            if tile.is_some() {
                let far = !leaf
                    && !eye_inside
                    && box_distance(camera.position, b) > rt.node_distances[v as usize] * scale;
                if leaf || far {
                    self.rank[v as usize] = self.order.len() as u32 + 1;
                    self.order.push(v);
                    continue;
                }
            }
            if leaf {
                continue;
            }
            // Nearer child first: it is popped first.
            let a = usize::from(rt.top.codes[v as usize].min(2));
            let split = rt
                .top
                .splits
                .get(v as usize)
                .copied()
                .unwrap_or((b.min[a] + b.max[a]) * 0.5);
            let (near, far_child) = if camera.position[a] < split {
                (lo, hi)
            } else {
                (hi, lo)
            };
            stack.push(far_child);
            stack.push(near);
        }
        self.pending.resize(self.order.len(), Vec::new());
        self.queued.resize(self.order.len(), false);
    }

    /// The cells the camera starts in; false when none.
    fn start(&mut self, rt: &Runtime, camera: &Camera, radius: f32) -> bool {
        // The position, a 3x3 grid over the near quad, and with a radius each of those moved
        // by the radius along each axis.
        let mut points = vec![camera.position];
        let q = camera.near_corners;
        for i in 0..3 {
            for j in 0..3 {
                let (s, t) = (i as f32 * 0.5, j as f32 * 0.5);
                points.push(std::array::from_fn(|k| {
                    let bottom = q[0][k] + (q[1][k] - q[0][k]) * s;
                    let top = q[3][k] + (q[2][k] - q[3][k]) * s;
                    bottom + (top - bottom) * t
                }));
            }
        }
        if radius > 0.0 {
            for c in points.clone() {
                for k in 0..3 {
                    for s in [-radius, radius] {
                        let mut p = c;
                        p[k] += s;
                        points.push(p);
                    }
                }
            }
        }
        let mut any = false;
        if radius > 0.0 {
            // Every cell whose box meets the near quad's box grown by the radius: an eye
            // anywhere within the radius may stand in it.
            let mut lo = camera.position;
            let mut hi = camera.position;
            for c in camera.near_corners {
                for k in 0..3 {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
            let reach = Aabb {
                min: lo.map(|v| v - radius),
                max: hi.map(|v| v + radius),
            };
            let meets =
                |a: &Aabb, b: &Aabb| (0..3).all(|k| a.min[k] <= b.max[k] && a.max[k] >= b.min[k]);
            for &node in &self.order.clone() {
                let tile = rt.tile_of(node);
                if !meets(&tile.bounds, &reach) {
                    continue;
                }
                for (local, cell) in tile.cells.iter().enumerate() {
                    if meets(&cell.bounds, &reach) {
                        self.activate(rt, node, local as u32, true);
                        any = true;
                    }
                }
            }
        }
        for p in points {
            let Some((node, local)) = rt.local_cell(p) else {
                continue;
            };
            let node = node as u32;
            if self.rank[node as usize] == 0 {
                // The camera's tile was not selected (a coarse ancestor stands in): start in
                // the coarse cell holding this leaf cell.
                let Some((anc, coarse)) = self.retarget(rt, node, local) else {
                    continue;
                };
                self.activate(rt, anc, coarse, true);
            } else {
                self.activate(rt, node, local, true);
            }
            any = true;
        }
        any
    }

    /// The selected ancestor of `node`, if any (cached per query).
    fn ancestor(&mut self, rt: &Runtime, node: u32) -> u32 {
        if self.anc_stamp[node as usize] == self.stamp {
            return self.anc[node as usize];
        }
        let mut v = rt.parent[node as usize];
        while v != u32::MAX && self.rank[v as usize] == 0 {
            v = rt.parent[v as usize];
        }
        self.anc_stamp[node as usize] = self.stamp;
        self.anc[node as usize] = v;
        v
    }

    /// The selected ancestor of `node` and its cell holding local cell `local` of `node`.
    fn retarget(&mut self, rt: &Runtime, node: u32, local: u32) -> Option<(u32, u32)> {
        let v = self.ancestor(rt, node);
        if v == u32::MAX {
            return None;
        }
        let cell = &rt.tile_of(node).cells[local as usize];
        let globals =
            &rt.cell_globals[cell.globals.0 as usize..(cell.globals.0 + cell.globals.1) as usize];
        let t = rt.tile_of(v);
        globals
            .iter()
            .find_map(|&g| t.local_of_global(g))
            .map(|l| (v, l))
    }

    /// The slot of `(node, local)`, made if needed (`full`: with full-screen coverage), marked
    /// dirty and queued. `None` when the slot pool is exhausted.
    fn activate(&mut self, rt: &Runtime, node: u32, local: u32, full: bool) -> Option<u32> {
        let idx = (rt.cell_offset[node as usize] + local) as usize;
        let slot = match self.slot_of[idx] {
            0 => {
                if self.slots.len() as u32 >= MAX_SLOTS {
                    return None;
                }
                let s = if full {
                    self.pool.push_full()
                } else {
                    self.pool.push_empty()
                };
                self.slots.push((node, local));
                self.dirty.push(false);
                self.dist.push(box_distance(
                    self.cam_pos,
                    &rt.tile_of(node).cells[local as usize].bounds,
                ));
                self.fresh.push(if full { Rect::FULL } else { Rect::EMPTY });
                self.slot_of[idx] = s + 1;
                self.touched.push(idx as u32);
                s
            }
            s => s - 1,
        };
        if full {
            // A start cell: full coverage whatever it had.
            self.pool.fill_full(slot);
            self.fresh[slot as usize] = Rect::FULL;
        }
        self.mark_dirty(slot);
        Some(slot)
    }

    fn mark_dirty(&mut self, slot: u32) {
        if self.dirty[slot as usize] {
            return;
        }
        self.dirty[slot as usize] = true;
        let node = self.slots[slot as usize].0;
        let r = self.rank[node as usize] - 1;
        if r < self.current {
            // A tile already passed in this sweep: wait for the next one.
            self.feedback.push(slot);
            return;
        }
        self.queue(slot);
    }

    /// Puts the dirty cell `slot` on its tile's pending list and queues the tile.
    fn queue(&mut self, slot: u32) {
        let node = self.slots[slot as usize].0;
        let r = self.rank[node as usize] - 1;
        self.pending[r as usize].push(slot);
        if !self.queued[r as usize] {
            self.queued[r as usize] = true;
            self.heap.push(r);
            let mut i = self.heap.len() - 1;
            while i > 0 {
                let p = (i - 1) / 2;
                if self.heap[p] <= self.heap[i] {
                    break;
                }
                self.heap.swap(p, i);
                i = p;
            }
        }
    }

    /// The pending cell of the tile at rank `r` nearest to the camera, removed from the list.
    fn pop_nearest(&mut self, r: u32) -> Option<u32> {
        let list = &mut self.pending[r as usize];
        if list.is_empty() {
            return None;
        }
        let mut best = 0;
        for i in 1..list.len() {
            if self.dist[list[i] as usize] < self.dist[list[best] as usize] {
                best = i;
            }
        }
        Some(list.swap_remove(best))
    }

    fn pop_tile(&mut self) -> Option<u32> {
        if self.heap.is_empty() {
            return None;
        }
        let r = self.heap.swap_remove(0);
        let mut i = 0;
        loop {
            let (l, rr) = (2 * i + 1, 2 * i + 2);
            let mut m = i;
            if l < self.heap.len() && self.heap[l] < self.heap[m] {
                m = l;
            }
            if rr < self.heap.len() && self.heap[rr] < self.heap[m] {
                m = rr;
            }
            if m == i {
                break;
            }
            self.heap.swap(m, i);
            i = m;
        }
        self.queued[r as usize] = false;
        Some(r)
    }

    fn gate_open(&self, rt: &Runtime, opts: &Options, p: &PortalData) -> bool {
        let Some((start, count)) = p.gate else {
            return true;
        };
        let Some(flags) = opts.gates else {
            return true;
        };
        rt.gate_index_list[start as usize..(start + count) as usize]
            .iter()
            .all(|&g| flags.get(g as usize).copied().unwrap_or(true))
    }

    /// Rasterises every portal of the cell in `slot` into its target.
    fn process_cell(
        &mut self,
        rt: &Runtime,
        camera: &Camera,
        opts: &Options,
        slot: u32,
        counters: &mut Counters,
    ) -> Result<(), Status> {
        let (node, local) = self.slots[slot as usize];
        let tile = rt.tile_of(node);
        let cell = tile.cells[local as usize];
        let fresh = std::mem::replace(&mut self.fresh[slot as usize], Rect::EMPTY);
        for pi in cell.portals.0..cell.portals.0 + cell.portals.1 {
            let p = tile.portals[pi as usize];
            counters.portals += 1;
            if p.boundary || !self.gate_open(rt, opts, &p) {
                counters.rejected[0] += 1;
                continue;
            }
            // A straight sight line crosses the portal's plane at most once, from the eye's
            // side to the other: with the eye (anywhere within the radius) on the neighbour's
            // side, nothing leaves this cell through the portal.
            let axis = p.axis as usize;
            let toward = if p.side == 1 { 1.0 } else { -1.0 };
            if (camera.position[axis] - p.plane) * toward > opts.radius {
                counters.rejected[0] += 1;
                continue;
            }
            // The portal's target (the selected node standing in for it) and screen
            // footprint, once per query.
            let g = (rt.portal_offset[node as usize] + pi) as usize;
            let si = match self.portal_ref[g] {
                0 => {
                    let target = if self.rank[p.target as usize] != 0 {
                        Some((p.target, p.local))
                    } else {
                        self.retarget(rt, p.target, p.local)
                    };
                    let rect = target.and_then(|_| self.portal_polygon(camera, &p, opts.radius));
                    let mut e = PortalScreen {
                        rect,
                        n: self.poly2.len().min(8) as u8,
                        poly: [[0.0; 2]; 8],
                        tnode: target.map_or(u32::MAX, |t| t.0),
                        tlocal: target.map_or(u32::MAX, |t| t.1),
                    };
                    for (k, v) in self.poly2.iter().take(8).enumerate() {
                        e.poly[k] = *v;
                    }
                    self.screens.push(e);
                    self.portal_ref[g] = self.screens.len() as u32;
                    self.portal_touched.push(g as u32);
                    self.screens.len() - 1
                }
                i => i as usize - 1,
            };
            let (rect, tnode, tlocal) = {
                let e = &self.screens[si];
                (e.rect, e.tnode, e.tlocal)
            };
            if tnode == u32::MAX || (tnode == node && tlocal == local) {
                counters.rejected[1] += 1;
                continue;
            }
            let Some(r) = rect else {
                counters.rejected[2] += 1;
                continue;
            };
            // Only the pixels added since the last visit can newly pass through.
            let r = r.intersect(&fresh);
            if r.is_empty() {
                counters.rejected[3] += 1;
                continue;
            }
            if !self.pool.any_in(slot, r) {
                counters.rejected[4] += 1;
                continue;
            }
            let Some(target) = self.activate_plain(rt, tnode, tlocal) else {
                return Err(Status::Budget);
            };
            if self.pool.covers(target, slot, r) {
                // Nothing the source shows through this rectangle is new to the target.
                counters.rejected[5] += 1;
                continue;
            }
            counters.rasterised += 1;
            counters.raster_rows += (r.y1 - r.y0) as u32;
            let e = &self.screens[si];
            let added = self
                .pool
                .rasterise(target, slot, &e.poly[..e.n as usize], r);
            if !added.is_empty() {
                self.fresh[target as usize] = self.fresh[target as usize].union(&added);
                self.mark_dirty(target);
            }
        }
        Ok(())
    }

    /// A slot for `(node, local)` with empty coverage if new, not marked dirty.
    fn activate_plain(&mut self, rt: &Runtime, node: u32, local: u32) -> Option<u32> {
        let idx = (rt.cell_offset[node as usize] + local) as usize;
        match self.slot_of[idx] {
            0 => {
                if self.slots.len() as u32 >= MAX_SLOTS {
                    return None;
                }
                let s = self.pool.push_empty();
                self.slots.push((node, local));
                self.dirty.push(false);
                self.dist.push(box_distance(
                    self.cam_pos,
                    &rt.tile_of(node).cells[local as usize].bounds,
                ));
                self.fresh.push(Rect::EMPTY);
                self.slot_of[idx] = s + 1;
                self.touched.push(idx as u32);
                Some(s)
            }
            s => Some(s - 1),
        }
    }

    /// The portal's rectangle, grown by the collision `radius` on its two in-plane axes (the
    /// answer must hold for every eye within the radius of the position, and an eye moved by
    /// `r` sees through an opening as if it were `r` wider), as a screen polygon (in
    /// `self.poly2`) and its pixel bounds; `None` when it is wholly behind the near plane or
    /// off screen.
    fn portal_polygon(&mut self, camera: &Camera, p: &PortalData, radius: f32) -> Option<Rect> {
        let axis = p.axis as usize;
        let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
        let grow = radius * PORTAL_GROWTH;
        let rect = [
            p.rect[0] - grow,
            p.rect[1] + grow,
            p.rect[2] - grow,
            p.rect[3] + grow,
        ];
        self.poly.clear();
        for (u, v) in [
            (rect[0], rect[2]),
            (rect[1], rect[2]),
            (rect[1], rect[3]),
            (rect[0], rect[3]),
        ] {
            let mut q = [0.0f32; 3];
            q[axis] = p.plane;
            q[a1] = u;
            q[a2] = v;
            self.poly.push(q);
        }
        self.clip_near(camera);
        if self.clipped.len() < 3 {
            return None;
        }

        self.poly2.clear();
        let (mut nx0, mut ny0, mut nx1, mut ny1) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for &q in &self.clipped {
            let [x, y, w] = camera.project(q);
            if w <= 0.0 {
                // Numerically on the near plane after clipping: the whole screen.
                self.poly2.clear();
                self.poly2
                    .extend_from_slice(&[[0.0, 0.0], [64.0, 0.0], [64.0, 64.0], [0.0, 64.0]]);
                return Some(Rect::FULL);
            }
            let (nx, ny) = (x / w, y / w);
            nx0 = nx0.min(nx);
            ny0 = ny0.min(ny);
            nx1 = nx1.max(nx);
            ny1 = ny1.max(ny);
            self.poly2.push([(nx + 1.0) * 32.0, (ny + 1.0) * 32.0]);
        }
        let r = Rect::from_ndc(nx0, ny0, nx1, ny1);
        // One pixel more each way: the rasteriser's dilation needs the room.
        let r = Rect {
            x0: (r.x0 - 1).max(0),
            y0: (r.y0 - 1).max(0),
            x1: (r.x1 + 1).min(64),
            y1: (r.y1 + 1).min(64),
        };
        (!r.is_empty()).then_some(r)
    }

    /// Clips `self.poly` against the near plane into `self.clipped`.
    fn clip_near(&mut self, camera: &Camera) {
        let n = camera.planes[0];
        let d = |q: [f32; 3]| n[0] * q[0] + n[1] * q[1] + n[2] * q[2] + n[3];
        self.clipped.clear();
        let m = self.poly.len();
        for i in 0..m {
            let (a, b) = (self.poly[i], self.poly[(i + 1) % m]);
            let (da, db) = (d(a), d(b));
            if da >= 0.0 {
                self.clipped.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                self.clipped
                    .push(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t));
            }
        }
    }

    /// The objects the final coverages show: in the frustum of `wide` (the camera grown by the
    /// radius) and covered on the screen of `camera`.
    fn output(
        &mut self,
        rt: &Runtime,
        camera: &Camera,
        wide: &Camera,
        radius: f32,
        out: &mut Vec<u32>,
        counters: &mut Counters,
    ) {
        for slot in 0..self.slots.len() as u32 {
            let (node, local) = self.slots[slot as usize];
            let cell = rt.tile_of(node).cells[local as usize];
            for &o in &rt.object_lists
                [cell.objects.0 as usize..(cell.objects.0 + cell.objects.1) as usize]
            {
                if self.seen[o as usize] {
                    continue;
                }
                counters.candidates += 1;
                // The object's screen rectangle, once per query: `None` outside the frustum
                // of any eye within the radius; the box grown by the radius, since an eye
                // moved by it sees the object that much larger on screen.
                if self.object_cache[o as usize].stamp != self.stamp {
                    let b = &rt.objects[o as usize];
                    let rect = wide
                        .box_in_frustum(b.min, b.max)
                        .then(|| self.box_rect(camera, &grown(b, radius)));
                    self.object_cache[o as usize] = ObjectScreen {
                        stamp: self.stamp,
                        rect,
                    };
                }
                let Some(r) = self.object_cache[o as usize].rect else {
                    continue;
                };
                if self.pool.any_in(slot, r) {
                    self.seen[o as usize] = true;
                    out.push(o);
                }
            }
        }
        out.sort_unstable();
        counters.output = out.len() as u32;
    }

    /// The pixels a box projects to, conservatively; the whole screen when a corner lies
    /// behind the near plane.
    fn box_rect(&self, camera: &Camera, b: &Aabb) -> Rect {
        let n = camera.planes[0];
        let (mut nx0, mut ny0, mut nx1, mut ny1) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for k in 0..8 {
            let q = [
                if k & 1 == 0 { b.min[0] } else { b.max[0] },
                if k & 2 == 0 { b.min[1] } else { b.max[1] },
                if k & 4 == 0 { b.min[2] } else { b.max[2] },
            ];
            if n[0] * q[0] + n[1] * q[1] + n[2] * q[2] + n[3] < 0.0 {
                return Rect::FULL;
            }
            let [x, y, w] = camera.project(q);
            if w <= 0.0 {
                return Rect::FULL;
            }
            nx0 = nx0.min(x / w);
            ny0 = ny0.min(y / w);
            nx1 = nx1.max(x / w);
            ny1 = ny1.max(y / w);
        }
        // The pixel the maximum edge lands on counts as touched too: an object whose edge
        // sits exactly on a pixel boundary is seen through either pixel.
        let r = Rect::from_ndc(nx0, ny0, nx1, ny1);
        Rect {
            x1: (r.x1 + 1).min(64),
            y1: (r.y1 + 1).min(64),
            ..r
        }
    }

    /// Diagnostics: the cells reached, as `(top node, local cell)`, with their coverage words.
    pub(crate) fn reached(&self) -> impl Iterator<Item = ((u32, u32), &[u32])> {
        self.slots
            .iter()
            .enumerate()
            .map(|(s, &cell)| (cell, self.pool.words_of(s as u32)))
    }

    /// Diagnostics: [`Traversal::box_rect`] for the caller.
    pub(crate) fn rect_of(&self, camera: &Camera, b: &Aabb) -> Rect {
        self.box_rect(camera, b)
    }
}
