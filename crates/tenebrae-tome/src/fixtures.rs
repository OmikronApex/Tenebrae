//! Small synthetic tomes for unit tests.

use crate::{
    Aabb, Bsp, Gates, GlobalCell, Header, KdTree, PARAMS_SIZE, RunList, Tile, TileCell, Tome,
    Users, ViewTree,
};

const UNIT: Aabb = Aabb {
    min: [0.0; 3],
    max: [1.0; 3],
};

fn empty_runs() -> RunList {
    RunList {
        index_bits: 0,
        run_bits: 0,
        entries: Vec::new(),
    }
}

/// A tome over the unit cube with the given top tree, tiles and global-cell count.
pub(crate) fn tome(top: KdTree, tiles: Vec<Option<Tile>>, globals: usize) -> Tome {
    let n = top.codes.len();
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
        top,
        node_distances: vec![0.0; n + 1],
        objects: Vec::new(),
        users: Users {
            prefix: None,
            ids: Vec::new(),
        },
        object_runs: empty_runs(),
        cell_runs: empty_runs(),
        global_cells: vec![
            GlobalCell {
                portal_start: 0,
                portal_count: 0,
                bounds: [0, 0xFFFF, 0xFFFF],
            };
            globals
        ],
        global_portals: Vec::new(),
        gates: Gates::default(),
        tiles,
    }
}

/// A top tree of depth `depth` that keeps splitting the cell below the split: inner nodes at
/// 0, 1, 3, 5, ..., leaves at every other node.
pub(crate) fn chain_top(depth: usize) -> KdTree {
    let mut codes = vec![0u8];
    for _ in 1..depth {
        codes.extend([0, 3]);
    }
    if depth > 0 {
        codes.extend([3, 3]);
    } else {
        codes[0] = 3;
    }
    KdTree {
        codes,
        splits: Vec::new(),
    }
}

/// A BSP node: plane index, `(front, front is a cell)`, `(back, back is a cell)`.
pub(crate) fn bsp_node(plane: u32, front: (u16, bool), back: (u16, bool)) -> u64 {
    let lo = plane | (u32::from(front.1) << 31) | (u32::from(back.1) << 30);
    (u64::from(front.0) << 48) | (u64::from(back.0) << 32) | u64::from(lo)
}

/// One tile over the unit cube whose view tree splits at x = 0.5: below is local cell 0, above
/// is BSP node 0. Node 0 tests y >= 0.5 (front: cell 1, back: node 1); node 1 tests z >= 0.5
/// (front: cell 2, back: `0xFFFF`). Local cell `k` is global cell `k`.
pub(crate) fn bsp_tome() -> Tome {
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
                bsp_node(0, (1, true), (1, false)),
                bsp_node(1, (2, true), (0xFFFF, true)),
            ],
            planes: vec![[0.0, 1.0, 0.0, -0.5], [0.0, 0.0, 1.0, -0.5]],
        }),
    };
    tome(
        KdTree {
            codes: vec![3],
            splits: Vec::new(),
        },
        vec![Some(tile)],
        3,
    )
}
