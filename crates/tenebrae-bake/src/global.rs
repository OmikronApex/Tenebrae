//! The serial global stage: joining the flood across tiles, cross-tile faces, object lists,
//! numbering and the tome.

use std::collections::HashMap;

use super::grid::{SOLID, UnionFind, coalesce, coalesce_runs, run_rects, zip};
use super::tile::{
    BOUNDARY, Ctx, FLEX, Flood, LABEL, Node, TileOut, is_midpoint_tree, midpoint, tile_frame,
};
use super::{CELL_SAMPLES, Report};
use tenebrae_geom::Vec3;
use tenebrae_tome::{
    Aabb, Gates, GlobalCell, Header, KdTree, PARAMS_SIZE, Portal, Run, RunList, Tile, TileCell,
    Tome, ViewTree,
};

/// Joins pass A's per-tile flood components across tile faces. A component is live when it holds
/// a non-surface start voxel. Returns, per tile, whether each label is live.
pub(super) fn join(ctx: &Ctx<'_>, floods: &[Flood]) -> Vec<Vec<bool>> {
    let mut base = Vec::with_capacity(floods.len());
    let mut total = 0usize;
    for f in floods {
        base.push(total as u32);
        total += f.labels;
    }
    let mut uf = UnionFind::new(total);
    for (t, f) in floods.iter().enumerate() {
        for axis in 0..3 {
            let Some(n) = ctx.neighbour(t, axis, 1) else {
                continue;
            };
            zip(
                &f.layers[axis * 2 + 1],
                &floods[n].layers[axis * 2],
                |_, _, a, b| {
                    let (la, lb) = (a & LABEL, b & LABEL);
                    if a >> 31 == 0 && la != LABEL && lb != LABEL {
                        uf.union(base[t] + la, base[n] + lb);
                    }
                },
            );
        }
    }
    let mut live = vec![false; total];
    for (t, f) in floods.iter().enumerate() {
        for &l in &f.seeded {
            let r = uf.find(base[t] + l);
            live[r as usize] = true;
        }
    }
    floods
        .iter()
        .enumerate()
        .map(|(t, f)| {
            (0..f.labels as u32)
                .map(|l| live[uf.find(base[t] + l) as usize])
                .collect()
        })
        .collect()
}

/// A node of the top tree.
#[derive(Clone, Copy, Debug)]
pub(super) enum Top {
    Inner { axis: u8, at: f32, kids: [usize; 2] },
    Leaf(usize),
}

/// The top KD tree over the tile grid, split at integer tile boundaries down to single tiles, and
/// the tile-grid cell of each leaf in level order.
pub(super) fn top_tree(
    tiles_n: [usize; 3],
    origin: Vec3,
    tile: f32,
) -> (Vec<Top>, Vec<[[usize; 2]; 3]>) {
    let mut nodes = vec![Top::Leaf(0)];
    let mut ranges = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((
        0usize,
        [[0usize, tiles_n[0]], [0, tiles_n[1]], [0, tiles_n[2]]],
    ));
    while let Some((i, r)) = queue.pop_front() {
        let ext = [r[0][1] - r[0][0], r[1][1] - r[1][0], r[2][1] - r[2][0]];
        let axis = (0..3).max_by_key(|&a| (ext[a], 3 - a)).unwrap_or(0);
        if ext[axis] <= 1 {
            nodes[i] = Top::Leaf(ranges.len());
            ranges.push(r);
            continue;
        }
        let at = r[axis][0] + ext[axis] / 2;
        let (mut lo, mut hi) = (r, r);
        lo[axis][1] = at;
        hi[axis][0] = at;
        let k = nodes.len();
        nodes[i] = Top::Inner {
            axis: axis as u8,
            at: origin[axis] + at as f32 * tile,
            kids: [k, k + 1],
        };
        nodes.push(Top::Leaf(0));
        nodes.push(Top::Leaf(0));
        queue.push_back((k, lo));
        queue.push_back((k + 1, hi));
    }
    (nodes, ranges)
}

/// One portal-to-be of a global cell, toward cell `nb` (a cell id) or [`BOUNDARY`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Link {
    pub(super) axis: u8,
    pub(super) side: u8,
    pub(super) nb: u32,
    pub(super) k: u32,
    pub(super) rect: [u32; 4],
}

/// A cell in the global stage. `uid` is its index in the first numbering; later steps remove and
/// merge cells, and links name cells by uid.
#[derive(Clone, Debug)]
pub(super) struct GCell {
    pub(super) uid: u32,
    pub(super) lo: [usize; 3],
    pub(super) hi: [usize; 3],
    /// Open voxels.
    pub(super) voxels: u64,
    pub(super) samples: Vec<Vec3>,
    pub(super) links: Vec<Link>,
    /// Targets (input indices until objects are renumbered), sorted.
    pub(super) objects: Vec<u32>,
}

/// A leaf tile in the global stage.
#[derive(Clone, Debug)]
pub(super) struct GTile {
    /// Tile-grid range.
    pub(super) range: [[usize; 2]; 3],
    pub(super) cells: Vec<GCell>,
    pub(super) tree: Vec<Node>,
    /// [`TileOut::choices`] until [`reachability`] gives every `FLEX` leaf its cell; empty after.
    pub(super) choices: Vec<Vec<u32>>,
    /// Lever L4's fallback: a collapsed tile whose view tree is not a midpoint tree over its box
    /// stores the level-order prefix of its splits that ends at its last off-midpoint split
    /// ([`trim_splits`]). Every other tile is split-free.
    pub(super) explicit: bool,
    /// The level shift it was baked at (`Ctx::level_shift`); a tile made of several, the least of
    /// theirs.
    pub(super) shift: u32,
}

/// The global stage's state.
pub(super) struct Scene {
    pub(super) top: Vec<Top>,
    /// Indexed by `Top::Leaf`; `None` once merged away.
    pub(super) tiles: Vec<Option<GTile>>,
    /// Merged uids point at the uid that absorbed them.
    pub(super) alias: Vec<u32>,
    /// Per target: listed in any cell before reachability.
    pub(super) touched: Vec<bool>,
}

impl Scene {
    pub(super) fn resolve(&self, mut uid: u32) -> u32 {
        while self.alias[uid as usize] != uid {
            uid = self.alias[uid as usize];
        }
        uid
    }
}

/// Builds the global cells, their links and object lists from the tiles' results.
pub(super) fn assemble(ctx: &Ctx<'_>, top: Vec<Top>, outs: Vec<TileOut>) -> Scene {
    let size = ctx.layout.tile() as u32;
    let mut base = Vec::with_capacity(outs.len());
    let mut total = 0u32;
    for o in &outs {
        base.push(total);
        total += o.cells.len() as u32;
    }
    // Cross-tile faces: pairs of open voxels on either side of a shared tile plane.
    let seam = ctx.portal_cost(true);
    let mut cross: Vec<Vec<Link>> = vec![Vec::new(); total as usize];
    for t in 0..outs.len() {
        let lo = ctx.lo(t);
        for axis in 0..3 {
            let Some(n) = ctx.neighbour(t, axis, 1) else {
                continue;
            };
            let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut rects: HashMap<(u32, u32), Vec<[u32; 4]>> = HashMap::new();
            zip(
                &outs[t].layers[axis * 2 + 1],
                &outs[n].layers[axis * 2],
                |s, e, c, d| {
                    if c == SOLID || d == SOLID {
                        return;
                    }
                    let list = rects.entry((base[t] + c, base[n] + d)).or_default();
                    for r in run_rects(s, e, size) {
                        list.push([
                            r[0] + lo[a1] as u32,
                            r[1] + lo[a1] as u32,
                            r[2] + lo[a2] as u32,
                            r[3] + lo[a2] as u32,
                        ]);
                    }
                },
            );
            let mut rects: Vec<_> = rects.into_iter().collect();
            rects.sort_unstable_by_key(|(k, _)| *k);
            let rects = rects.into_iter().flat_map(|(key, mut list)| {
                list.sort_unstable();
                coalesce(&list, seam).into_iter().map(move |r| (key, r))
            });
            let k = (lo[axis] + size as usize) as u32;
            for ((c, d), rect) in rects {
                cross[c as usize].push(Link {
                    axis: axis as u8,
                    side: 1,
                    nb: d,
                    k,
                    rect,
                });
                cross[d as usize].push(Link {
                    axis: axis as u8,
                    side: 0,
                    nb: c,
                    k,
                    rect,
                });
            }
        }
    }
    let ntargets = ctx.input.targets.len();
    let mut touched = vec![false; ntargets];
    let mut tiles = Vec::with_capacity(outs.len());
    for (t, out) in outs.into_iter().enumerate() {
        let mut cells: Vec<GCell> = out
            .cells
            .into_iter()
            .enumerate()
            .map(|(i, c)| GCell {
                uid: base[t] + i as u32,
                lo: c.lo,
                hi: c.hi,
                voxels: c.voxels,
                samples: c.samples,
                links: Vec::new(),
                objects: Vec::new(),
            })
            .collect();
        for f in &out.faces {
            cells[f.c as usize].links.push(Link {
                axis: f.axis,
                side: f.side,
                nb: if f.nb == BOUNDARY {
                    BOUNDARY
                } else {
                    base[t] + f.nb
                },
                k: f.k,
                rect: f.rect,
            });
        }
        for c in &mut cells {
            c.links.append(&mut cross[c.uid as usize]);
            c.links
                .sort_unstable_by_key(|l| (l.axis, l.side, l.nb, l.k));
        }
        for &(c, o) in &out.objects {
            cells[c as usize].objects.push(o);
            touched[o as usize] = true;
        }
        tiles.push(Some(GTile {
            range: ctx.tile_ranges[t],
            cells,
            tree: out.tree,
            choices: out.choices,
            explicit: false,
            shift: ctx.level_shift(t),
        }));
    }
    // The part of a target beyond a domain face: any sight line to it leaves the domain through
    // that face, so it is listed in every cell with a boundary portal there.
    let domain = &ctx.domain;
    let mut beyond: [Vec<u32>; 6] = Default::default();
    for (o, t) in ctx.input.targets.iter().enumerate() {
        for axis in 0..3 {
            if t.bounds.min[axis] < domain.min[axis] {
                beyond[axis * 2].push(o as u32);
            }
            if t.bounds.max[axis] > domain.max[axis] {
                beyond[axis * 2 + 1].push(o as u32);
            }
        }
    }
    for tile in tiles.iter_mut().flatten() {
        for c in &mut tile.cells {
            let mut faces = [false; 6];
            for l in &c.links {
                if l.nb == BOUNDARY {
                    faces[l.axis as usize * 2 + l.side as usize] = true;
                }
            }
            for f in 0..6 {
                if faces[f] {
                    c.objects.extend_from_slice(&beyond[f]);
                    for &o in &beyond[f] {
                        touched[o as usize] = true;
                    }
                }
            }
            c.objects.sort_unstable();
            c.objects.dedup();
        }
    }
    Scene {
        top,
        tiles,
        alias: (0..total).collect(),
        touched,
    }
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

fn bits_for(max_value: u32) -> u32 {
    32 - max_value.leading_zeros()
}

/// Level-order codes, splits and leaf values of a view tree. With `midpoint` (lever L4) every
/// split is the implicit midpoint and no split is stored.
fn emit_tree(nodes: &[Node], solid_value: u32, midpoint: bool) -> (KdTree, Vec<u32>) {
    let mut codes = Vec::new();
    let mut splits = Vec::new();
    let mut leaves = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(0u32);
    while let Some(i) = queue.pop_front() {
        match nodes[i as usize] {
            Node::Leaf(v) => {
                codes.push(3);
                if !midpoint {
                    splits.push(0.0);
                }
                leaves.push(if v == SOLID { solid_value } else { v });
            }
            Node::Split { axis, at, kids } => {
                codes.push(axis);
                if !midpoint {
                    splits.push(at);
                }
                queue.push_back(kids[0]);
                queue.push_back(kids[1]);
            }
        }
    }
    (KdTree { codes, splits }, leaves)
}

/// Lever L4's fallback in the format's own form: the stored splits are a level-order prefix and
/// every node past it splits at its implicit midpoint, so the prefix ends at the last inner node
/// whose split is not, bit for bit, the runtime's f32 midpoint of its box (the boxes descending
/// from `frame` through the stored splits). In a collapsed tile that is the seam, or a few seams
/// after cascaded collapses: the trees below were midpoint trees over their own boxes.
fn trim_splits(tree: &mut KdTree, frame: Aabb) {
    let n = tree.codes.len();
    let mut boxes = vec![frame; n];
    let mut inner = 0usize;
    let mut keep = 0usize;
    for v in 0..n {
        let code = tree.codes[v];
        if code == 3 {
            continue;
        }
        inner += 1;
        let a = usize::from(code);
        let b = boxes[v];
        let at = tree.splits[v];
        if at.to_bits() != midpoint(&b, a).to_bits() {
            keep = v + 1;
        }
        let (below, above) = (2 * inner - 1, 2 * inner);
        if above < n {
            boxes[below] = b;
            boxes[below].max[a] = at;
            boxes[above] = b;
            boxes[above].min[a] = at;
        }
    }
    tree.splits.truncate(keep);
}

/// Packs cell object lists into one run stream, sharing windows (`Params::share_lists`): lists
/// go longest first (ties in cell order); a list whose entries already appear in the stream, in
/// order, takes that window, and otherwise it is appended, reusing as much of the stream's tail
/// as matches its head. Returns each list's `(node, record, start entry)`. Empty lists start at
/// 0. Deterministic: the order depends on the lists alone.
pub(super) fn pack_lists(
    stream: &mut Vec<Run>,
    lists: Vec<(usize, usize, Vec<Run>)>,
) -> Vec<(usize, usize, u32)> {
    let mut order: Vec<usize> = (0..lists.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(lists[i].2.len()), i));
    let key = |r: &Run| (r.index, r.run);
    let mut at_entry: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    let mut out = Vec::with_capacity(lists.len());
    for i in order {
        let (node, rec, runs) = &lists[i];
        let Some(first) = runs.first() else {
            out.push((*node, *rec, 0));
            continue;
        };
        let found = at_entry.get(&key(first)).and_then(|starts| {
            starts
                .iter()
                .copied()
                .find(|&s| stream.get(s..s + runs.len()) == Some(&runs[..]))
        });
        let start = match found {
            Some(s) => s,
            None => {
                let tail = (1..runs.len().min(stream.len() + 1))
                    .rev()
                    .find(|&k| stream[stream.len() - k..] == runs[..k])
                    .unwrap_or(0);
                let s = stream.len() - tail;
                for r in &runs[tail..] {
                    at_entry.entry(key(r)).or_default().push(stream.len());
                    stream.push(*r);
                }
                s
            }
        };
        out.push((*node, *rec, start as u32));
    }
    out
}

/// A link's key: axis, side, neighbour, plane.
type LinkKey = (u8, u8, u32, u32);

/// A link by final ids: axis, side, neighbour, plane, rect.
type FinalLink = (u8, u8, u32, u32, [u32; 4]);

/// Numbers the scene's cells and objects and writes the tome.
pub(super) fn emit(ctx: &Ctx<'_>, scene: Scene, map: &[Option<u32>], report: &mut Report) -> Tome {
    let p = &ctx.input.params;
    let g = &ctx.grid;
    let domain = ctx.domain;
    // Leaf tiles in level order, and the top tree.
    let mut codes = Vec::new();
    let mut splits = Vec::new();
    let mut node_distances = Vec::new();
    let mut order: Vec<(usize, usize)> = Vec::new(); // (top node, tile)
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(0usize);
    while let Some(i) = queue.pop_front() {
        match scene.top[i] {
            Top::Inner { axis, at, kids } => {
                codes.push(axis);
                splits.push(at);
                node_distances.push(-1.0);
                queue.push_back(kids[0]);
                queue.push_back(kids[1]);
            }
            Top::Leaf(t) => {
                let node = codes.len();
                codes.push(3);
                splits.push(0.0);
                let r = &scene.tiles[t]
                    .as_ref()
                    .unwrap_or_else(|| unreachable!())
                    .range;
                let span = (0..3).map(|a| r[a][1] - r[a][0]).max().unwrap_or(1);
                node_distances.push(span as f32 * p.tile / p.base_distance);
                order.push((node, t));
            }
        }
    }
    node_distances.push(-1.0);
    let top = KdTree { codes, splits };
    // Final cell numbering: tiles in level order, cells in tile order.
    let mut gid = vec![u32::MAX; scene.alias.len()];
    let mut place: Vec<(usize, u32)> = Vec::new(); // (top node, local) per gid
    for &(node, t) in &order {
        let tile = scene.tiles[t].as_ref().unwrap_or_else(|| unreachable!());
        for (i, c) in tile.cells.iter().enumerate() {
            gid[c.uid as usize] = place.len() as u32;
            place.push((node, i as u32));
        }
    }
    let ncells = place.len();
    // A link's final neighbour: the boundary, a cell id, or `None` for a removed cell.
    let resolve = |uid: u32| {
        if uid == BOUNDARY {
            return Some(BOUNDARY);
        }
        let g = gid[scene.resolve(uid) as usize];
        (g != u32::MAX).then_some(g)
    };
    let world_box = |lo: [usize; 3], hi: [usize; 3]| Aabb {
        min: [g.coord(0, lo[0]), g.coord(1, lo[1]), g.coord(2, lo[2])],
        max: [g.coord(0, hi[0]), g.coord(1, hi[1]), g.coord(2, hi[2])],
    };
    let ntargets = ctx.input.targets.len();
    // Object runs.
    let max_run = 31u32;
    let survivors: Vec<usize> = (0..ntargets).filter(|&o| map[o].is_some()).collect();
    let index_bits = bits_for(survivors.len().saturating_sub(1) as u32).max(1);
    let mut object_runs = RunList {
        index_bits,
        run_bits: bits_for(max_run),
        entries: Vec::new(),
    };
    // With `share_lists`: per cell, its tile's node, its record and its run entries.
    let mut lists: Vec<(usize, usize, Vec<Run>)> = Vec::new();
    // Global cells: one per cell, or (lever L3) one per connected group of a tile's cells over
    // in-tile links, numbered by first cell.
    let mut group_of: Vec<u32> = (0..ncells as u32).collect();
    let mut ngroups = ncells;
    if p.global_groups {
        let mut uf = UnionFind::new(ncells);
        for &(node, t) in &order {
            let tile = scene.tiles[t].as_ref().unwrap_or_else(|| unreachable!());
            for c in &tile.cells {
                let own = gid[c.uid as usize];
                for l in &c.links {
                    if let Some(nb) = resolve(l.nb)
                        && nb != BOUNDARY
                        && place[nb as usize].0 == node
                    {
                        uf.union(own, nb);
                    }
                }
            }
        }
        let mut id = vec![u32::MAX; ncells];
        let mut next = 0u32;
        for (c, group) in group_of.iter_mut().enumerate() {
            let r = uf.find(c as u32) as usize;
            if id[r] == u32::MAX {
                id[r] = next;
                next += 1;
            }
            *group = id[r];
        }
        ngroups = next as usize;
    }
    // Per group: voxel box, links (by group), samples.
    let mut group_box: Vec<([usize; 3], [usize; 3])> = vec![([usize::MAX; 3], [0; 3]); ngroups];
    let mut group_links: Vec<Vec<FinalLink>> = vec![Vec::new(); ngroups];
    let mut group_samples: Vec<Vec<(Vec<Vec3>, u64)>> = vec![Vec::new(); ngroups];
    let mut tiles: Vec<Option<Tile>> = vec![None; top.codes.len()];
    let mut global_cells = Vec::with_capacity(ngroups);
    let mut global_portals = Vec::new();
    report.cell_samples = Vec::with_capacity(ncells);
    for &(node, t) in &order {
        let tile = scene.tiles[t].as_ref().unwrap_or_else(|| unreachable!());
        let frame = tile_frame(g, p.tile, &tile.range);
        let mut tile_portals = Vec::new();
        let mut records = Vec::new();
        for c in &tile.cells {
            let own = gid[c.uid as usize];
            // Links by final ids: drop links into the cell itself, merge equal keys.
            let mut links: Vec<(u8, u8, u32, u32, [u32; 4])> = c
                .links
                .iter()
                .filter_map(|l| {
                    let nb = resolve(l.nb)?;
                    (nb != own).then_some((l.axis, l.side, nb, l.k, l.rect))
                })
                .collect();
            let mut keyed: Vec<(LinkKey, [u32; 4])> =
                links.iter().map(|l| ((l.0, l.1, l.2, l.3), l.4)).collect();
            keyed.sort_unstable();
            coalesce_runs(&mut keyed, ctx.portal_cost(false));
            links = keyed
                .into_iter()
                .map(|((a, s, n, k), r)| (a, s, n, k, r))
                .collect();
            let start = tile_portals.len() as u32;
            let gstart = global_portals.len() as u32;
            let world = |axis: usize, k: u32, raw: [u32; 4]| {
                let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
                let plane = g.coord(axis, k as usize);
                let rect = [
                    g.coord(a1, raw[0] as usize),
                    g.coord(a1, raw[1] as usize),
                    g.coord(a2, raw[2] as usize),
                    g.coord(a2, raw[3] as usize),
                ];
                (plane, rect)
            };
            // Portals within the tile first: the runtime's cell sort reads a
            // cell's portals only up to the first one that leaves the tile.
            let mut tile_order: Vec<_> = links.iter().collect();
            tile_order.sort_by_key(|l| !(l.2 != BOUNDARY && place[l.2 as usize].0 == node));
            for &&(axis, side, nb, k, raw) in &tile_order {
                let axis = axis as usize;
                let (plane, rect) = world(axis, k, raw);
                let (target, local, boundary) = if nb == BOUNDARY {
                    (0x3FF_FFFF, 0, true)
                } else {
                    let (n_node, n_local) = place[nb as usize];
                    (n_node as u32, n_local, false)
                };
                tile_portals.push(portal(
                    &PortalSpec {
                        axis,
                        side: u32::from(side),
                        boundary,
                        target,
                        local,
                        plane,
                        rect,
                    },
                    &frame,
                ));
            }
            for &(axis, side, nb, k, raw) in &links {
                let axis = axis as usize;
                let (plane, rect) = world(axis, k, raw);
                // Global portals mirror the tile portals, without boundary portals.
                if p.global_groups {
                    let (gc, gn) = (group_of[own as usize], group_of.get(nb as usize).copied());
                    if let Some(gn) = gn
                        && nb != BOUNDARY
                        && gn != gc
                    {
                        group_links[gc as usize].push((axis as u8, side, gn, k, raw));
                    }
                } else if nb != BOUNDARY {
                    global_portals.push(portal(
                        &PortalSpec {
                            axis,
                            side: u32::from(side),
                            boundary: false,
                            target: nb,
                            local: 0xFFFF,
                            plane,
                            rect,
                        },
                        &domain,
                    ));
                }
            }
            let count = tile_portals.len() as u32 - start;
            let wb = world_box(c.lo, c.hi);
            let packed = pack_box(&wb, &frame);
            let list = &c.objects;
            let mut runs = Vec::new();
            let mut k = 0;
            while k < list.len() {
                let mut run = 1;
                while k + run < list.len()
                    && list[k + run] == list[k] + run as u32
                    && (run as u32) < max_run
                {
                    run += 1;
                }
                runs.push(Run {
                    index: list[k],
                    run: run as u32,
                });
                k += run;
            }
            let ostart = if p.share_lists {
                // Placed once every list is known (`pack_lists`).
                lists.push((node, records.len(), runs));
                0
            } else {
                let at = object_runs.entries.len() as u32;
                object_runs.entries.extend(runs);
                at
            };
            records.push(TileCell([
                start,
                count,
                ostart,
                list.len() as u32,
                group_of[own as usize],
                0,
                packed[0],
                packed[1],
                packed[2],
            ]));
            if p.global_groups {
                let gc = group_of[own as usize] as usize;
                let b = &mut group_box[gc];
                for a in 0..3 {
                    b.0[a] = b.0[a].min(c.lo[a]);
                    b.1[a] = b.1[a].max(c.hi[a]);
                }
                group_samples[gc].push((c.samples.clone(), c.voxels));
            } else {
                global_cells.push(GlobalCell {
                    portal_start: gstart,
                    portal_count: global_portals.len() as u32 - gstart,
                    bounds: pack_box(&wb, &domain),
                });
                report.cell_samples.push(c.samples.clone());
            }
        }
        report.portals += tile_portals.len();
        let n = tile.cells.len() as u32;
        let leaf_bits = 1 + bits_for(n.saturating_sub(1));
        let solid_value = (1u32 << leaf_bits) - 1;
        // Lever L4: the runtime splits every node at its f32 midpoint from the tile's box down;
        // the baker's planes must be exactly those, except in a collapsed tile that keeps its
        // splits explicitly.
        let midpoint = p.midpoint_views && !tile.explicit;
        let mut nodes = std::borrow::Cow::Borrowed(&tile.tree);
        if p.midpoint_views {
            simplify(nodes.to_mut());
        }
        debug_assert!(
            !midpoint || is_midpoint_tree(&nodes, frame),
            "view tree of top node {node} is not a midpoint tree"
        );
        let (mut tree, leaf_values) = emit_tree(&nodes, solid_value, midpoint);
        if p.midpoint_views && tile.explicit {
            trim_splits(&mut tree, frame);
        }
        tiles[node] = Some(Tile {
            bounds: frame,
            flags: 1,
            level: super::tile_level(p.voxel * (1u32 << tile.shift) as f32, p.tile, &frame),
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
    if p.global_groups {
        for (gc, mut links) in group_links.into_iter().enumerate() {
            links.sort_unstable_by_key(|l| (l.0, l.1, l.2, l.3));
            links.dedup_by(|b, a| {
                if (a.0, a.1, a.2, a.3) != (b.0, b.1, b.2, b.3) {
                    return false;
                }
                a.4 = [
                    a.4[0].min(b.4[0]),
                    a.4[1].max(b.4[1]),
                    a.4[2].min(b.4[2]),
                    a.4[3].max(b.4[3]),
                ];
                true
            });
            let gstart = global_portals.len() as u32;
            for (axis, side, gn, k, rect) in links {
                let axis = axis as usize;
                let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
                global_portals.push(portal(
                    &PortalSpec {
                        axis,
                        side: u32::from(side),
                        boundary: false,
                        target: gn,
                        local: 0xFFFF,
                        plane: g.coord(axis, k as usize),
                        rect: [
                            g.coord(a1, rect[0] as usize),
                            g.coord(a1, rect[1] as usize),
                            g.coord(a2, rect[2] as usize),
                            g.coord(a2, rect[3] as usize),
                        ],
                    },
                    &domain,
                ));
            }
            let (lo, hi) = group_box[gc];
            global_cells.push(GlobalCell {
                portal_start: gstart,
                portal_count: global_portals.len() as u32 - gstart,
                bounds: pack_box(&world_box(lo, hi), &domain),
            });
            report
                .cell_samples
                .push(merge_samples(&std::mem::take(&mut group_samples[gc])));
        }
    }
    report.tiles = order.len();
    report.cells = ncells;
    debug_assert!(report.cell_samples.iter().all(|s| s.len() <= CELL_SAMPLES));
    let mut params = [0u8; PARAMS_SIZE];
    let text = format!("tenebrae 0.1 T {:.1} V {:.3}", p.tile, p.voxel);
    params[..text.len().min(PARAMS_SIZE - 1)]
        .copy_from_slice(&text.as_bytes()[..text.len().min(PARAMS_SIZE - 1)]);
    Tome {
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
        objects: survivors
            .iter()
            .map(|&o| ctx.input.targets[o].bounds)
            .collect(),
        users: super::users(survivors.iter().map(|&o| &ctx.input.targets[o])),
        object_runs: {
            if p.share_lists {
                for (node, rec, at) in pack_lists(&mut object_runs.entries, lists) {
                    if let Some(tile) = &mut tiles[node] {
                        tile.cells[rec].0[2] = at;
                    }
                }
            }
            object_runs
        },
        cell_runs: RunList {
            index_bits: bits_for(ngroups.saturating_sub(1) as u32).max(1),
            run_bits: 1,
            entries: Vec::new(),
        },
        global_cells,
        global_portals,
        gates: Gates::default(),
        tiles,
    }
}

/// A view tree as nested nodes, for [`simplify`].
#[derive(Clone, Debug, PartialEq)]
enum Tree {
    Leaf(u32),
    Split(u8, f32, Box<[Tree; 2]>),
}

impl Tree {
    fn of(nodes: &[Node], i: usize) -> Tree {
        match nodes[i] {
            Node::Leaf(v) => Tree::Leaf(v),
            Node::Split { axis, at, kids } => Tree::Split(
                axis,
                at,
                Box::new([
                    Tree::of(nodes, kids[0] as usize),
                    Tree::of(nodes, kids[1] as usize),
                ]),
            ),
        }
    }

    /// The tree in level order.
    fn nodes(self) -> Vec<Node> {
        let mut out = vec![Node::Leaf(SOLID)];
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((0usize, self));
        while let Some((i, t)) = queue.pop_front() {
            out[i] = match t {
                Tree::Leaf(v) => Node::Leaf(v),
                Tree::Split(axis, at, kids) => {
                    let k = out.len() as u32;
                    out.push(Node::Leaf(SOLID));
                    out.push(Node::Leaf(SOLID));
                    let [lo, hi] = *kids;
                    queue.push_back((k as usize, lo));
                    queue.push_back((k as usize + 1, hi));
                    Node::Split {
                        axis,
                        at,
                        kids: [k, k + 1],
                    }
                }
            };
        }
        out
    }

    fn size(&self) -> usize {
        match self {
            Tree::Leaf(_) => 1,
            Tree::Split(_, _, k) => 1 + k[0].size() + k[1].size(),
        }
    }

    /// Whether any split of the subtree is on `axis`.
    fn splits_on(&self, axis: u8) -> bool {
        match self {
            Tree::Leaf(_) => false,
            Tree::Split(a, _, k) => *a == axis || k[0].splits_on(axis) || k[1].splits_on(axis),
        }
    }

    /// The one cell of the subtree's leaves, solid ones being don't-care (`SOLID` when all are
    /// solid); `None` when they hold two cells.
    fn uniform(&self) -> Option<u32> {
        match self {
            Tree::Leaf(v) => Some(*v),
            Tree::Split(_, _, k) => match (k[0].uniform()?, k[1].uniform()?) {
                (a, b) if a == b || b == SOLID => Some(a),
                (SOLID, b) => Some(b),
                _ => None,
            },
        }
    }

    /// The position every path from the root first splits `axis` at, when every path does so
    /// before reaching a leaf, and at one position.
    fn covered(&self, axis: u8) -> Option<f32> {
        match self {
            Tree::Leaf(_) => None,
            Tree::Split(a, at, _) if *a == axis => Some(*at),
            Tree::Split(_, _, k) => {
                let (lo, hi) = (k[0].covered(axis)?, k[1].covered(axis)?);
                (lo.to_bits() == hi.to_bits()).then_some(lo)
            }
        }
    }

    /// The subtree on `side` of the first splits on `axis` (which [`Tree::covered`] found).
    fn restrict(&self, axis: u8, side: usize) -> Tree {
        match self {
            Tree::Leaf(v) => Tree::Leaf(*v),
            Tree::Split(a, _, k) if *a == axis => k[side].clone(),
            Tree::Split(a, at, k) => Tree::Split(
                *a,
                *at,
                Box::new([k[0].restrict(axis, side), k[1].restrict(axis, side)]),
            ),
        }
    }

    /// Umbra's collapse of one node whose children are simplified (`tree_collapseNode`): (a) a uniform subtree becomes a leaf; (b) identical children become one;
    /// (c) a solid child is dropped and the other stretches over it. (b) and (c) need the kept
    /// child to have no split on the node's axis, so every implicit midpoint below stays where it
    /// was.
    fn collapse(self) -> Tree {
        if let Some(v) = self.uniform() {
            return Tree::Leaf(v);
        }
        let Tree::Split(axis, at, kids) = self else {
            return self;
        };
        let keep = if kids[0] == kids[1] || kids[1] == Tree::Leaf(SOLID) {
            Some(0)
        } else if kids[0] == Tree::Leaf(SOLID) {
            Some(1)
        } else {
            None
        };
        match keep {
            Some(k) if !kids[k].splits_on(axis) => {
                let [lo, hi] = *kids;
                if k == 0 { lo } else { hi }
            }
            _ => Tree::Split(axis, at, kids),
        }
    }

    /// Simplifies bottom up: each node is collapsed once its children are; a node that does not
    /// collapse tries lifting each other axis its subtree splits on everywhere (Umbra's
    /// `tree_rotateAxis_r` retry in `tree_simplify_r`), in the order
    /// `(axis + 1) % 3`, `(axis + 2) % 3`, and keeps a rotation that leaves fewer nodes.
    fn simplify(self) -> Tree {
        let Tree::Split(axis, at, kids) = self else {
            return self;
        };
        let [lo, hi] = *kids;
        let mut t = Tree::Split(axis, at, Box::new([lo.simplify(), hi.simplify()])).collapse();
        for step in 1..3u8 {
            let Tree::Split(axis, at, kids) = &t else {
                break;
            };
            let other = (axis + step) % 3;
            let Some(mid) = kids[0].covered(other) else {
                continue;
            };
            if kids[1].covered(other).map(f32::to_bits) != Some(mid.to_bits()) {
                continue;
            }
            let half = |side: usize| {
                Tree::Split(
                    *axis,
                    *at,
                    Box::new([kids[0].restrict(other, side), kids[1].restrict(other, side)]),
                )
                .collapse()
            };
            let rotated = Tree::Split(other, mid, Box::new([half(0), half(1)])).collapse();
            if rotated.size() < t.size() {
                t = rotated;
            }
        }
        t
    }
}

/// Lever L4's view-tree simplification at export, after unreachable-cell removal turned removed
/// cells into solid leaves: Umbra's `tree_simplify_r` with solid space as don't-care, run on every
/// leaf tile before encoding (`tome_exportTile`). A leaf may then cover solid
/// space, or space of removed cells, which the camera never reaches; every open point keeps its
/// cell. Splits that are implicit midpoints stay so.
pub(super) fn simplify(nodes: &mut Vec<Node>) {
    *nodes = Tree::of(nodes, 0).simplify().nodes();
}

/// Collapses splits whose two children are leaves of one value, bottom up, and stores the tree
/// again in level order.
pub(super) fn collapse_leaves(nodes: &mut Vec<Node>) {
    for i in (0..nodes.len()).rev() {
        if let Node::Split { kids, .. } = nodes[i]
            && let (Node::Leaf(a), Node::Leaf(b)) =
                (nodes[kids[0] as usize], nodes[kids[1] as usize])
            && a == b
        {
            nodes[i] = Node::Leaf(a);
        }
    }
    let mut out = Vec::with_capacity(nodes.len());
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(0u32);
    out.push(nodes[0]);
    let mut at = 0usize;
    while let Some(i) = queue.pop_front() {
        if let Node::Split {
            axis,
            at: split,
            kids,
        } = nodes[i as usize]
        {
            let k = out.len() as u32;
            out[at] = Node::Split {
                axis,
                at: split,
                kids: [k, k + 1],
            };
            out.push(nodes[kids[0] as usize]);
            out.push(nodes[kids[1] as usize]);
            queue.push_back(kids[0]);
            queue.push_back(kids[1]);
        }
        at += 1;
    }
    *nodes = out;
}

/// Unreachable-cell removal, Umbra's global-cluster pass (reachability pass 2).
///
/// * A cell takes part when its box overlaps a view volume (its reach box: the bounding box of
///   its overlaps with the view volumes, which may hold space in none of them), or when it gets a reach box through
///   a link whose plane lies inside a neighbour's reach box (propagated to a fixed point).
/// * Components run over links whose plane lies inside the source cell's reach box, between cells
///   that both take part.
/// * A component is kept when it holds at least `threshold` × the largest component's cells.
///
/// Removed cells leave their tiles (renumbered in order), links to them go, view leaves that
/// pointed at them become solid. A `FLEX` leaf takes the first cell of its choices that stays. A target is dropped when it was listed in some cell and every
/// such cell was removed, or kept with no cells under [`super::Params::keep_every_target`].
/// Returns the object map and the number of targets whose every cell went.
pub(super) fn reachability(
    ctx: &Ctx<'_>,
    scene: &mut Scene,
    threshold: f32,
) -> (Vec<Option<u32>>, usize) {
    let g = &ctx.grid;
    let volumes = &ctx.volumes;
    let n = scene.alias.len();
    let mut boxes = vec![
        Aabb {
            min: [0.0; 3],
            max: [0.0; 3],
        };
        n
    ];
    // Links as (neighbour, axis, plane), grouped per cell; merged uids resolve to live ones.
    let alias = scene.alias.clone();
    let live = |mut uid: u32| {
        while alias[uid as usize] != uid {
            uid = alias[uid as usize];
        }
        uid
    };
    let mut span = vec![(0usize, 0usize); n];
    let mut edges: Vec<(u32, u8, f32)> = Vec::new();
    for tile in scene.tiles.iter().flatten() {
        for c in &tile.cells {
            boxes[c.uid as usize] = Aabb {
                min: [
                    g.coord(0, c.lo[0]),
                    g.coord(1, c.lo[1]),
                    g.coord(2, c.lo[2]),
                ],
                max: [
                    g.coord(0, c.hi[0]),
                    g.coord(1, c.hi[1]),
                    g.coord(2, c.hi[2]),
                ],
            };
            let first = edges.len();
            for l in c.links.iter().filter(|l| l.nb != BOUNDARY) {
                edges.push((live(l.nb), l.axis, g.coord(l.axis as usize, l.k as usize)));
            }
            span[c.uid as usize] = (first, edges.len());
        }
    }
    let inside = |r: &Aabb, axis: u8, plane: f32| {
        r.min[axis as usize] <= plane && plane <= r.max[axis as usize]
    };
    let mut is_live = vec![false; n];
    for tile in scene.tiles.iter().flatten() {
        for c in &tile.cells {
            is_live[c.uid as usize] = true;
        }
    }
    let mut reach: Vec<Option<Aabb>> = boxes
        .iter()
        .zip(&is_live)
        .map(|(b, &is_live)| {
            if !is_live {
                return None;
            }
            volumes
                .iter()
                .filter_map(|vv| {
                    let min = [0, 1, 2].map(|a| b.min[a].max(vv.min[a]));
                    let max = [0, 1, 2].map(|a| b.max[a].min(vv.max[a]));
                    (0..3)
                        .all(|a| min[a] <= max[a])
                        .then_some(Aabb { min, max })
                })
                .reduce(|x, y| Aabb {
                    min: [0, 1, 2].map(|a| x.min[a].min(y.min[a])),
                    max: [0, 1, 2].map(|a| x.max[a].max(y.max[a])),
                })
        })
        .collect();
    let mut queue: std::collections::VecDeque<u32> = (0..n as u32)
        .filter(|&c| reach[c as usize].is_some())
        .collect();
    let mut queued: Vec<bool> = reach.iter().map(Option::is_some).collect();
    while let Some(c) = queue.pop_front() {
        queued[c as usize] = false;
        let Some(rc) = reach[c as usize] else {
            continue;
        };
        let (a, b) = span[c as usize];
        for &(m, axis, plane) in &edges[a..b] {
            if !inside(&rc, axis, plane) {
                continue;
            }
            let grown = match reach[m as usize] {
                None => rc,
                Some(r) => Aabb {
                    min: [0, 1, 2].map(|a| r.min[a].min(rc.min[a])),
                    max: [0, 1, 2].map(|a| r.max[a].max(rc.max[a])),
                },
            };
            if reach[m as usize] != Some(grown) {
                reach[m as usize] = Some(grown);
                if !queued[m as usize] {
                    queued[m as usize] = true;
                    queue.push_back(m);
                }
            }
        }
    }
    let mut uf = UnionFind::new(n);
    for c in 0..n {
        let Some(rc) = reach[c] else {
            continue;
        };
        for &(m, axis, plane) in &edges[span[c].0..span[c].1] {
            if reach[m as usize].is_some() && inside(&rc, axis, plane) {
                uf.union(c as u32, m);
            }
        }
    }
    let mut size = vec![0usize; n];
    for (c, r) in reach.iter().enumerate() {
        if r.is_some() {
            size[uf.find(c as u32) as usize] += 1;
        }
    }
    let big = size.iter().copied().max().unwrap_or(0);
    let keep: Vec<bool> = (0..n)
        .map(|c| {
            reach[c].is_some()
                && size[uf.find(c as u32) as usize] as f64 >= f64::from(threshold) * big as f64
        })
        .collect();
    for tile in scene.tiles.iter_mut().flatten() {
        let mut local = vec![SOLID; tile.cells.len()];
        let mut next = 0u32;
        for (i, c) in tile.cells.iter().enumerate() {
            if keep[c.uid as usize] {
                local[i] = next;
                next += 1;
            }
        }
        let flexed = !tile.choices.is_empty();
        if flexed {
            for node in &mut tile.tree {
                if let Node::Leaf(v) = node
                    && *v != SOLID
                    && *v & FLEX != 0
                {
                    *v = tile.choices[(*v & !FLEX) as usize]
                        .iter()
                        .copied()
                        .find(|&c| local[c as usize] != SOLID)
                        .unwrap_or(SOLID);
                }
            }
            tile.choices.clear();
        }
        if next as usize == tile.cells.len() {
            if flexed {
                collapse_leaves(&mut tile.tree);
            }
            continue;
        }
        tile.cells.retain(|c| keep[c.uid as usize]);
        for node in &mut tile.tree {
            if let Node::Leaf(v) = node
                && *v != SOLID
            {
                *v = local[*v as usize];
            }
        }
        collapse_leaves(&mut tile.tree);
    }
    let mut after = vec![false; scene.touched.len()];
    for tile in scene.tiles.iter_mut().flatten() {
        for c in &mut tile.cells {
            c.links
                .retain(|l| l.nb == BOUNDARY || keep[live(l.nb) as usize]);
            for &o in &c.objects {
                after[o as usize] = true;
            }
        }
    }
    // Surviving targets keep input order.
    let keep_all = ctx.input.params.keep_every_target;
    let mut map = vec![None; scene.touched.len()];
    let mut next = 0u32;
    let mut lost = 0usize;
    for (o, m) in map.iter_mut().enumerate() {
        let gone = scene.touched[o] && !after[o];
        lost += usize::from(gone);
        if !gone || keep_all {
            *m = Some(next);
            next += 1;
        }
    }
    for tile in scene.tiles.iter_mut().flatten() {
        for c in &mut tile.cells {
            for o in &mut c.objects {
                *o = map[*o as usize].unwrap_or_else(|| unreachable!());
            }
        }
    }
    (map, lost)
}

/// Most cells two leaf tiles may hold together and still collapse (Umbra's literal 96).
const COLLAPSE_CELLS: usize = 96;

/// Leaf-tile collapse, Umbra's rule, post-order over the top tree so merges
/// cascade upward: two sibling leaf tiles merge when their union's largest extent is at most
/// `size` metres (0: any) and they hold fewer than 96 cells together. The merged tile keeps A's
/// cells then B's; a seam pass then pairs cells across the shared plane whose union is at least
/// `fill` open (see [`super::CellMode::Merged`]; no seam merge without a fill). Its view tree
/// splits at the shared plane with A's and B's trees below (with lever L4, storing splits up to
/// that plane's when it is not the union's midpoint).
pub(super) fn collapse(ctx: &Ctx<'_>, scene: &mut Scene, size: f32, fill: Option<f32>) {
    collapse_node(ctx, scene, 0, size, fill);
}

fn collapse_node(ctx: &Ctx<'_>, scene: &mut Scene, node: usize, size: f32, fill: Option<f32>) {
    let Top::Inner { axis, at, kids } = scene.top[node] else {
        return;
    };
    collapse_node(ctx, scene, kids[0], size, fill);
    collapse_node(ctx, scene, kids[1], size, fill);
    let (Top::Leaf(a), Top::Leaf(b)) = (scene.top[kids[0]], scene.top[kids[1]]) else {
        return;
    };
    let tile = ctx.input.params.tile;
    let (ta, tb) = match (&scene.tiles[a], &scene.tiles[b]) {
        (Some(ta), Some(tb)) => (ta, tb),
        _ => return,
    };
    let range: [[usize; 2]; 3] = std::array::from_fn(|k| {
        [
            ta.range[k][0].min(tb.range[k][0]),
            ta.range[k][1].max(tb.range[k][1]),
        ]
    });
    let extent = (0..3).map(|k| range[k][1] - range[k][0]).max().unwrap_or(0) as f32 * tile;
    // Two coarse tiles (`Params::coarsen_outside`) may grow to their coarser tile size.
    let level_shift = ta.shift.min(tb.shift);
    let limit = if level_shift > 0 {
        size.max(tile * (1u32 << level_shift) as f32)
    } else {
        size
    };
    if size > 0.0 && extent > limit {
        return;
    }
    if ta.cells.len() + tb.cells.len() >= COLLAPSE_CELLS {
        return;
    }
    let (Some(ta), Some(tb)) = (scene.tiles[a].take(), scene.tiles[b].take()) else {
        return;
    };
    let axis_u = axis as usize;
    let plane = (ta.range[axis_u][1] * ctx.layout.tile()) as u32;
    let na = ta.cells.len();
    let mut cells: Vec<GCell> = ta.cells.into_iter().chain(tb.cells).collect();
    for c in &mut cells {
        normalize_links(scene, c, ctx.portal_cost(false));
    }
    let at_of: HashMap<u32, usize> = cells.iter().enumerate().map(|(i, c)| (c.uid, i)).collect();
    // Seam pass: each cell takes the first link across the shared plane to an unpaired cell of
    // the other tile whose union with it is at least `fill` open.
    let n = cells.len();
    let mut partner: Vec<Option<usize>> = vec![None; n];
    let mut paired = vec![false; n];
    if let Some(fill) = fill {
        for c in 0..n {
            if paired[c] {
                continue;
            }
            paired[c] = true;
            for l in &cells[c].links {
                if l.nb == BOUNDARY || l.axis != axis || l.k != plane {
                    continue;
                }
                let Some(&m) = at_of.get(&l.nb) else {
                    continue;
                };
                if (m < na) == (c < na) || paired[m] || !union_fill_ok(&cells[c], &cells[m], fill) {
                    continue;
                }
                partner[c] = Some(m);
                paired[m] = true;
                break;
            }
        }
    }
    // Merged cells in order of their first member.
    let mut new_of = vec![u32::MAX; n];
    let mut next = 0u32;
    for c in 0..n {
        if new_of[c] == u32::MAX {
            new_of[c] = next;
            if let Some(m) = partner[c] {
                new_of[m] = next;
            }
            next += 1;
        }
    }
    let mut slots: Vec<Option<GCell>> = cells.into_iter().map(Some).collect();
    let mut merged: Vec<GCell> = Vec::with_capacity(next as usize);
    for c in 0..n {
        let Some(mut cell) = slots[c].take() else {
            continue;
        };
        if let Some(m) = partner[c]
            && let Some(other) = slots[m].take()
        {
            scene.alias[other.uid as usize] = cell.uid;
            for k in 0..3 {
                cell.lo[k] = cell.lo[k].min(other.lo[k]);
                cell.hi[k] = cell.hi[k].max(other.hi[k]);
            }
            cell.samples = merge_samples(&[
                (std::mem::take(&mut cell.samples), cell.voxels),
                (other.samples, other.voxels),
            ]);
            cell.voxels += other.voxels;
            cell.objects.extend(other.objects);
            cell.objects.sort_unstable();
            cell.objects.dedup();
            cell.links.extend(other.links);
        }
        merged.push(cell);
    }
    for c in &mut merged {
        normalize_links(scene, c, ctx.portal_cost(false));
    }
    // View tree: the shared plane at the root, A's tree below, B's above, leaves renumbered.
    let mut tree = vec![Node::Split {
        axis,
        at,
        kids: [1, 1 + ta.tree.len() as u32],
    }];
    let shift = |nodes: &[Node], offset: u32, base: usize, tree: &mut Vec<Node>| {
        for node in nodes {
            tree.push(match *node {
                Node::Leaf(v) if v == SOLID => Node::Leaf(SOLID),
                Node::Leaf(v) => Node::Leaf(new_of[base + v as usize]),
                Node::Split { axis, at, kids } => Node::Split {
                    axis,
                    at,
                    kids: [kids[0] + offset, kids[1] + offset],
                },
            });
        }
    };
    shift(&ta.tree, 1, 0, &mut tree);
    shift(&tb.tree, 1 + ta.tree.len() as u32, na, &mut tree);
    collapse_leaves(&mut tree);
    // Lever L4: the shared plane is the union's midpoint only when A and B are equally wide (and
    // both are midpoint trees); otherwise the tile keeps the assembled tree (user decision
    // 2026-10-12) and stores a split prefix up to its last off-midpoint split (user, 2026-10-12).
    let explicit = ctx.input.params.midpoint_views
        && !is_midpoint_tree(&tree, tile_frame(&ctx.grid, tile, &range));
    scene.tiles.push(Some(GTile {
        range,
        cells: merged,
        tree,
        choices: Vec::new(),
        explicit,
        shift: level_shift,
    }));
    scene.top[node] = Top::Leaf(scene.tiles.len() - 1);
}

/// Resolves a cell's links to live uids, drops links into the cell itself and merges links with
/// equal keys, sorted by `(axis, side, neighbour, plane)`.
fn normalize_links(scene: &Scene, c: &mut GCell, cost: Option<f64>) {
    for l in &mut c.links {
        if l.nb != BOUNDARY {
            l.nb = scene.resolve(l.nb);
        }
    }
    let own = c.uid;
    let mut keyed: Vec<(LinkKey, [u32; 4])> = c
        .links
        .iter()
        .filter(|l| l.nb != own)
        .map(|l| ((l.axis, l.side, l.nb, l.k), l.rect))
        .collect();
    keyed.sort_unstable();
    coalesce_runs(&mut keyed, cost);
    c.links = keyed
        .into_iter()
        .map(|((axis, side, nb, k), rect)| Link {
            axis,
            side,
            nb,
            k,
            rect,
        })
        .collect();
}

/// The merge rule of [`super::CellMode::Merged`]: the two cells' open voxels fill at least `fill`
/// of their union's bounding box (compared in millionths, as the per-tile merge does).
fn union_fill_ok(a: &GCell, b: &GCell, fill: f32) -> bool {
    let volume: u64 = (0..3)
        .map(|k| (a.hi[k].max(b.hi[k]) - a.lo[k].min(b.lo[k])) as u64)
        .product();
    let f = (a.voxels + b.voxels) as f64 / volume as f64;
    ((f * 1e6) as u64) as f64 >= f64::from(fill) * 1e6
}

/// Lever L1: rebuilds the top tree so that empty space forms large leaf tiles, as Umbra's tile grid
/// does. A grid tile stays a tile of its own when triangles reach it (`populated`) or when it is
/// not wholly open in one cell or wholly solid. The tree splits a region along a grid plane that
/// bounds such tiles, preferring the plane that cuts off the most tiles free of them; a region
/// whose tiles are all empty but of both kinds splits at the middle of its longest axis. Each
/// leaf region of several tiles becomes one ordinary tile: one cell over the whole region when
/// open, no cell when solid.
pub(super) fn empty_leaves(ctx: &Ctx<'_>, scene: &mut Scene, populated: &[bool]) {
    let n = ctx.tiles_n;
    let size = ctx.layout.tile();
    let full = (size * size * size) as u64;
    let at_grid = |c: [usize; 3]| ctx.tile_at[(c[2] * n[1] + c[1]) * n[0] + c[0]];
    // Kind per grid tile: 0 = a tile of its own, 1 = empty and solid, 2 = empty and open.
    let kind: Vec<u8> = (0..scene.tiles.len())
        .map(|t| {
            let tile = scene.tiles[t].as_ref().unwrap_or_else(|| unreachable!());
            let lo = ctx.lo(t);
            if populated[t] {
                0
            } else if tile.cells.is_empty() {
                1
            } else if tile.cells.len() == 1
                && tile.cells[0].voxels == full
                && tile.cells[0].lo == lo
                && tile.cells[0].hi == lo.map(|k| k + size)
            {
                2
            } else {
                0
            }
        })
        .collect();
    let mut old: Vec<Option<GTile>> = std::mem::take(&mut scene.tiles);
    let mut top = vec![Top::Leaf(0)];
    let mut tiles: Vec<Option<GTile>> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((0usize, [[0usize, n[0]], [0, n[1]], [0, n[2]]]));
    while let Some((i, r)) = queue.pop_front() {
        let mut fixed: Vec<[usize; 3]> = Vec::new();
        let mut kinds = [false; 3];
        for z in r[2][0]..r[2][1] {
            for y in r[1][0]..r[1][1] {
                for x in r[0][0]..r[0][1] {
                    let k = kind[at_grid([x, y, z])];
                    kinds[k as usize] = true;
                    if k == 0 {
                        fixed.push([x, y, z]);
                    }
                }
            }
        }
        let ext = [0, 1, 2].map(|a| r[a][1] - r[a][0]);
        let single = ext == [1, 1, 1];
        if single || (fixed.is_empty() && !(kinds[1] && kinds[2])) {
            let members: Vec<usize> = (r[2][0]..r[2][1])
                .flat_map(|z| {
                    (r[1][0]..r[1][1]).flat_map(move |y| (r[0][0]..r[0][1]).map(move |x| [x, y, z]))
                })
                .map(at_grid)
                .collect();
            let tile = if single {
                old[members[0]].take().unwrap_or_else(|| unreachable!())
            } else {
                region_tile(ctx, scene, &mut old, &members, r, kinds[2])
            };
            top[i] = Top::Leaf(tiles.len());
            tiles.push(Some(tile));
            continue;
        }
        let (axis, at) = if fixed.is_empty() {
            let axis = (0..3).max_by_key(|&a| (ext[a], 3 - a)).unwrap_or(0);
            (axis, r[axis][0] + ext[axis] / 2)
        } else {
            region_split(&r, &fixed)
        };
        let (mut lo, mut hi) = (r, r);
        lo[axis][1] = at;
        hi[axis][0] = at;
        let k = top.len();
        top[i] = Top::Inner {
            axis: axis as u8,
            at: ctx.grid.origin[axis] + at as f32 * ctx.input.params.tile,
            kids: [k, k + 1],
        };
        top.push(Top::Leaf(0));
        top.push(Top::Leaf(0));
        queue.push_back((k, lo));
        queue.push_back((k + 1, hi));
    }
    scene.top = top;
    scene.tiles = tiles;
}

/// The split of region `r` holding the tiles `fixed`: among the grid planes bounding them strictly
/// inside the region, the one whose side without such tiles is largest; with no such plane, the
/// one on the longest axis that divides them most evenly.
fn region_split(r: &[[usize; 2]; 3], fixed: &[[usize; 3]]) -> (usize, usize) {
    let ext = [0, 1, 2].map(|a| r[a][1] - r[a][0]);
    let volume = |a: usize, from: usize, to: usize| {
        (to - from)
            * (0..3)
                .filter(|&b| b != a)
                .map(|b| ext[b])
                .product::<usize>()
    };
    let mut best_empty: Option<(usize, usize, usize)> = None; // (volume, axis, plane)
    let mut best_even: Option<((usize, usize), usize, usize)> = None; // ((-extent, imbalance), axis, plane)
    for a in 0..3 {
        let mut planes: Vec<usize> = fixed.iter().flat_map(|c| [c[a], c[a] + 1]).collect();
        planes.sort_unstable();
        planes.dedup();
        for p in planes {
            if p <= r[a][0] || p >= r[a][1] {
                continue;
            }
            let below = fixed.iter().filter(|c| c[a] < p).count();
            let above = fixed.len() - below;
            let empty = match (below, above) {
                (0, _) => volume(a, r[a][0], p),
                (_, 0) => volume(a, p, r[a][1]),
                _ => 0,
            };
            if empty > 0 && best_empty.is_none_or(|b| empty > b.0) {
                best_empty = Some((empty, a, p));
            }
            let key = (usize::MAX - ext[a], below.abs_diff(above));
            if best_even.is_none_or(|b| key < b.0) {
                best_even = Some((key, a, p));
            }
        }
    }
    if let Some((_, a, p)) = best_empty {
        return (a, p);
    }
    if let Some((_, a, p)) = best_even {
        return (a, p);
    }
    // Every fixed tile spans the region on every axis with extent > 1: impossible for unit
    // tiles, but split in the middle to be safe.
    let a = (0..3).max_by_key(|&a| (ext[a], 3 - a)).unwrap_or(0);
    (a, r[a][0] + ext[a] / 2)
}

/// One tile over the empty grid tiles `members` (region `r`): the members' single cells merged
/// into one when `open`, else no cell.
fn region_tile(
    ctx: &Ctx<'_>,
    scene: &mut Scene,
    old: &mut [Option<GTile>],
    members: &[usize],
    r: [[usize; 2]; 3],
    open: bool,
) -> GTile {
    let mut cells: Vec<GCell> = Vec::new();
    let mut shift = u32::MAX;
    for &t in members {
        let tile = old[t].take().unwrap_or_else(|| unreachable!());
        shift = shift.min(tile.shift);
        cells.extend(tile.cells);
    }
    if !open {
        return GTile {
            range: r,
            cells: Vec::new(),
            tree: vec![Node::Leaf(SOLID)],
            choices: Vec::new(),
            explicit: false,
            shift,
        };
    }
    let mut it = cells.into_iter();
    let mut cell = it.next().unwrap_or_else(|| unreachable!());
    for other in it {
        scene.alias[other.uid as usize] = cell.uid;
        for k in 0..3 {
            cell.lo[k] = cell.lo[k].min(other.lo[k]);
            cell.hi[k] = cell.hi[k].max(other.hi[k]);
        }
        cell.voxels += other.voxels;
        cell.objects.extend(other.objects);
        cell.links.extend(other.links);
    }
    cell.objects.sort_unstable();
    cell.objects.dedup();
    normalize_links(scene, &mut cell, ctx.portal_cost(false));
    // Samples of a wholly open box: every stride-th voxel in z, y, x order.
    let d = [0, 1, 2].map(|a| cell.hi[a] - cell.lo[a]);
    let stride = cell.voxels.div_ceil(CELL_SAMPLES as u64).max(1);
    cell.samples = (0..cell.voxels)
        .step_by(stride as usize)
        .map(|i| {
            let i = i as usize;
            let v = [i % d[0], (i / d[0]) % d[1], i / (d[0] * d[1])];
            ctx.grid
                .centre([cell.lo[0] + v[0], cell.lo[1] + v[1], cell.lo[2] + v[2]])
        })
        .collect();
    GTile {
        range: r,
        cells: vec![cell],
        tree: vec![Node::Leaf(0)],
        choices: Vec::new(),
        explicit: false,
        shift,
    }
}

/// The samples of a cell made of several members, each given as (its samples, its open voxels):
/// all of them when they fit in [`CELL_SAMPLES`], else the slots are shared out in proportion to
/// the members' open voxels (largest remainders first, ties in member order, no member above its
/// own sample count, spare slots handed on in member order) and each member's samples are thinned
/// evenly. Member order is kept, so the result is deterministic.
pub(super) fn merge_samples(parts: &[(Vec<Vec3>, u64)]) -> Vec<Vec3> {
    let total: usize = parts.iter().map(|p| p.0.len()).sum();
    if total <= CELL_SAMPLES {
        return parts.iter().flat_map(|p| p.0.iter().copied()).collect();
    }
    let voxels: u128 = parts.iter().map(|p| u128::from(p.1)).sum::<u128>().max(1);
    let slots = CELL_SAMPLES as u128;
    let mut quota: Vec<usize> = parts
        .iter()
        .map(|p| ((slots * u128::from(p.1) / voxels) as usize).min(p.0.len()))
        .collect();
    let mut left = CELL_SAMPLES - quota.iter().sum::<usize>();
    let mut order: Vec<usize> = (0..parts.len()).collect();
    // Largest remainder first; `sort_by_key` is stable, so ties keep member order.
    order.sort_by_key(|&i| std::cmp::Reverse((slots * u128::from(parts[i].1)) % voxels));
    for &i in &order {
        if left > 0 && quota[i] < parts[i].0.len() {
            quota[i] += 1;
            left -= 1;
        }
    }
    // Members capped at their own sample count leave slots over; they go to the others in order
    // (the samples exceed the slots, so this ends).
    while left > 0 {
        for (q, p) in quota.iter_mut().zip(parts) {
            if left > 0 && *q < p.0.len() {
                *q += 1;
                left -= 1;
            }
        }
    }
    let mut out = Vec::with_capacity(CELL_SAMPLES);
    for (p, &q) in parts.iter().zip(&quota) {
        let len = p.0.len();
        out.extend((0..q).map(|j| p.0[j * len / q]));
    }
    out
}
