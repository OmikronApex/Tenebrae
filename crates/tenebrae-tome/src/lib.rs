//! The Umbra 3.3.6 tome, format version 18: a lossless typed model, a reader and a writer.
//!
//! The model keeps only what cannot be derived. The writer recomputes the rank directories after
//! every KD tree, the per-node path keys (`+0x98`) and their width (`+0x94`), the per-node tile
//! cell prefix (`+0x88`), the top leaf count (`+0x8C`), every count and offset field, every tile's
//! size field, and the section placement (each section starts on a 16-byte boundary in the order
//! [`write`] documents).

mod bits;
#[cfg(test)]
mod fixtures;
pub mod locate;
mod read;
mod write;

pub use read::read;
pub use write::write;

use core::fmt;

/// Format version stored in the low 16 bits of the first word.
pub const VERSION: u16 = 18;
/// Size of the fixed header, including the bake-parameter string.
pub const HEADER_SIZE: usize = 0x140;
/// Size of the bake-parameter string area (`+0xBC..+0x140`).
pub const PARAMS_SIZE: usize = HEADER_SIZE - 0xBC;
/// Size of a tile header.
pub const TILE_HEADER_SIZE: usize = 0x50;

/// Axis-aligned box, minimum then maximum corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// Minimum corner.
    pub min: [f32; 3],
    /// Maximum corner.
    pub max: [f32; 3],
}

/// Header words with no derivable value. Their meaning is partly unknown, so they are carried.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// High half of the first word; `0xD600` in every known tome.
    pub flags: u16,
    /// `+0x04`: a hash of the body (algorithm unknown, not checked by the runtime as far as known).
    pub hash: u32,
    /// `+0x0C`: base distance used by the portal query constructor.
    pub base_distance: f32,
    /// `+0x10`: zero in every known tome.
    pub word_10: u32,
    /// `+0x14..+0x2C`: the tome's domain. Portals and global cells dequantise against it.
    pub bounds: Aabb,
    /// `+0x34`: zero in every known tome.
    pub word_34: u32,
    /// `+0x54` above bit 20 (the four list widths live below and belong to the run lists).
    pub list_widths_high: u32,
    /// `+0xA4..+0xB0`: zero in every known tome.
    pub words_a4: [u32; 3],
    /// `+0xB0` / `+0xB4`: the alternative cell-to-portal index's count and flag. Must be zero;
    /// the reader refuses a tome that uses it.
    pub words_b0: [u32; 2],
    /// `+0xBC..+0x140`: the bake parameters, NUL padded (`"3.3.6 T 32.0 SO 6.0 ..."`).
    pub params: [u8; PARAMS_SIZE],
}

/// A KD tree in the implicit level-order encoding: 2 bits per node (0/1/2 split on x/y/z, 3 leaf).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KdTree {
    /// One code per node, in level order.
    pub codes: Vec<u8>,
    /// Explicit split positions for the first `splits.len()` nodes; later inner nodes split at
    /// the midpoint of their box.
    pub splits: Vec<f32>,
}

/// One `(index, run)` entry of a run list: it stands for `index, index + 1, ..., index + run - 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// First item.
    pub index: u32,
    /// Number of consecutive items (at least 1 in baked tomes).
    pub run: u32,
}

/// A bit-packed run list (`+0x58` object runs, `+0x60` global-cell runs).
#[derive(Clone, Debug, PartialEq)]
pub struct RunList {
    /// Bits of the index field.
    pub index_bits: u32,
    /// Bits of the run field.
    pub run_bits: u32,
    /// The entries. A cell addresses a slice of them by start entry and expanded item count.
    pub entries: Vec<Run>,
}

/// A 16-byte portal record (the same layout in the global list and in tiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Portal(pub [u32; 4]);

impl Portal {
    /// `true` for a gate portal (bit 27), whose last two words address the gate lists.
    pub fn is_gate(&self) -> bool {
        self.0[0] & (1 << 27) != 0
    }
}

/// A global cell (`+0x80`, 20 bytes): its portal run and a box quantised to the tome bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalCell {
    /// First global portal.
    pub portal_start: u32,
    /// Number of global portals.
    pub portal_count: u32,
    /// Six u16 box coordinates, packed as the tome stores them.
    pub bounds: [u32; 3],
}

/// A tile cell (36 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileCell(pub [u32; 9]);

impl TileCell {
    /// First tile portal.
    pub fn portal_start(&self) -> u32 {
        self.0[0]
    }
    /// Number of tile portals.
    pub fn portal_count(&self) -> u32 {
        self.0[1]
    }
}

/// A leaf tile's view tree: the KD tree locating the camera's local cell.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewTree {
    /// The tree.
    pub tree: KdTree,
    /// Bits per leaf value.
    pub leaf_bits: u32,
    /// One value per leaf, in leaf order. With `all_ones = (1 << leaf_bits) - 1` and the top
    /// bit `1 << (leaf_bits - 1)`: a value below the top bit is a local cell, `all_ones` is
    /// solid space, and any other value names node `value & (all_ones >> 1)` of the tile's
    /// [`Bsp`], which refines the leaf (walk described there).
    pub leaf_values: Vec<u32>,
}

/// Plane BSP refining view-tree leaves.
///
/// A point lookup starts at the node a leaf value names ([`ViewTree::leaf_values`]). For node
/// `n`, with `lo = n as u32`: the plane is `planes[lo & 0x3FFF_FFFF]`, and
/// `d = p.x * x + p.y * y + p.z * z + w` in `f32`. When `d >= 0` the next step is
/// `(n >> 48) & 0xFFFF`, a local cell if `lo` bit 31 is set; otherwise it is
/// `(n >> 32) & 0xFFFF`, a local cell if bit 30 is set. Without that bit the step is the next
/// node. A local cell `0xFFFF` is no cell. The baker never emits a BSP.
#[derive(Clone, Debug, PartialEq)]
pub struct Bsp {
    /// Nodes (u64 each), encoded as above.
    pub nodes: Vec<u64>,
    /// Planes (xyz normal, w distance).
    pub planes: Vec<[f32; 4]>,
}

/// A tile: one top-tree node's cells and portals, plus a view tree on leaf nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// The tile's box.
    pub bounds: Aabb,
    /// `+0x44` low byte (bit 0: has a view tree).
    pub flags: u8,
    /// `+0x48`: a per-tile scale (0, 0.125 ... 2 seen); meaning unknown.
    pub level: f32,
    /// `+0x52`: high half of the cell-count word (0 on inner-node tiles).
    pub cell_word_high: u16,
    /// The cells.
    pub cells: Vec<TileCell>,
    /// The portals the cells address.
    pub portals: Vec<Portal>,
    /// The view tree, on leaf tiles.
    pub view: Option<ViewTree>,
    /// The plane BSP, when a leaf uses one.
    pub bsp: Option<Bsp>,
}

/// Gate data: portals that can be opened and closed at runtime.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gates {
    /// `+0x6C`: one id per gate (`+0x68` = count).
    pub ids: Vec<u32>,
    /// `+0x70`: float3 records gate portals address (`+0x74` = count).
    pub vertices: Vec<[f32; 3]>,
    /// `+0x78`: gate indices gate portals address.
    pub index_list: Vec<u32>,
}

/// User ids: which caller ids each object stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct Users {
    /// `+0x4C`: per object, its first slot in `ids` (objects + 1 entries); absent when every
    /// object has exactly one user.
    pub prefix: Option<Vec<u32>>,
    /// `+0x50`: the user ids.
    pub ids: Vec<u32>,
}

/// A whole tome.
#[derive(Clone, Debug, PartialEq)]
pub struct Tome {
    /// Non-derivable header words.
    pub header: Header,
    /// The top KD tree over tiles.
    pub top: KdTree,
    /// `+0x9C`: one float per top node plus one (`-1` where the node has no tile).
    pub node_distances: Vec<f32>,
    /// `+0x44`: object boxes.
    pub objects: Vec<Aabb>,
    /// `+0x4C` / `+0x50`.
    pub users: Users,
    /// `+0x58`: cell-to-object runs.
    pub object_runs: RunList,
    /// `+0x60`: cell-to-global-cell runs.
    pub cell_runs: RunList,
    /// `+0x80`: global cells.
    pub global_cells: Vec<GlobalCell>,
    /// `+0x84`: global portals.
    pub global_portals: Vec<Portal>,
    /// Gates.
    pub gates: Gates,
    /// One entry per top node: its tile, if any.
    pub tiles: Vec<Option<Tile>>,
}

/// Why a tome could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data ends before a field or section it declares.
    Truncated {
        /// What was being read.
        what: &'static str,
        /// Byte offset.
        offset: usize,
    },
    /// The first word does not carry version 18.
    Version(u16),
    /// A feature the model does not carry is in use.
    Unsupported(&'static str),
    /// A declared value contradicts another.
    Inconsistent(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { what, offset } => write!(f, "truncated {what} at 0x{offset:X}"),
            Error::Version(v) => write!(f, "tome version {v}, expected {VERSION}"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Inconsistent(what) => write!(f, "inconsistent: {what}"),
        }
    }
}

impl std::error::Error for Error {}
