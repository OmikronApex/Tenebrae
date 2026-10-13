//! [`Runtime`]: a tome decoded once into the structures the queries walk, validated so that no
//! query needs to bounds-check the format again.
//!
//! Conventions from the format (`tome_layout.md`): both KD levels use the implicit level-order
//! encoding (the children of inner node `v` are `2R(v) - 1` below the split and `2R(v)` at or
//! above it, `R(v)` the number of inner nodes in `0..=v`; a node splits at its explicit split
//! when it has one, else at the midpoint of its box); tile portals and cell boxes are quantised
//! in 1/65535 steps of the tile's box, global ones of the domain; a cell's objects and global
//! cells are run lists; a gate portal's third word is `start << 12 | count` into the gate index
//! list.

use std::fmt;

use tenebrae_tome::locate::{self, dequant, leaf_ranked, ranks};
use tenebrae_tome::{Aabb, Bsp, KdTree, Portal, RunList, Tile, TileCell, Tome, ViewTree};

/// Why a tome could not be decoded: an index that points outside the structure it addresses.
/// A tome from `tenebrae_tome::read` or the baker never produces one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A reference out of range: what, and in which top node (`None`: the tome level).
    Malformed {
        /// The reference.
        what: &'static str,
        /// The top node whose tile holds it.
        node: Option<usize>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Malformed { what, node: None } => write!(f, "malformed tome: {what}"),
            Error::Malformed {
                what,
                node: Some(n),
            } => write!(f, "malformed tome: {what} in the tile of node {n}"),
        }
    }
}

impl std::error::Error for Error {}

/// A tile portal in world units.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PortalData {
    /// The top node it leads into; meaningless when `boundary`.
    pub(crate) target: u32,
    /// The local cell in that node's tile.
    pub(crate) local: u32,
    /// The axis the portal is flat on.
    pub(crate) axis: u8,
    /// The portal's side bit: 1 when the neighbour lies at the positive side of the plane.
    pub(crate) side: u8,
    /// A portal on the domain boundary: leads nowhere.
    pub(crate) boundary: bool,
    /// A gate portal: the gates it names, as `(start, count)` into the tome's gate index list.
    /// A gate portal carries no rectangle (its words address the gate geometry instead); this
    /// runtime opens it over the whole cell face on its plane.
    pub(crate) gate: Option<(u32, u32)>,
    /// Plane coordinate on `axis`.
    pub(crate) plane: f32,
    /// `[a1 min, a1 max, a2 min, a2 max]`, `a1 = (axis + 1) % 3`, `a2 = (axis + 2) % 3`. For
    /// a gate portal, the cell's box on those axes.
    pub(crate) rect: [f32; 4],
}

/// A cell of a tile.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CellData {
    /// Its portals: a range of the tile's portal list.
    pub(crate) portals: (u32, u32),
    /// Its objects: a range of [`Runtime::object_lists`].
    pub(crate) objects: (u32, u32),
    /// Its global cells: a range of [`Runtime::cell_globals`] (one for a leaf cell, several for
    /// a coarse cell).
    pub(crate) globals: (u32, u32),
    /// Its box.
    pub(crate) bounds: Aabb,
}

/// A leaf tile's view tree with its lookups precomputed.
#[derive(Clone, Debug)]
pub(crate) struct ViewData {
    pub(crate) view: ViewTree,
    pub(crate) ranks: Vec<usize>,
    /// Per tree node, its leaf ordinal (meaningful for leaves).
    pub(crate) ordinal: Vec<usize>,
    /// The tile's plane BSP, which leaves with the top bit set refine.
    pub(crate) bsp: Option<Bsp>,
}

impl ViewData {
    /// The local cell at `p` inside the tile `bounds`: `None` outside the tile, in solid space,
    /// or where a plane BSP ends on no cell ([`locate::local_cell`]).
    pub(crate) fn local_cell(&self, bounds: Aabb, p: [f32; 3]) -> Option<u32> {
        let v = leaf_ranked(&self.view.tree, &self.ranks, bounds, p)?;
        locate::local_cell(&self.view, self.bsp.as_ref(), *self.ordinal.get(v)?, p)
    }
}

/// A tile: one top node's cells and portals.
#[derive(Clone, Debug)]
pub(crate) struct TileData {
    pub(crate) bounds: Aabb,
    /// The tile's `+0x48` level, by which the frustum test of its node grows.
    pub(crate) level: f32,
    pub(crate) cells: Vec<CellData>,
    pub(crate) portals: Vec<PortalData>,
    /// Leaf tiles only.
    pub(crate) view: Option<ViewData>,
    /// Coarse tiles: `(global cell, local cell)` ascending, for retargeting portals that lead
    /// into a tile below this one.
    pub(crate) global_to_local: Vec<(u32, u32)>,
}

impl TileData {
    /// The local cell of a coarse tile that holds global cell `g`.
    pub(crate) fn local_of_global(&self, g: u32) -> Option<u32> {
        let i = self.global_to_local.partition_point(|&(x, _)| x < g);
        self.global_to_local
            .get(i)
            .filter(|&&(x, _)| x == g)
            .map(|&(_, l)| l)
    }
}

/// A tome decoded for queries. Build one per tome and share it across threads; queries run
/// through a [`crate::Query`].
#[derive(Clone, Debug)]
pub struct Runtime {
    pub(crate) bounds: Aabb,
    pub(crate) base_distance: f32,
    pub(crate) top: KdTree,
    pub(crate) top_ranks: Vec<usize>,
    /// Per top node, its box.
    pub(crate) node_boxes: Vec<Aabb>,
    /// Per top node, the `+0x9C` distance (`-1` without a tile).
    pub(crate) node_distances: Vec<f32>,
    /// Per top node, its parent (`u32::MAX` for the root).
    pub(crate) parent: Vec<u32>,
    /// Per top node, its children (`u32::MAX` for a leaf).
    pub(crate) children: Vec<[u32; 2]>,
    pub(crate) tiles: Vec<Option<TileData>>,
    /// Per top node, the first index of its cells in a numbering over every tile's cells
    /// (nodes without a tile repeat the next value); `cell_total` closes it.
    pub(crate) cell_offset: Vec<u32>,
    pub(crate) cell_total: u32,
    /// Per top node, the first index of its portals in a numbering over every tile's
    /// portals; `portal_total` closes it.
    pub(crate) portal_offset: Vec<u32>,
    pub(crate) portal_total: u32,
    pub(crate) objects: Vec<Aabb>,
    /// Every cell's objects, concatenated (see [`CellData::objects`]).
    pub(crate) object_lists: Vec<u32>,
    /// Every cell's global cells, concatenated (see [`CellData::globals`]).
    pub(crate) cell_globals: Vec<u32>,
    pub(crate) gate_count: usize,
    pub(crate) gate_index_list: Vec<u32>,
}

/// Expands `count` items of the run list `list` from entry `start`.
fn expand(
    list: &RunList,
    start: u32,
    count: u32,
    limit: usize,
    out: &mut Vec<u32>,
    what: &'static str,
    node: usize,
) -> Result<(), Error> {
    let bad = || Error::Malformed {
        what,
        node: Some(node),
    };
    let mut at = start as usize;
    let mut left = count;
    while left > 0 {
        let r = list.entries.get(at).ok_or_else(bad)?;
        let take = left.min(r.run);
        for i in r.index..r.index.checked_add(take).ok_or_else(bad)? {
            if i as usize >= limit {
                return Err(bad());
            }
            out.push(i);
        }
        left -= take;
        at += 1;
    }
    Ok(())
}

/// Every node's box under `root` in the implicit level-order encoding, with parents and
/// children.
fn node_boxes(tree: &KdTree, root: Aabb) -> (Vec<Aabb>, Vec<u32>, Vec<[u32; 2]>) {
    let n = tree.codes.len();
    let mut boxes = vec![root; n];
    let mut parent = vec![u32::MAX; n];
    let mut children = vec![[u32::MAX; 2]; n];
    let mut inner = 0usize;
    for v in 0..n {
        let code = tree.codes[v];
        if code == 3 {
            continue;
        }
        inner += 1;
        let a = usize::from(code.min(2));
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
            parent[below] = v as u32;
            parent[above] = v as u32;
            children[v] = [below as u32, above as u32];
        }
    }
    (boxes, parent, children)
}

fn unpack_box(w: &[u32], frame: &Aabb) -> Aabb {
    let q = |v: u32, a: usize| dequant(v, frame.min[a], frame.max[a]);
    Aabb {
        min: [q(w[0] >> 16, 0), q(w[0] & 0xFFFF, 1), q(w[1] >> 16, 2)],
        max: [q(w[1] & 0xFFFF, 0), q(w[2] >> 16, 1), q(w[2] & 0xFFFF, 2)],
    }
}

fn portal_data(p: &Portal, frame: &Aabb, cell: &Aabb) -> PortalData {
    let d0 = p.0[0];
    let axis = ((d0 >> 30) & 3) as usize;
    let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
    let q = |v: u32, a: usize| dequant(v, frame.min[a], frame.max[a]);
    let gate = p.is_gate();
    PortalData {
        target: d0 & 0x3FF_FFFF,
        local: p.0[1] >> 16,
        axis: axis as u8,
        side: ((d0 >> 29) & 1) as u8,
        boundary: d0 & (1 << 28) != 0,
        gate: gate.then_some((p.0[2] >> 12, p.0[2] & 0xFFF)),
        plane: q(p.0[1] & 0xFFFF, axis),
        rect: if gate {
            [cell.min[a1], cell.max[a1], cell.min[a2], cell.max[a2]]
        } else {
            [
                q(p.0[2] >> 16, a1),
                q(p.0[2] & 0xFFFF, a1),
                q(p.0[3] >> 16, a2),
                q(p.0[3] & 0xFFFF, a2),
            ]
        },
    }
}

impl Runtime {
    /// Decodes `tome`. Fails only on a reference outside the structure it addresses, which
    /// `tenebrae_tome::read` and the baker never produce.
    pub fn new(tome: &Tome) -> Result<Runtime, Error> {
        let n = tome.top.codes.len();
        let (node_boxes, parent, children) = node_boxes(&tome.top, tome.header.bounds);
        if tome.tiles.len() != n || tome.node_distances.len() < n {
            return Err(Error::Malformed {
                what: "tile or node-distance count",
                node: None,
            });
        }
        let mut object_lists = Vec::new();
        let mut cell_globals = Vec::new();
        let mut tiles = Vec::with_capacity(n);
        for (node, tile) in tome.tiles.iter().enumerate() {
            let Some(tile) = tile else {
                tiles.push(None);
                continue;
            };
            tiles.push(Some(Self::tile(
                tome,
                node,
                tile,
                node_boxes[node],
                &mut object_lists,
                &mut cell_globals,
            )?));
        }
        // Portal targets, now that every tile's cell count is known.
        for (node, tile) in tiles.iter().enumerate() {
            let Some(tile) = tile else { continue };
            for p in &tile.portals {
                if p.boundary {
                    continue;
                }
                let ok = tiles
                    .get(p.target as usize)
                    .and_then(|t| t.as_ref())
                    .is_some_and(|t| (p.local as usize) < t.cells.len());
                if !ok {
                    return Err(Error::Malformed {
                        what: "portal target",
                        node: Some(node),
                    });
                }
                if let Some((start, count)) = p.gate
                    && (start as usize).saturating_add(count as usize) > tome.gates.index_list.len()
                {
                    return Err(Error::Malformed {
                        what: "gate portal index range",
                        node: Some(node),
                    });
                }
            }
        }
        let mut cell_offset = Vec::with_capacity(n);
        let mut cell_total = 0u32;
        let mut portal_offset = Vec::with_capacity(n);
        let mut portal_total = 0u32;
        for tile in &tiles {
            cell_offset.push(cell_total);
            cell_total += tile.as_ref().map_or(0, |t| t.cells.len() as u32);
            portal_offset.push(portal_total);
            portal_total += tile.as_ref().map_or(0, |t| t.portals.len() as u32);
        }
        let gate_count = tome.gates.ids.len();
        if tome
            .gates
            .index_list
            .iter()
            .any(|&g| g as usize >= gate_count)
        {
            return Err(Error::Malformed {
                what: "gate index",
                node: None,
            });
        }
        Ok(Runtime {
            bounds: tome.header.bounds,
            base_distance: tome.header.base_distance,
            top: tome.top.clone(),
            top_ranks: ranks(&tome.top),
            node_boxes,
            node_distances: tome.node_distances[..n].to_vec(),
            parent,
            children,
            tiles,
            cell_offset,
            cell_total,
            portal_offset,
            portal_total,
            objects: tome.objects.clone(),
            object_lists,
            cell_globals,
            gate_count,
            gate_index_list: tome.gates.index_list.clone(),
        })
    }

    fn tile(
        tome: &Tome,
        node: usize,
        tile: &Tile,
        frame: Aabb,
        object_lists: &mut Vec<u32>,
        cell_globals: &mut Vec<u32>,
    ) -> Result<TileData, Error> {
        let bad = |what: &'static str| Error::Malformed {
            what,
            node: Some(node),
        };
        let nglobal = tome.global_cells.len();
        let mut cells = Vec::with_capacity(tile.cells.len());
        let mut global_to_local = Vec::new();
        for (local, c) in tile.cells.iter().enumerate() {
            let TileCell(w) = c;
            let (ps, pc) = (w[0], w[1]);
            if (ps as usize).saturating_add(pc as usize) > tile.portals.len() {
                return Err(bad("cell portal range"));
            }
            let ostart = object_lists.len() as u32;
            expand(
                &tome.object_runs,
                w[2],
                w[3],
                tome.objects.len(),
                object_lists,
                "cell object list",
                node,
            )?;
            let gstart = cell_globals.len() as u32;
            if w[5] == 0 {
                if w[4] as usize >= nglobal {
                    return Err(bad("cell global cell"));
                }
                cell_globals.push(w[4]);
            } else {
                expand(
                    &tome.cell_runs,
                    w[4],
                    w[5],
                    nglobal,
                    cell_globals,
                    "cell global-cell list",
                    node,
                )?;
            }
            let gcount = cell_globals.len() as u32 - gstart;
            if tile.view.is_none() {
                for &g in &cell_globals[gstart as usize..] {
                    global_to_local.push((g, local as u32));
                }
            }
            cells.push(CellData {
                portals: (ps, pc),
                objects: (ostart, w[3]),
                globals: (gstart, gcount),
                bounds: unpack_box(&w[6..9], &frame),
            });
        }
        global_to_local.sort_unstable();
        // Each portal's owning cell, for the gate portals' rectangle.
        let mut owner = vec![frame; tile.portals.len()];
        for c in &cells {
            let (s, n) = (c.portals.0 as usize, c.portals.1 as usize);
            owner[s..s + n].iter_mut().for_each(|o| *o = c.bounds);
        }
        let portals = tile
            .portals
            .iter()
            .zip(&owner)
            .map(|(p, cell)| portal_data(p, &frame, cell))
            .collect();
        let view = tile.view.as_ref().map(|v| {
            let mut ordinal = Vec::with_capacity(v.tree.codes.len());
            let mut k = 0usize;
            for &c in &v.tree.codes {
                ordinal.push(k);
                k += usize::from(c == 3);
            }
            ViewData {
                view: v.clone(),
                ranks: ranks(&v.tree),
                ordinal,
                bsp: tile.bsp.clone(),
            }
        });
        Ok(TileData {
            bounds: tile.bounds,
            level: tile.level,
            cells,
            portals,
            view,
            global_to_local,
        })
    }

    /// Number of objects.
    pub fn objects(&self) -> usize {
        self.objects.len()
    }

    /// The tome's base distance (`+0x0C`), the level-of-detail scale an `accuracy` of `-1`
    /// stands for.
    pub fn base_distance(&self) -> f32 {
        self.base_distance
    }

    /// The domain.
    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    /// Number of gates the tome declares.
    pub fn gates(&self) -> usize {
        self.gate_count
    }

    /// The top node and local cell holding `p`: `None` outside the domain, in solid space, or
    /// where a plane BSP ends on no cell or on one past the tile's cells.
    pub(crate) fn local_cell(&self, p: [f32; 3]) -> Option<(usize, u32)> {
        let node = leaf_ranked(&self.top, &self.top_ranks, self.bounds, p)?;
        let tile = self.tiles.get(node)?.as_ref()?;
        let local = tile.view.as_ref()?.local_cell(tile.bounds, p)?;
        ((local as usize) < tile.cells.len()).then_some((node, local))
    }

    /// The global cell holding `p`, as `tenebrae_tome::locate::find_cell` finds it.
    pub fn find_cell(&self, p: [f32; 3]) -> Option<u32> {
        let (node, local) = self.local_cell(p)?;
        let cell = &self.tiles[node].as_ref()?.cells[local as usize];
        (cell.globals.1 == 1).then(|| self.cell_globals[cell.globals.0 as usize])
    }

    /// The tile of `node`, which the caller knows to exist.
    pub(crate) fn tile_of(&self, node: u32) -> &TileData {
        self.tiles[node as usize]
            .as_ref()
            .unwrap_or_else(|| unreachable!("node {node} has no tile"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tenebrae_tome::{Gates, GlobalCell, Header, PARAMS_SIZE, Users};

    const UNIT: Aabb = Aabb {
        min: [0.0; 3],
        max: [1.0; 3],
    };

    fn node(plane: u32, front: (u16, bool), back: (u16, bool)) -> u64 {
        let lo = plane | (u32::from(front.1) << 31) | (u32::from(back.1) << 30);
        (u64::from(front.0) << 48) | (u64::from(back.0) << 32) | u64::from(lo)
    }

    /// One tile over the unit cube: x < 0.5 is local cell 0; above, a BSP splits on y, then on
    /// z (cells 1 and 2, and no cell); local cell `k` is global cell `k`.
    fn bsp_tome() -> Tome {
        let runs = || RunList {
            index_bits: 0,
            run_bits: 0,
            entries: Vec::new(),
        };
        let tile = Tile {
            bounds: UNIT,
            flags: 1,
            level: 0.0,
            cell_word_high: 0,
            cells: (0..3)
                .map(|g| TileCell([0, 0, 0, 0, g, 0, 0, 0xFFFF, 0xFFFF]))
                .collect(),
            portals: Vec::new(),
            view: Some(ViewTree {
                tree: KdTree {
                    codes: vec![0, 3, 3],
                    splits: Vec::new(),
                },
                leaf_bits: 8,
                leaf_values: vec![0, 0x80],
            }),
            bsp: Some(Bsp {
                nodes: vec![
                    node(0, (1, true), (1, false)),
                    node(1, (2, true), (0xFFFF, true)),
                ],
                planes: vec![[0.0, 1.0, 0.0, -0.5], [0.0, 0.0, 1.0, -0.5]],
            }),
        };
        Tome {
            header: Header {
                flags: 0xD600,
                hash: 0,
                base_distance: 1.0,
                word_10: 0,
                bounds: UNIT,
                word_34: 0,
                list_widths_high: 0,
                words_a4: [0; 3],
                words_b0: [0; 2],
                params: [0; PARAMS_SIZE],
            },
            top: KdTree {
                codes: vec![3],
                splits: Vec::new(),
            },
            node_distances: vec![0.0; 2],
            objects: Vec::new(),
            users: Users {
                prefix: None,
                ids: Vec::new(),
            },
            object_runs: runs(),
            cell_runs: runs(),
            global_cells: vec![
                GlobalCell {
                    portal_start: 0,
                    portal_count: 0,
                    bounds: [0, 0xFFFF, 0xFFFF],
                };
                3
            ],
            global_portals: Vec::new(),
            gates: Gates::default(),
            tiles: vec![Some(tile)],
        }
    }

    #[test]
    fn find_cell_follows_the_bsp_as_locate_does() {
        let tome = bsp_tome();
        let rt = Runtime::new(&tome).expect("decode");
        let mut seen = [0usize; 4];
        let steps = 24;
        for i in 0..=steps {
            for j in 0..=steps {
                for k in 0..=steps {
                    let p = [i, j, k].map(|s| s as f32 / steps as f32);
                    let want = locate::find_cell(&tome, p);
                    assert_eq!(rt.find_cell(p), want, "{p:?}");
                    seen[want.map_or(3, |c| c as usize)] += 1;
                }
            }
        }
        assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
        assert_eq!(rt.find_cell([0.75, 0.25, 0.75]), Some(2));
    }

    #[test]
    fn bsp_cell_past_the_cell_table_is_no_cell() {
        let mut tome = bsp_tome();
        if let Some(Some(tile)) = tome.tiles.first_mut() {
            tile.cells.truncate(2);
        }
        let rt = Runtime::new(&tome).expect("decode");
        for (p, want) in [([0.75, 0.25, 0.75], None), ([0.75, 0.75, 0.25], Some(1))] {
            assert_eq!(rt.find_cell(p), want, "{p:?}");
            assert_eq!(locate::find_cell(&tome, p), want, "{p:?}");
        }
    }
}
