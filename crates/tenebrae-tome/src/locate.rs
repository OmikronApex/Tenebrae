//! Point and cell lookups over a [`Tome`], following the runtime's own traversal: the global cell
//! at a point, the view-tree leaves, each cell's regions, the cell transfer between two bakes,
//! and the global cells and portals as geometry.
//!
//! Both KD levels use the implicit level-order encoding: the children of inner node `v` are
//! `2R(v) - 1` (below the split) and `2R(v)` (at or above it), `R(v)` being the number of inner
//! nodes in `0..=v`. Node `v` splits at `splits[v]` when it has one, else at the midpoint of its
//! box. Matches the engine's own lookup on 120,385 sampled points.

use std::collections::BTreeMap;

use crate::{Aabb, GlobalCell, KdTree, Portal, Tile, TileCell, Tome, ViewTree};

/// Every leaf of `tree` over `root`: `(node index, box)` in leaf-ordinal order.
pub fn leaves(tree: &KdTree, root: Aabb) -> Vec<(usize, Aabb)> {
    let n = tree.codes.len();
    let mut boxes = vec![root; n];
    let mut inner = 0usize;
    let mut out = Vec::new();
    for v in 0..n {
        let code = tree.codes[v];
        if code == 3 {
            out.push((v, boxes[v]));
            continue;
        }
        inner += 1;
        let a = usize::from(code);
        let b = boxes[v];
        let s = tree
            .splits
            .get(v)
            .copied()
            .unwrap_or((b.min[a] + b.max[a]) * 0.5);
        let (below, above) = (2 * inner - 1, 2 * inner);
        if above < n {
            boxes[below] = b;
            boxes[below].max[a] = s;
            boxes[above] = b;
            boxes[above].min[a] = s;
        }
    }
    out
}

/// Inner-node ranks of `tree`, as a prefix count: `R(v)` for every node `v`. With them the
/// children of inner node `v` are `2 R(v) - 1` (below the split) and `2 R(v)` (at or above).
pub fn ranks(tree: &KdTree) -> Vec<usize> {
    let mut rank = Vec::with_capacity(tree.codes.len());
    let mut r = 0usize;
    for &c in &tree.codes {
        r += usize::from(c != 3);
        rank.push(r);
    }
    rank
}

/// The leaf node of `tree` holding `p`, or `None` outside `root`.
fn leaf_at(tree: &KdTree, root: Aabb, p: [f32; 3]) -> Option<usize> {
    leaf_ranked(tree, &ranks(tree), root, p)
}

/// The leaf node of `tree` over `root` holding `p`, with the tree's [`ranks`] precomputed;
/// `None` outside `root`. A node splits at its explicit split when it has one, else at the
/// midpoint of its box; `p` at the split goes above it.
pub fn leaf_ranked(tree: &KdTree, rank: &[usize], root: Aabb, p: [f32; 3]) -> Option<usize> {
    if (0..3).any(|a| p[a] < root.min[a] || p[a] > root.max[a]) {
        return None;
    }
    let mut b = root;
    let mut v = 0usize;
    while *tree.codes.get(v)? != 3 {
        let a = usize::from(tree.codes[v]);
        let s = tree
            .splits
            .get(v)
            .copied()
            .unwrap_or((b.min[a] + b.max[a]) * 0.5);
        if s <= p[a] {
            v = 2 * rank[v];
            b.min[a] = s;
        } else {
            v = 2 * rank[v] - 1;
            b.max[a] = s;
        }
    }
    Some(v)
}

/// The global cell holding `p`, as the runtime's findCell returns it: `None` outside the domain,
/// in solid space, or in a leaf refined by a plane BSP (not followed here).
pub fn find_cell(tome: &Tome, p: [f32; 3]) -> Option<u32> {
    let node = leaf_at(&tome.top, tome.header.bounds, p)?;
    let tile = tome.tiles.get(node)?.as_ref()?;
    let view = tile.view.as_ref()?;
    let v = leaf_at(&view.tree, tile.bounds, p)?;
    let ordinal = view.tree.codes[..v].iter().filter(|c| **c == 3).count();
    leaf_cell(tile, view, ordinal)
}

/// The global cell of leaf `ordinal` of a tile's view tree, if it is a cell.
fn leaf_cell(tile: &Tile, view: &ViewTree, ordinal: usize) -> Option<u32> {
    let value = *view.leaf_values.get(ordinal)?;
    let all_ones = if view.leaf_bits >= 32 {
        u32::MAX
    } else {
        (1u32 << view.leaf_bits) - 1
    };
    if value == all_ones || value >> (view.leaf_bits - 1) != 0 {
        return None;
    }
    global_of(tile.cells.get(value as usize)?)
}

fn global_of(cell: &TileCell) -> Option<u32> {
    // Words 4/5: the global cell when word 5 is 0, else a run in the cell-run list.
    (cell.0[5] == 0).then_some(cell.0[4])
}

/// Per global cell, the boxes of the view-tree leaves that resolve to it (leaves refined by a
/// plane BSP are skipped). Every point strictly inside such a box finds that cell.
pub fn cell_regions(tome: &Tome) -> Vec<Vec<Aabb>> {
    let mut out = vec![Vec::new(); tome.global_cells.len()];
    for tile in tome.tiles.iter().flatten() {
        let Some(view) = &tile.view else { continue };
        let all_ones = if view.leaf_bits >= 32 {
            u32::MAX
        } else {
            (1u32 << view.leaf_bits) - 1
        };
        for (k, (_, b)) in leaves(&view.tree, tile.bounds).into_iter().enumerate() {
            let Some(&value) = view.leaf_values.get(k) else {
                continue;
            };
            if value == all_ones || value >> (view.leaf_bits - 1) != 0 {
                continue;
            }
            if let Some(g) = tile.cells.get(value as usize).and_then(global_of)
                && let Some(list) = out.get_mut(g as usize)
            {
                list.push(b);
            }
        }
    }
    out
}

/// [`find_cell`] for many points: the trees' ranks and leaf ordinals are computed once.
pub struct Locator<'t> {
    tome: &'t Tome,
    top: Vec<usize>,
    /// Per top node with a view tree: its ranks and, per node, the leaf ordinal.
    views: Vec<Option<(Vec<usize>, Vec<usize>)>>,
}

impl<'t> Locator<'t> {
    /// Prepares `tome` for point lookups.
    pub fn new(tome: &'t Tome) -> Locator<'t> {
        let views = tome
            .tiles
            .iter()
            .map(|t| {
                let tree = &t.as_ref()?.view.as_ref()?.tree;
                let mut ordinal = Vec::with_capacity(tree.codes.len());
                let mut k = 0usize;
                for &c in &tree.codes {
                    ordinal.push(k);
                    k += usize::from(c == 3);
                }
                Some((ranks(tree), ordinal))
            })
            .collect();
        Locator {
            tome,
            top: ranks(&tome.top),
            views,
        }
    }

    /// Exactly [`find_cell`]`(tome, p)`.
    pub fn find_cell(&self, p: [f32; 3]) -> Option<u32> {
        let t = self.tome;
        let node = leaf_ranked(&t.top, &self.top, t.header.bounds, p)?;
        let tile = t.tiles.get(node)?.as_ref()?;
        let view = tile.view.as_ref()?;
        let (rank, ordinal) = self.views.get(node)?.as_ref()?;
        let v = leaf_ranked(&view.tree, rank, tile.bounds, p)?;
        leaf_cell(tile, view, *ordinal.get(v)?)
    }
}

/// Where one new global cell's open space lies in an old tome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellVotes {
    /// `(old global cell, samples found in it)`, ascending by old cell. Samples in no old cell
    /// (outside its domain, in its solid space) are not counted.
    pub votes: Vec<(u32, u32)>,
    /// For a cell with no votes: the new cell with votes whose point is nearest to this cell's
    /// (squared distance, ties to the lower index); `None` when the cell has votes, or when no
    /// cell has any.
    pub nearest: Option<u32>,
}

/// The cell transfer from `old` to a new tome, for rebuilding per-cell data (light-probe
/// assignments, per-cell bitsets) after a re-bake: per new global cell `g`, the old cells its
/// open-voxel `samples[g]` land in (the baker's `Report::cell_samples`), and for a cell none
/// of whose samples lands anywhere, the nearest voted cell by `points` (the baker's
/// `Report::cell_points`). Taking the majority old cell, or the union over the voted cells, is
/// the caller's rule. The result has one entry per sample list; a cell without a point is never
/// chosen as, and never gets, a nearest cell.
pub fn transfer(
    old: &Tome,
    points: &[Option<[f32; 3]>],
    samples: &[Vec<[f32; 3]>],
) -> Vec<CellVotes> {
    let locator = Locator::new(old);
    let mut out: Vec<CellVotes> = samples
        .iter()
        .map(|list| {
            let mut counts = BTreeMap::new();
            for p in list {
                if let Some(c) = locator.find_cell(*p) {
                    *counts.entry(c).or_insert(0u32) += 1;
                }
            }
            CellVotes {
                votes: counts.into_iter().collect(),
                nearest: None,
            }
        })
        .collect();
    let voted: Vec<(u32, [f32; 3])> = out
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.votes.is_empty())
        .filter_map(|(g, _)| Some((g as u32, (*points.get(g)?)?)))
        .collect();
    for (g, v) in out.iter_mut().enumerate() {
        if !v.votes.is_empty() {
            continue;
        }
        let Some(Some(p)) = points.get(g) else {
            continue;
        };
        let d = |q: &[f32; 3]| -> f64 {
            (0..3)
                .map(|a| (f64::from(q[a]) - f64::from(p[a])).powi(2))
                .sum()
        };
        let mut best: Option<(f64, u32)> = None;
        for (m, q) in &voted {
            let dm = d(q);
            if best.is_none_or(|(bd, _)| dm < bd) {
                best = Some((dm, *m));
            }
        }
        v.nearest = best.map(|(_, m)| m);
    }
    out
}

/// `q` of 65535 steps from `lo` to `hi`: the inverse of the tome's u16 quantisation (exact to
/// one step, outward for maxima). Portals and cell boxes in a tile are quantised against the
/// tile's box, global cells and portals against the domain.
pub fn dequant(q: u32, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * (q as f32 / 65535.0)
}

/// Each global cell's box (`+0x80`), dequantised against the tome's domain. The tome rounds
/// minima down and maxima up, so a box can be up to one 1/65535 step of the domain larger than
/// the cell.
pub fn global_cell_boxes(tome: &Tome) -> Vec<Aabb> {
    let d = &tome.header.bounds;
    tome.global_cells
        .iter()
        .map(|c: &GlobalCell| {
            let w = c.bounds;
            let q = [
                w[0] >> 16,
                w[0] & 0xFFFF,
                w[1] >> 16,
                w[1] & 0xFFFF,
                w[2] >> 16,
                w[2] & 0xFFFF,
            ];
            Aabb {
                min: [0, 1, 2].map(|a| dequant(q[a], d.min[a], d.max[a])),
                max: [0, 1, 2].map(|a| dequant(q[3 + a], d.min[a], d.max[a])),
            }
        })
        .collect()
}

/// One global portal (`+0x84`) as geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalRect {
    /// The global cell whose portal list holds it.
    pub from: u32,
    /// The global cell it leads to; `None` for a portal on the domain boundary.
    pub to: Option<u32>,
    /// The axis the portal faces (0 x, 1 y, 2 z); `rect` is flat on it.
    pub axis: u8,
    /// The portal's rectangle, dequantised against the domain.
    pub rect: Aabb,
}

/// Every non-gate global portal, in the tome's order, as geometry for drawing.
pub fn global_portals(tome: &Tome) -> Vec<PortalRect> {
    let d = &tome.header.bounds;
    let mut out = Vec::with_capacity(tome.global_portals.len());
    for (g, c) in tome.global_cells.iter().enumerate() {
        let start = c.portal_start as usize;
        let end = start.saturating_add(c.portal_count as usize);
        for portal in tome.global_portals.get(start..end).unwrap_or(&[]) {
            if let Some(r) = portal_rect(g as u32, portal, d) {
                out.push(r);
            }
        }
    }
    out
}

fn portal_rect(from: u32, p: &Portal, d: &Aabb) -> Option<PortalRect> {
    if p.is_gate() {
        return None;
    }
    let w = p.0;
    let axis = (w[0] >> 30) as usize;
    if axis > 2 {
        return None;
    }
    let boundary = w[0] & (1 << 28) != 0;
    let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
    let plane = dequant(w[1] & 0xFFFF, d.min[axis], d.max[axis]);
    let mut rect = Aabb {
        min: [plane; 3],
        max: [plane; 3],
    };
    rect.min[a1] = dequant(w[2] >> 16, d.min[a1], d.max[a1]);
    rect.max[a1] = dequant(w[2] & 0xFFFF, d.min[a1], d.max[a1]);
    rect.min[a2] = dequant(w[3] >> 16, d.min[a2], d.max[a2]);
    rect.max[a2] = dequant(w[3] & 0xFFFF, d.min[a2], d.max[a2]);
    Some(PortalRect {
        from,
        to: (!boundary).then_some(w[0] & 0x3FF_FFFF),
        axis: axis as u8,
        rect,
    })
}
