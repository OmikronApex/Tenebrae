//! Point and cell lookups over a [`Tome`], following the runtime's own traversal: the global cell
//! at a point, the view-tree leaves, each cell's regions, the cell transfer between two bakes,
//! and the global cells and portals as geometry.
//!
//! Both KD levels use the implicit level-order encoding: the children of inner node `v` are
//! `2R(v) - 1` (below the split) and `2R(v)` (at or above it), `R(v)` being the number of inner
//! nodes in `0..=v`. Node `v` splits at `splits[v]` when it has one, else at the midpoint of its
//! box. Matches the engine's own lookup on 120,385 sampled points.

use std::collections::BTreeMap;

use crate::{Aabb, Bsp, GlobalCell, KdTree, Portal, Tile, TileCell, Tome, ViewTree};

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

/// The global cell holding `p`, as the runtime's findCell returns it: `None` outside the domain
/// or in solid space. A view-tree leaf refined by a plane BSP is followed to its local cell
/// ([`local_cell`]).
pub fn find_cell(tome: &Tome, p: [f32; 3]) -> Option<u32> {
    let node = leaf_at(&tome.top, tome.header.bounds, p)?;
    let tile = tome.tiles.get(node)?.as_ref()?;
    let view = tile.view.as_ref()?;
    let v = leaf_at(&view.tree, tile.bounds, p)?;
    let ordinal = view.tree.codes[..v].iter().filter(|c| **c == 3).count();
    leaf_cell(tile, view, ordinal, p)
}

/// The local cell at `p` in leaf `ordinal` of `view`, following the tile's plane BSP (`bsp`)
/// when the leaf value names one. `None` for solid space, for a BSP that ends on `0xFFFF`, and
/// for any malformed reference (a missing leaf, node or plane, a walk that loops, `leaf_bits`
/// 0). The result is not checked against the tile's cell table.
///
/// A leaf value below the top bit (`1 << (leaf_bits - 1)`) is the local cell; all ones is solid
/// space; any other value names BSP node `value & (all_ones >> 1)`. The walk is the one
/// [`Bsp`] documents.
pub fn local_cell(view: &ViewTree, bsp: Option<&Bsp>, ordinal: usize, p: [f32; 3]) -> Option<u32> {
    let bits = view.leaf_bits.min(32);
    if bits == 0 {
        return None;
    }
    let value = *view.leaf_values.get(ordinal)?;
    let all_ones = u32::MAX >> (32 - bits);
    if value == all_ones {
        return None;
    }
    if value >> (bits - 1) == 0 {
        return Some(value);
    }
    let bsp = bsp?;
    let mut node = (value & (all_ones >> 1)) as usize;
    // An acyclic walk visits each node at most once; a longer one loops.
    for _ in 0..bsp.nodes.len() {
        let n = *bsp.nodes.get(node)?;
        let lo = n as u32;
        let plane = bsp.planes.get((lo & 0x3FFF_FFFF) as usize)?;
        let d = p[0] * plane[0] + p[1] * plane[1] + p[2] * plane[2] + plane[3];
        let (next, is_cell) = if d >= 0.0 {
            ((n >> 48) & 0xFFFF, lo & (1 << 31) != 0)
        } else {
            ((n >> 32) & 0xFFFF, lo & (1 << 30) != 0)
        };
        if is_cell {
            return (next != 0xFFFF).then_some(next as u32);
        }
        node = next as usize;
    }
    None
}

/// The global cell of leaf `ordinal` of a tile's view tree at `p`, if it is a cell.
fn leaf_cell(tile: &Tile, view: &ViewTree, ordinal: usize, p: [f32; 3]) -> Option<u32> {
    let local = local_cell(view, tile.bsp.as_ref(), ordinal, p)?;
    global_of(tile.cells.get(local as usize)?)
}

fn global_of(cell: &TileCell) -> Option<u32> {
    // Words 4/5: the global cell when word 5 is 0, else a run in the cell-run list.
    (cell.0[5] == 0).then_some(cell.0[4])
}

/// Per global cell, the boxes of the view-tree leaves that resolve to it. Leaves refined by a
/// plane BSP are skipped: their box splits between several cells, so it is exact for none (the
/// point lookups follow the BSP). Every point strictly inside a listed box finds that cell.
pub fn cell_regions(tome: &Tome) -> Vec<Vec<Aabb>> {
    let mut out = vec![Vec::new(); tome.global_cells.len()];
    for tile in tome.tiles.iter().flatten() {
        let Some(view) = &tile.view else { continue };
        if view.leaf_bits == 0 {
            continue;
        }
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
        leaf_cell(tile, view, *ordinal.get(v)?, p)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{bsp_node, bsp_tome};

    fn view(leaf_bits: u32, value: u32) -> ViewTree {
        ViewTree {
            tree: KdTree {
                codes: vec![3],
                splits: Vec::new(),
            },
            leaf_bits,
            leaf_values: vec![value],
        }
    }

    /// Plane 0: x >= 0.5; plane 1: y >= 0.5.
    fn bsp(nodes: Vec<u64>) -> Bsp {
        Bsp {
            nodes,
            planes: vec![[1.0, 0.0, 0.0, -0.5], [0.0, 1.0, 0.0, -0.5]],
        }
    }

    const FRONT: [f32; 3] = [0.75, 0.75, 0.0];
    const BACK: [f32; 3] = [0.25, 0.25, 0.0];

    #[test]
    fn direct_leaves_and_solid_space() {
        assert_eq!(local_cell(&view(8, 5), None, 0, BACK), Some(5));
        assert_eq!(local_cell(&view(8, 0x7F), None, 0, BACK), Some(0x7F));
        assert_eq!(local_cell(&view(8, 0xFF), None, 0, BACK), None);
        assert_eq!(local_cell(&view(8, 5), None, 1, BACK), None);
    }

    #[test]
    fn bsp_front_and_back_reach_cells() {
        let b = bsp(vec![bsp_node(0, (7, true), (9, true))]);
        assert_eq!(local_cell(&view(8, 0x80), Some(&b), 0, FRONT), Some(7));
        assert_eq!(local_cell(&view(8, 0x80), Some(&b), 0, BACK), Some(9));
        // On the plane counts as front.
        assert_eq!(
            local_cell(&view(8, 0x80), Some(&b), 0, [0.5, 0.0, 0.0]),
            Some(7)
        );
    }

    #[test]
    fn bsp_nested_walk() {
        // Node 1 is the root; its back goes to node 0, which tests y.
        let b = bsp(vec![
            bsp_node(1, (3, true), (4, true)),
            bsp_node(0, (2, true), (0, false)),
        ]);
        let v = view(8, 0x81);
        assert_eq!(local_cell(&v, Some(&b), 0, [0.75, 0.0, 0.0]), Some(2));
        assert_eq!(local_cell(&v, Some(&b), 0, [0.25, 0.75, 0.0]), Some(3));
        assert_eq!(local_cell(&v, Some(&b), 0, [0.25, 0.25, 0.0]), Some(4));
    }

    #[test]
    fn bsp_ending_on_0xffff_is_no_cell() {
        let b = bsp(vec![bsp_node(0, (0xFFFF, true), (1, true))]);
        assert_eq!(local_cell(&view(8, 0x80), Some(&b), 0, FRONT), None);
        assert_eq!(local_cell(&view(8, 0x80), Some(&b), 0, BACK), Some(1));
    }

    #[test]
    fn broken_bsp_is_no_cell() {
        // Root out of range, and no BSP at all.
        let b = bsp(vec![bsp_node(0, (1, true), (1, true))]);
        assert_eq!(local_cell(&view(8, 0x85), Some(&b), 0, FRONT), None);
        assert_eq!(local_cell(&view(8, 0x80), None, 0, FRONT), None);
        // A step to a missing node.
        let b = bsp(vec![bsp_node(0, (40, false), (1, true))]);
        assert_eq!(local_cell(&view(8, 0x80), Some(&b), 0, FRONT), None);
        // A missing plane.
        let b = bsp(vec![bsp_node(2, (1, true), (1, true))]);
        assert_eq!(local_cell(&view(8, 0x80), Some(&b), 0, FRONT), None);
        // A loop: node 0 -> node 1 -> node 0.
        let b = bsp(vec![
            bsp_node(0, (1, false), (1, false)),
            bsp_node(1, (0, false), (0, false)),
        ]);
        assert_eq!(local_cell(&view(8, 0x80), Some(&b), 0, FRONT), None);
    }

    #[test]
    fn zero_bit_leaves_are_no_cell() {
        let b = bsp(vec![bsp_node(0, (1, true), (1, true))]);
        assert_eq!(local_cell(&view(0, 0), Some(&b), 0, FRONT), None);
        let mut t = bsp_tome();
        if let Some(Some(tile)) = t.tiles.first_mut()
            && let Some(v) = &mut tile.view
        {
            v.leaf_bits = 0;
        }
        assert_eq!(find_cell(&t, FRONT), None);
        assert_eq!(Locator::new(&t).find_cell(FRONT), None);
        assert!(cell_regions(&t).iter().all(Vec::is_empty));
    }

    #[test]
    fn find_cell_follows_the_bsp() {
        let t = bsp_tome();
        let locator = Locator::new(&t);
        for (p, want) in [
            ([0.25, 0.5, 0.5], Some(0)),
            ([0.75, 0.75, 0.25], Some(1)),
            ([0.75, 0.25, 0.75], Some(2)),
            ([0.75, 0.25, 0.25], None),
            ([1.5, 0.5, 0.5], None),
        ] {
            assert_eq!(find_cell(&t, p), want, "{p:?}");
            assert_eq!(locator.find_cell(p), want, "{p:?}");
        }
        // The BSP leaf is skipped by the region boxes; the direct one is listed.
        let regions = cell_regions(&t);
        assert_eq!(regions[0].len(), 1);
        assert!(regions[1].is_empty() && regions[2].is_empty());
    }

    #[test]
    fn bsp_cell_past_the_cell_table_is_no_cell() {
        let mut t = bsp_tome();
        if let Some(Some(tile)) = t.tiles.first_mut() {
            tile.cells.truncate(2);
        }
        assert_eq!(find_cell(&t, [0.75, 0.25, 0.75]), None);
        assert_eq!(find_cell(&t, [0.75, 0.75, 0.25]), Some(1));
    }

    #[test]
    fn bsp_tome_round_trips() {
        let t = bsp_tome();
        let bytes = crate::write(&t);
        assert_eq!(crate::read(&bytes), Ok(t));
    }
}
