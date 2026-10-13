//! Inner-node LOD tiles (lever [`super::Params::lod_tiles`]): coarse tiles on inner top-tree
//! nodes, which the runtime traverses instead of the leaf tiles below them once the camera is
//! far enough away (a node is walked into while the camera's distance to its
//! box is at most the query distance times the node's `+0x9C` entry).
//!
//! The pass runs on the emitted leaf-only tome. As Umbra's `lod_buildTile`,
//! it pools sibling subtrees bottom up and gives a node a tile once at least [`MIN_TILES`] tiles
//! have pooled since the last one below and its coarse cells number at most [`RATIO`] of the
//! pooled cells; the pool then counts as one tile. Coarse cells are connected components of the
//! subtree's leaf cells (Umbra's mode 3), joined only within each subtree
//! [`SPLIT_LEVELS`] below the node. A coarse cell lists the union of its leaf cells' objects
//! and, through the cell-run list (`+0x60`), their global cells.
//!
//! Every opening between two leaf cells is listed once per pair of levels whose regions do not
//! contain each other: in each tile on the source's path (its leaf tile and the LOD tiles above
//! it) towards each tile on the target's path. The runtime takes a portal into another tile
//! only when its walk selected the target node, so exactly the copies between selected tiles
//! are live. Openings on one plane between the same two cells are coalesced into at most
//! [`MAX_RECTS`] rectangles, each the hull of the openings it replaces.

use std::collections::{BTreeMap, BTreeSet};

use tenebrae_tome::{Aabb, Portal, Run, RunList, Tile, TileCell, Tome};

/// Least number of tiles pooled under a node before it gets a coarse tile (Umbra: 5).
const MIN_TILES: usize = 5;
/// Most coarse cells, as a share of the pooled cells, for a node to get a tile (Umbra: 0.7).
const RATIO: f32 = 0.7;
/// Most cells a coarse tile may hold (Umbra's `UMBRA_MAX_CELLS_PER_TILE`).
const MAX_CELLS: usize = 2047;
/// Most rectangles per opening plane between two cells.
const MAX_RECTS: usize = 4;
/// Leaf cells join a coarse cell only within the same subtree this many levels below the coarse
/// node (a quarter of it). Components over the whole node cull markedly less than Umbra's
/// coarse tiles on the benchmark (city.t16 keeps 21 % more objects than without LOD tiles); two
/// levels bring every case within 7.4 % and Umbra's coarse-cell counts (about 5 per 32 m city
/// tile) within reach.
const SPLIT_LEVELS: usize = 2;

/// Portal flags: boundary (bit 28) and gate (bit 27) portals stay in their leaf tiles.
const BOUNDARY_OR_GATE: u32 = 0x1800_0000;

/// Counts of an [`add`] pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Inner nodes given a coarse tile.
    pub(crate) tiles: usize,
    /// Coarse cells over all of them.
    pub(crate) cells: usize,
}

/// One opening in world space: plane `axis` at `plane`, `side` the portal's side bit, `rect`
/// `[a1 min, a1 max, a2 min, a2 max]` with `a1 = (axis + 1) % 3`, `a2 = (axis + 2) % 3`.
#[derive(Clone, Copy)]
struct Opening {
    axis: usize,
    side: u32,
    plane: f32,
    rect: [f32; 4],
}

fn dequant(q: u32, lo: f32, hi: f32) -> f32 {
    lo + q as f32 / 65535.0 * (hi - lo)
}

/// The writer's quantisation (`global.rs`): `up` rounds outward toward the maximum.
fn quant(x: f32, lo: f32, hi: f32, up: bool) -> u32 {
    let f = ((x - lo) / (hi - lo) * 65535.0).clamp(0.0, 65535.0);
    (if up { f.ceil() } else { f.floor() }) as u32
}

/// The opening of a tile portal, whose coordinates are quantised against the tile's box.
fn opening(p: &Portal, frame: &Aabb) -> Opening {
    let d0 = p.0[0];
    let axis = (d0 >> 30) as usize;
    let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
    let q = |v: u32, a: usize| dequant(v, frame.min[a], frame.max[a]);
    Opening {
        axis,
        side: (d0 >> 29) & 1,
        plane: q(p.0[1] & 0xFFFF, axis),
        rect: [
            q(p.0[2] >> 16, a1),
            q(p.0[2] & 0xFFFF, a1),
            q(p.0[3] >> 16, a2),
            q(p.0[3] & 0xFFFF, a2),
        ],
    }
}

/// A tile portal through `o` into local cell `local` of top node `target`, quantised against
/// `frame` (rectangles rounded outward).
fn portal(o: &Opening, target: u32, local: u32, frame: &Aabb) -> Portal {
    let (axis, side) = (o.axis, o.side);
    let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
    let q = |v: f32, a: usize, up: bool| quant(v, frame.min[a], frame.max[a], up);
    let d0 = ((axis as u32) << 30) | (side << 29) | (target & 0x3FF_FFFF);
    let d1 = q(o.plane, axis, side == 1) | (local << 16);
    let d2 = (q(o.rect[0], a1, false) << 16) | q(o.rect[1], a1, true);
    let d3 = (q(o.rect[2], a2, false) << 16) | q(o.rect[3], a2, true);
    Portal([d0, d1, d2, d3])
}

/// A packed cell box (`+0x18..+0x24`) in world space.
fn unpack_box(w: &[u32], frame: &Aabb) -> Aabb {
    let q = |v: u32, a: usize| dequant(v, frame.min[a], frame.max[a]);
    Aabb {
        min: [q(w[0] >> 16, 0), q(w[0] & 0xFFFF, 1), q(w[1] >> 16, 2)],
        max: [q(w[1] & 0xFFFF, 0), q(w[2] >> 16, 1), q(w[2] & 0xFFFF, 2)],
    }
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

fn union(a: &Aabb, b: &Aabb) -> Aabb {
    Aabb {
        min: std::array::from_fn(|k| a.min[k].min(b.min[k])),
        max: std::array::from_fn(|k| a.max[k].max(b.max[k])),
    }
}

fn area(r: &[f32; 4]) -> f32 {
    (r[1] - r[0]).max(0.0) * (r[3] - r[2]).max(0.0)
}

fn hull(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].max(b[3]),
    ]
}

/// At most [`MAX_RECTS`] rectangles covering `rs`: rectangles inside another are dropped, then
/// the pair whose hull adds the least area is merged until few enough remain.
fn coalesce(mut rs: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
    let inside =
        |a: &[f32; 4], b: &[f32; 4]| a[0] >= b[0] && a[1] <= b[1] && a[2] >= b[2] && a[3] <= b[3];
    let mut k = 0;
    while k < rs.len() {
        if (0..rs.len()).any(|j| j != k && inside(&rs[k], &rs[j]) && (rs[k] != rs[j] || j < k)) {
            rs.remove(k);
        } else {
            k += 1;
        }
    }
    while rs.len() > MAX_RECTS {
        let mut best = (f32::INFINITY, 0, 1);
        for i in 0..rs.len() {
            for j in i + 1..rs.len() {
                let cost = area(&hull(&rs[i], &rs[j])) - area(&rs[i]) - area(&rs[j]);
                if cost < best.0 {
                    best = (cost, i, j);
                }
            }
        }
        let h = hull(&rs[best.1], &rs[best.2]);
        rs.remove(best.2);
        rs[best.1] = h;
    }
    rs
}

fn find(parent: &mut [usize], x: usize) -> usize {
    let mut r = x;
    while parent[r] != r {
        r = parent[r];
    }
    let mut x = x;
    while parent[x] != r {
        let next = parent[x];
        parent[x] = r;
        x = next;
    }
    r
}

/// Bits that hold `n`.
fn bits_for(n: u32) -> u32 {
    32 - n.max(1).leading_zeros()
}

/// The `count` items a cell addresses from entry `start` of `list`.
fn expand(list: &RunList, start: u32, count: u32) -> Vec<u32> {
    let mut out = Vec::with_capacity(count as usize);
    for r in list.entries.iter().skip(start as usize) {
        for k in 0..r.run {
            if out.len() as u32 == count {
                return out;
            }
            out.push(r.index + k);
        }
        if out.len() as u32 == count {
            break;
        }
    }
    out
}

/// Appends `items` (ascending) to `list` as runs; returns the start entry.
fn push_runs(list: &mut RunList, items: &[u32]) -> u32 {
    let start = list.entries.len() as u32;
    let max_run = (1u32 << list.run_bits) - 1;
    let mut k = 0;
    while k < items.len() {
        let mut run = 1;
        while k + run < items.len()
            && items[k + run] == items[k] + run as u32
            && (run as u32) < max_run
        {
            run += 1;
        }
        list.entries.push(Run {
            index: items[k],
            run: run as u32,
        });
        k += run;
    }
    start
}

/// Opening key: axis, side, target node, target local cell, plane bits.
type Key = (usize, u32, u32, u32, u32);

/// Per (source tile, source local cell): per [`Key`], the opening and every rectangle through
/// that plane.
type Emitted = BTreeMap<(usize, usize), BTreeMap<Key, (Opening, Vec<[f32; 4]>)>>;

/// Adds the coarse tiles to a leaf-only tome `t`.
pub(crate) fn add(t: &mut Tome) -> Counts {
    let n = t.top.codes.len();
    // The top tree: children (level order, `2r - 1` and `2r` for the inner node of rank `r`),
    // parents and boxes, split as the runtime splits.
    let mut child = vec![[usize::MAX; 2]; n];
    let mut parent = vec![usize::MAX; n];
    let mut rank = 0;
    for (v, &code) in t.top.codes.iter().enumerate() {
        if code != 3 {
            rank += 1;
            let (lo, hi) = (2 * rank - 1, 2 * rank);
            if hi < n {
                child[v] = [lo, hi];
                parent[lo] = v;
                parent[hi] = v;
            }
        }
    }
    let mut depth = vec![0usize; n];
    for v in 1..n {
        if parent[v] != usize::MAX {
            depth[v] = depth[parent[v]] + 1;
        }
    }
    let mut boxes = vec![t.header.bounds; n];
    for v in 0..n {
        let [lo, hi] = child[v];
        if lo == usize::MAX {
            continue;
        }
        let a = usize::from(t.top.codes[v]);
        let b = boxes[v];
        let split = t
            .top
            .splits
            .get(v)
            .copied()
            .unwrap_or((b.min[a] + b.max[a]) * 0.5);
        boxes[lo] = b;
        boxes[lo].max[a] = split;
        boxes[hi] = b;
        boxes[hi].min[a] = split;
    }
    // Leaf cells, numbered across tiles.
    let mut leaf_cells: Vec<(usize, usize)> = Vec::new();
    let mut first = vec![usize::MAX; n];
    for (v, tile) in t.tiles.iter().enumerate() {
        if let Some(tile) = tile
            && tile.view.is_some()
        {
            first[v] = leaf_cells.len();
            leaf_cells.extend((0..tile.cells.len()).map(|c| (v, c)));
        }
    }
    if leaf_cells.is_empty() {
        return Counts::default();
    }
    let id = |v: usize, c: usize| first[v] + c;
    let portals_of = |tile: &Tile, c: usize| {
        let cell = &tile.cells[c];
        let s = cell.portal_start() as usize;
        tile.portals[s..s + cell.portal_count() as usize].to_vec()
    };
    let leaf_target = |p: &Portal| {
        let d0 = p.0[0];
        let m = (d0 & 0x3FF_FFFF) as usize;
        (d0 & BOUNDARY_OR_GATE == 0 && m < n && first[m] != usize::MAX)
            .then(|| (m, (p.0[1] >> 16) as usize))
    };
    let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); leaf_cells.len()];
    for (f, &(v, c)) in leaf_cells.iter().enumerate() {
        let tile = t.tiles[v].as_ref().unwrap_or_else(|| unreachable!());
        for p in portals_of(tile, c) {
            if let Some((m, d)) = leaf_target(&p) {
                adjacent[f].push(id(m, d));
            }
        }
    }
    // Choice of coarse tiles, children before parents (reverse level order).
    let mut pooled_tiles = vec![0usize; n];
    let mut pooled_cells = vec![0usize; n];
    let mut below: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut coarse: BTreeMap<usize, Vec<Vec<usize>>> = BTreeMap::new();
    let mut uf: Vec<usize> = (0..leaf_cells.len()).collect();
    for v in (0..n).rev() {
        let [a, b] = child[v];
        if a == usize::MAX {
            if first[v] != usize::MAX {
                let k = t.tiles[v].as_ref().map_or(0, |x| x.cells.len());
                pooled_tiles[v] = 1;
                pooled_cells[v] = k;
                below[v] = (first[v]..first[v] + k).collect();
            }
            continue;
        }
        pooled_tiles[v] = pooled_tiles[a] + pooled_tiles[b];
        pooled_cells[v] = pooled_cells[a] + pooled_cells[b];
        let mut cells = std::mem::take(&mut below[a]);
        cells.append(&mut below[b]);
        if pooled_tiles[v] >= MIN_TILES {
            let inside: BTreeSet<usize> = cells.iter().copied().collect();
            for &f in &cells {
                uf[f] = f;
            }
            // The node's descendant `SPLIT_LEVELS` below it that holds leaf cell `f`.
            let block = |f: usize| {
                let mut u = leaf_cells[f].0;
                while depth[u] > depth[v] + SPLIT_LEVELS {
                    u = parent[u];
                }
                u
            };
            for &f in &cells {
                for &g in &adjacent[f] {
                    if inside.contains(&g) && block(f) == block(g) {
                        let (x, y) = (find(&mut uf, f), find(&mut uf, g));
                        if x != y {
                            uf[x] = y;
                        }
                    }
                }
            }
            let mut parts: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for &f in &cells {
                let root = find(&mut uf, f);
                parts.entry(root).or_default().push(f);
            }
            let k = parts.len();
            if k <= MAX_CELLS && k as f32 <= RATIO * pooled_cells[v] as f32 {
                coarse.insert(v, parts.into_values().collect());
                pooled_tiles[v] = 1;
                pooled_cells[v] = k;
            }
        }
        below[v] = cells;
    }
    if coarse.is_empty() {
        return Counts::default();
    }
    let mut coarse_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for (&x, parts) in &coarse {
        for (i, part) in parts.iter().enumerate() {
            for &f in part {
                coarse_of.insert((x, f), i);
            }
        }
    }
    // `v` and the coarse tiles above it, nearest first.
    let path = |v: usize| {
        let mut out = vec![v];
        let mut u = parent[v];
        while u != usize::MAX {
            if coarse.contains_key(&u) {
                out.push(u);
            }
            u = parent[u];
        }
        out
    };
    let contains = |x: usize, mut v: usize| {
        loop {
            if v == x {
                return true;
            }
            if parent[v] == usize::MAX {
                return false;
            }
            v = parent[v];
        }
    };
    // Every opening, once per pair of levels.
    let mut emitted: Emitted = BTreeMap::new();
    let mut put = |x: usize, src: usize, y: usize, dst: usize, o: &Opening| {
        let key = (o.axis, o.side, y as u32, dst as u32, o.plane.to_bits());
        emitted
            .entry((x, src))
            .or_default()
            .entry(key)
            .or_insert((*o, Vec::new()))
            .1
            .push(o.rect);
    };
    for (f, &(l, c)) in leaf_cells.iter().enumerate() {
        let tile = t.tiles[l].as_ref().unwrap_or_else(|| unreachable!());
        let from = path(l);
        for p in portals_of(tile, c) {
            let Some((m, d)) = leaf_target(&p) else {
                continue;
            };
            let g = id(m, d);
            let o = opening(&p, &tile.bounds);
            let to = path(m);
            for &x in &from {
                if contains(x, m) {
                    // Both ends below a coarse tile: a portal between two of its cells.
                    if x != l && coarse_of[&(x, f)] != coarse_of[&(x, g)] {
                        put(x, coarse_of[&(x, f)], x, coarse_of[&(x, g)], &o);
                    }
                    continue;
                }
                let src = if x == l { c } else { coarse_of[&(x, f)] };
                for &y in &to {
                    if contains(y, l) || (x == l && y == m) {
                        continue;
                    }
                    let dst = if y == m { d } else { coarse_of[&(y, g)] };
                    put(x, src, y, dst, &o);
                }
            }
        }
    }
    let emit = |x: usize, src: usize, frame: &Aabb, out: &mut Vec<Portal>| {
        if let Some(e) = emitted.get(&(x, src)) {
            // Portals within the tile first: the runtime's cell sort stops at the first other.
            let mut items: Vec<_> = e.iter().collect();
            items.sort_by_key(|(k, _)| k.2 != x as u32);
            for (&(_, _, y, dst, _), (o, rects)) in items {
                for r in coalesce(rects.clone()) {
                    out.push(portal(&Opening { rect: r, ..*o }, y, dst, frame));
                }
            }
        }
    };
    // Leaf tiles: their portals into coarse tiles follow each cell's own.
    for (v, &start) in first.iter().enumerate() {
        if start == usize::MAX {
            continue;
        }
        let tile = t.tiles[v].as_mut().unwrap_or_else(|| unreachable!());
        let mut portals = Vec::with_capacity(tile.portals.len());
        for c in 0..tile.cells.len() {
            let start = portals.len() as u32;
            portals.extend(portals_of(tile, c));
            emit(v, c, &tile.bounds, &mut portals);
            tile.cells[c].0[0] = start;
            tile.cells[c].0[1] = portals.len() as u32 - start;
        }
        tile.portals = portals;
    }
    // Coarse tiles.
    if t.cell_runs.entries.is_empty() {
        t.cell_runs.index_bits = bits_for(t.global_cells.len() as u32);
        t.cell_runs.run_bits = t.cell_runs.run_bits.max(5);
    }
    let mut counts = Counts::default();
    for (&x, parts) in &coarse {
        let frame = boxes[x];
        let mut cells = Vec::with_capacity(parts.len());
        let mut portals = Vec::new();
        // Umbra: an inner tile's level is the least of its children's.
        let mut level = f32::INFINITY;
        for (i, part) in parts.iter().enumerate() {
            let mut objects = BTreeSet::new();
            let mut globals = BTreeSet::new();
            let mut bounds: Option<Aabb> = None;
            for &f in part {
                let (l, c) = leaf_cells[f];
                let tile = t.tiles[l].as_ref().unwrap_or_else(|| unreachable!());
                level = level.min(tile.level);
                let cell = tile.cells[c].0;
                objects.extend(expand(&t.object_runs, cell[2], cell[3]));
                if cell[5] == 0 {
                    globals.insert(cell[4]);
                } else {
                    globals.extend(expand(&t.cell_runs, cell[4], cell[5]));
                }
                let b = unpack_box(&cell[6..9], &tile.bounds);
                bounds = Some(bounds.map_or(b, |a| union(&a, &b)));
            }
            let objects: Vec<u32> = objects.into_iter().collect();
            let globals: Vec<u32> = globals.into_iter().collect();
            let object_start = push_runs(&mut t.object_runs, &objects);
            let (global, runs) = match globals[..] {
                [g] => (g, 0),
                _ => (push_runs(&mut t.cell_runs, &globals), globals.len() as u32),
            };
            let start = portals.len() as u32;
            emit(x, i, &frame, &mut portals);
            let b = pack_box(&bounds.unwrap_or(frame), &frame);
            cells.push(TileCell([
                start,
                portals.len() as u32 - start,
                object_start,
                objects.len() as u32,
                global,
                runs,
                b[0],
                b[1],
                b[2],
            ]));
        }
        counts.tiles += 1;
        counts.cells += cells.len();
        t.tiles[x] = Some(Tile {
            bounds: frame,
            flags: 0,
            level,
            cell_word_high: 0,
            cells,
            portals,
            view: None,
            bsp: None,
        });
        // Umbra's LOD distance: the cube root of the node's volume in units of the base
        // distance (`+0x9C`, measured on Umbra's tomes).
        let e: [f32; 3] = std::array::from_fn(|k| frame.max[k] - frame.min[k]);
        t.node_distances[x] = (e[0] * e[1] * e[2]).cbrt() / t.header.base_distance;
    }
    counts
}
