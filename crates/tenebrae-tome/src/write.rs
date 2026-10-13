//! Tome writer: the typed model to bytes.
//!
//! Physical order, each section starting on a 16-byte boundary (zero padded):
//! header (0x140), top KD codes + rank directory (`+0x30`), top splits (`+0x3C`), node keys
//! (`+0x98`), node distances (`+0x9C`), node cell prefix (`+0x88`), object runs (`+0x58`), cell runs
//! (`+0x60`), global cells (`+0x80`), global portals (`+0x84`), gate vertices (`+0x70`), gate index
//! list (`+0x78`), object boxes (`+0x44`), user prefix (`+0x4C`), user ids (`+0x50`), tile table
//! (`+0xA0`), tiles, gate ids (`+0x6C`).
//!
//! Inside a tile: header (0x50), cells, portals, BSP planes, view-tree codes + rank directory,
//! splits, leaf values, BSP nodes, each on a 16-byte boundary relative to the tile.

use super::bits::{self, LEAF};
use super::{Aabb, HEADER_SIZE, KdTree, Portal, RunList, TILE_HEADER_SIZE, Tile, Tome, VERSION};

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn align(&mut self) -> usize {
        while !self.0.len().is_multiple_of(16) {
            self.0.push(0);
        }
        self.0.len()
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn words(&mut self, w: &[u32]) {
        for &v in w {
            self.u32(v);
        }
    }
    fn aabb(&mut self, b: &Aabb) {
        for v in b.min.iter().chain(b.max.iter()) {
            self.f32(*v);
        }
    }
    fn portals(&mut self, p: &[Portal]) {
        for portal in p {
            self.words(&portal.0);
        }
    }
    fn set(&mut self, offset: usize, v: u32) {
        self.0[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
    }
    /// Starts a 16-aligned section and returns its offset, or 0 (and writes nothing) when empty.
    fn section(&mut self, present: bool) -> u32 {
        if present { self.align() as u32 } else { 0 }
    }
}

fn kd(out: &mut Out, tree: &KdTree) {
    out.words(&bits::pack_codes(&tree.codes));
    out.words(&bits::rank_directory(&tree.codes));
}

fn runs(out: &mut Out, list: &RunList) {
    let width = list.index_bits + list.run_bits;
    let values = list
        .entries
        .iter()
        .map(|r| r.index | (r.run << list.index_bits));
    out.words(&bits::pack(
        values,
        width,
        bits::packed_words(list.entries.len(), width),
    ));
}

/// Children of inner node `v` are `2R(v) - 1` (below the split) and `2R(v)` (above), where
/// `R(v)` counts inner nodes in `[0, v]`. Returns each node's depth.
fn depths(codes: &[u8]) -> Vec<u32> {
    let mut depth = vec![0u32; codes.len()];
    let mut inner = 0usize;
    for v in 0..codes.len() {
        if codes[v] != LEAF {
            inner += 1;
            for child in [2 * inner - 1, 2 * inner] {
                if child < codes.len() {
                    depth[child] = depth[v] + 1;
                }
            }
        }
    }
    depth
}

/// Per-node path keys (`+0x98`), packed LSB-first at width `w` = max depth + 1, and `w`. The
/// root is `1 << (w - 1)`; a child is its parent shifted right, with the top bit set on the
/// child below the split. So at depth `d` the marker sits at bit `w - 1 - d`, and bit
/// `w - 1 - d + j` (`j` = 1..=d) is step `j` from the root (1 = below the split). Built bit by
/// bit, so `w` may exceed 32 (stock tomes reach 34).
fn node_keys(codes: &[u8]) -> (u32, Vec<u32>) {
    let n = codes.len();
    let width = depths(codes).into_iter().max().map_or(1, |d| d + 1);
    let w = width as usize;
    let mut words = vec![0u32; bits::packed_words(n, width)];
    let get = |words: &[u32], node: usize, b: usize| {
        let at = node * w + b;
        words[at >> 5] >> (at & 31) & 1 != 0
    };
    let set = |words: &mut [u32], node: usize, b: usize| {
        let at = node * w + b;
        words[at >> 5] |= 1 << (at & 31);
    };
    if n > 0 {
        set(&mut words, 0, w - 1);
    }
    let mut inner = 0usize;
    for (v, &code) in codes.iter().enumerate() {
        if code != LEAF {
            inner += 1;
            if 2 * inner < n {
                for (child, below) in [(2 * inner - 1, true), (2 * inner, false)] {
                    for b in 1..w {
                        if get(&words, v, b) {
                            set(&mut words, child, b - 1);
                        }
                    }
                    if below {
                        set(&mut words, child, w - 1);
                    }
                }
            }
        }
    }
    (width, words)
}

fn tile(out: &mut Out, t: &Tile) {
    let base = out.align();
    let rel = |out: &Out| (out.0.len() - base) as u32;
    let align = |out: &mut Out| {
        while !(out.0.len() - base).is_multiple_of(16) {
            out.0.push(0);
        }
    };
    out.0.resize(base + TILE_HEADER_SIZE, 0);
    let mut w = [0u32; 20];
    let cells = if t.cells.is_empty() {
        0
    } else {
        align(out);
        let at = rel(out);
        for c in &t.cells {
            out.words(&c.0);
        }
        at
    };
    let portals = if t.portals.is_empty() {
        0
    } else {
        align(out);
        let at = rel(out);
        out.portals(&t.portals);
        at
    };
    let planes = match &t.bsp {
        Some(b) if !b.planes.is_empty() => {
            align(out);
            let at = rel(out);
            for p in &b.planes {
                for v in p {
                    out.f32(*v);
                }
            }
            at
        }
        _ => 0,
    };
    if let Some(view) = &t.view {
        align(out);
        w[6] = ((view.tree.codes.len() as u32) << 5) | view.leaf_bits;
        w[7] = rel(out);
        kd(out, &view.tree);
        if !view.tree.splits.is_empty() {
            align(out);
            w[9] = view.tree.splits.len() as u32;
            w[10] = rel(out);
            for s in &view.tree.splits {
                out.f32(*s);
            }
        }
        align(out);
        w[8] = rel(out);
        let words = bits::packed_words(view.leaf_values.len(), view.leaf_bits);
        out.words(&bits::pack(
            view.leaf_values.iter().copied(),
            view.leaf_bits,
            words,
        ));
    }
    if let Some(b) = &t.bsp {
        w[17] = b.nodes.len() as u32;
        w[19] = b.planes.len() as u32;
        w[18] = planes;
        if !b.nodes.is_empty() {
            align(out);
            w[16] = rel(out);
            for n in &b.nodes {
                out.u32(*n as u32);
                out.u32((*n >> 32) as u32);
            }
        }
    }
    align(out);
    let size = rel(out);
    w[11] = (size << 8) | u32::from(t.flags);
    w[12] = t.level.to_bits();
    w[13] = t.cells.len() as u32 | (u32::from(t.cell_word_high) << 16);
    w[14] = cells;
    w[15] = portals;
    let mut head = Out::default();
    head.aabb(&t.bounds);
    for (i, v) in head.0.chunks_exact(4).enumerate() {
        w[i] = u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
    }
    for (i, v) in w.iter().enumerate() {
        out.set(base + 4 * i, *v);
    }
}

/// Writes a tome.
pub fn write(t: &Tome) -> Vec<u8> {
    let mut out = Out(vec![0u8; HEADER_SIZE]);
    let n = t.top.codes.len();
    let h = &t.header;

    let top_codes = out.section(true);
    kd(&mut out, &t.top);
    let top_splits = out.section(true);
    for s in &t.top.splits {
        out.f32(*s);
    }
    let (key_bits, keys) = node_keys(&t.top.codes);
    let keys_at = out.section(true);
    out.words(&keys);
    let distances = out.section(true);
    for d in &t.node_distances {
        out.f32(*d);
    }
    let prefix_at = out.section(true);
    let mut total = 0u32;
    out.u32(0);
    for tile in &t.tiles {
        total += tile.as_ref().map_or(0, |t| t.cells.len() as u32);
        out.u32(total);
    }
    let object_runs = out.section(!t.object_runs.entries.is_empty());
    runs(&mut out, &t.object_runs);
    let cell_runs = out.section(!t.cell_runs.entries.is_empty());
    runs(&mut out, &t.cell_runs);
    let cells_at = out.section(!t.global_cells.is_empty());
    for c in &t.global_cells {
        out.words(&[
            c.portal_start,
            c.portal_count,
            c.bounds[0],
            c.bounds[1],
            c.bounds[2],
        ]);
    }
    let portals_at = out.section(!t.global_portals.is_empty());
    out.portals(&t.global_portals);
    let gate_vertices = out.section(!t.gates.vertices.is_empty());
    for v in &t.gates.vertices {
        for c in v {
            out.f32(*c);
        }
    }
    let gate_list = out.section(!t.gates.index_list.is_empty());
    out.words(&t.gates.index_list);
    let objects_at = out.section(!t.objects.is_empty());
    for o in &t.objects {
        out.aabb(o);
    }
    let user_prefix = out.section(t.users.prefix.is_some());
    if let Some(p) = &t.users.prefix {
        out.words(p);
    }
    let user_ids = out.section(!t.users.ids.is_empty());
    out.words(&t.users.ids);
    let table = out.section(true);
    out.0.resize(table as usize + 4 * n, 0);
    for (node, tile_data) in t.tiles.iter().enumerate() {
        if let Some(tile_data) = tile_data {
            let at = out.align() as u32;
            out.set(table as usize + 4 * node, at);
            tile(&mut out, tile_data);
        }
    }
    let gate_ids = out.section(!t.gates.ids.is_empty());
    out.words(&t.gates.ids);
    out.align();

    let size = out.0.len() as u32;
    let mut head = Out(Vec::with_capacity(HEADER_SIZE));
    head.u32(u32::from(VERSION) | (u32::from(h.flags) << 16));
    head.u32(h.hash);
    head.u32(size);
    head.f32(h.base_distance);
    head.u32(h.word_10);
    head.aabb(&h.bounds);
    head.u32((n as u32) << 5); // +2C
    head.u32(top_codes); // +30
    head.u32(h.word_34); // +34
    head.u32(t.top.splits.len() as u32); // +38
    head.u32(top_splits); // +3C
    head.u32(t.objects.len() as u32); // +40
    head.u32(objects_at); // +44
    head.u32(0); // +48
    head.u32(user_prefix); // +4C
    head.u32(user_ids); // +50
    let o = &t.object_runs;
    let c = &t.cell_runs;
    head.u32(
        o.index_bits
            | (o.run_bits << 5)
            | (c.index_bits << 10)
            | (c.run_bits << 15)
            | (h.list_widths_high << 20),
    ); // +54
    head.u32(object_runs); // +58
    head.u32(o.entries.len() as u32); // +5C
    head.u32(cell_runs); // +60
    head.u32(c.entries.len() as u32); // +64
    head.u32(t.gates.ids.len() as u32); // +68
    head.u32(gate_ids); // +6C
    head.u32(gate_vertices); // +70
    head.u32(t.gates.vertices.len() as u32); // +74
    head.u32(gate_list); // +78
    head.u32(t.global_cells.len() as u32); // +7C
    head.u32(cells_at); // +80
    head.u32(portals_at); // +84
    head.u32(prefix_at); // +88
    head.u32(
        t.tiles
            .iter()
            .zip(&t.top.codes)
            .filter(|(_, c)| **c == LEAF)
            .count() as u32,
    ); // +8C
    head.u32(n as u32); // +90
    head.u32(key_bits); // +94
    head.u32(keys_at); // +98
    head.u32(distances); // +9C
    head.u32(table); // +A0
    head.words(&h.words_a4); // +A4..+AC
    head.words(&h.words_b0); // +B0, +B4
    head.u32(0); // +B8
    head.0.extend_from_slice(&h.params);
    out.0[..HEADER_SIZE].copy_from_slice(&head.0);
    out.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{chain_top, tome};

    /// The keys as `u32` values, as the writer built them before keys could exceed 32 bits.
    fn u32_keys(codes: &[u8]) -> (u32, Vec<u32>) {
        let width = depths(codes).into_iter().max().map_or(1, |d| d + 1);
        let top = 1u32 << (width - 1);
        let mut keys = vec![0u32; codes.len()];
        if !keys.is_empty() {
            keys[0] = top;
        }
        let mut inner = 0usize;
        for v in 0..codes.len() {
            if codes[v] != LEAF {
                inner += 1;
                if 2 * inner < codes.len() {
                    keys[2 * inner - 1] = (keys[v] >> 1) | top;
                    keys[2 * inner] = keys[v] >> 1;
                }
            }
        }
        (width, keys)
    }

    /// A tree of `depth` levels with leaves cut in by `mask`, for varied shapes, and at most
    /// `cap` inner nodes per level.
    fn ragged(depth: usize, mask: u32, cap: usize) -> Vec<u8> {
        let mut codes = vec![0u8];
        let mut frontier = 1usize;
        let mut k = 0u32;
        for level in 1..=depth {
            let mut next = 0;
            for _ in 0..2 * frontier {
                k = k.wrapping_mul(1_103_515_245).wrapping_add(12_345 ^ mask);
                let inner = level < depth && (next == 0 || ((k >> 16) & 3 != 0 && next < cap));
                codes.push(if inner { (k >> 20) as u8 % 3 } else { LEAF });
                next += usize::from(inner);
            }
            frontier = next;
        }
        codes
    }

    #[test]
    fn keys_match_the_u32_path_below_32_levels() {
        let mut trees: Vec<Vec<u8>> = (0..32).map(|d| chain_top(d).codes).collect();
        trees.extend((1..12).map(|d| ragged(d, d as u32, usize::MAX)));
        trees.push(Vec::new());
        for codes in &trees {
            let (w, old) = u32_keys(codes);
            let (width, words) = node_keys(codes);
            assert_eq!(width, w);
            assert_eq!(
                words,
                bits::pack(old, w, bits::packed_words(codes.len(), w)),
                "{codes:?}"
            );
        }
    }

    #[test]
    fn deep_keys_hold_the_path() {
        for depth in [33usize, 70] {
            let codes = chain_top(depth).codes;
            let (width, words) = node_keys(&codes);
            assert_eq!(width as usize, depth + 1);
            let w = width as usize;
            let bit = |node: usize, b: usize| {
                let at = node * w + b;
                words[at >> 5] >> (at & 31) & 1
            };
            // The deepest above-split leaf: path below^(depth-1), then above.
            let node = codes.len() - 1;
            let d = depth;
            assert_eq!(bit(node, w - 1 - d), 1, "marker");
            for j in 1..=d {
                assert_eq!(bit(node, w - 1 - d + j), u32::from(j < d), "step {j}");
            }
            for b in 0..w - 1 - d {
                assert_eq!(bit(node, b), 0);
            }
        }
    }

    #[test]
    fn every_deep_key_matches_a_u128_recurrence() {
        let trees = [chain_top(33).codes, chain_top(70).codes, ragged(45, 7, 4)];
        for codes in &trees {
            let n = codes.len();
            let w = depths(codes).into_iter().max().map_or(1, |d| d + 1);
            assert!(w > 33, "depth {}", w - 1);
            let top = 1u128 << (w - 1);
            let mut keys = vec![0u128; n];
            keys[0] = top;
            let mut inner = 0usize;
            for (v, &code) in codes.iter().enumerate() {
                if code != LEAF {
                    inner += 1;
                    if 2 * inner < n {
                        keys[2 * inner - 1] = (keys[v] >> 1) | top;
                        keys[2 * inner] = keys[v] >> 1;
                    }
                }
            }
            let (width, words) = node_keys(codes);
            assert_eq!(width, w);
            assert_eq!(words.len(), bits::packed_words(n, w));
            for (node, key) in keys.iter().enumerate() {
                for b in 0..w as usize {
                    let at = node * w as usize + b;
                    let got = words[at >> 5] >> (at & 31) & 1;
                    assert_eq!(u128::from(got), key >> b & 1, "node {node} bit {b}");
                }
            }
        }
    }

    #[test]
    fn deep_top_trees_write_and_read_back() {
        for depth in [33usize, 70] {
            let top = chain_top(depth);
            let n = top.codes.len();
            let t = tome(top, vec![None; n], 0);
            let bytes = write(&t);
            let width = u32::from_le_bytes(bytes[0x94..0x98].try_into().unwrap());
            assert_eq!(width as usize, depth + 1);
            assert_eq!(crate::read(&bytes), Ok(t));
        }
    }
}
