//! The per-tile cache of an incremental re-bake (M3c).
//!
//! The tile stage is about 98 % of a bake. Each tile's two results, pass A's [`Flood`] and pass
//! B's [`TileOut`], are pure functions of a small set of inputs, so [`TileCache`] keeps every
//! tile's results under a 128-bit hash of exactly those inputs, and [`super::bake`]
//! recomputes only the tiles whose key changed. The serial global stage (the flood join, cross-tile
//! faces, unreachable-cell removal, collapse, the tome) always runs in full.
//!
//! Key material, beside a signature of the grid (origin, voxel counts, tile counts, voxel edge,
//! brick layout) and of the tile-stage [`super::Params`] (solid and cell modes, tile, voxel,
//! `surface_occludes`, `max_cells_per_tile`, `cell_merge`, `midpoint_views`, `view_mask`), which
//! every key carries so a change of either misses every tile:
//!
//! * **Pass A:** the tile coordinate; the tile's bucketed triangles as a sorted set of their
//!   corners' bit patterns (voxelisation ORs triangles, so their order and duplicates do not
//!   matter); the tile's snapped seeds; and whether no seeds were given at all.
//! * **Pass B:** the tile's pass-A key; in flood mode its post-join liveness and, per face, the
//!   neighbour's pass-A key and liveness (or none); in parity mode a hash of the whole mesh (rays
//!   cross all of it); with the view mask, the view volume's voxel range in the tile; the tile's
//!   level shift (`Ctx::level_shift`, 0 unless the tile lies outside the view volume); and the
//!   boxes of the targets that reach the tile (grown by one voxel), in input order. Object lists are stored as positions in that list and remapped to input indices
//!   on use, so an edit to a target elsewhere leaves this tile's entry valid.
//!
//! The global-only settings (`base_distance`, `reachability_threshold`, `collapse_size`,
//! `empty_leaves`, `global_groups`), `threads` and `memory_limit` enter no key.
//!
//! Byte layout of [`TileCache::to_bytes`] (little endian):
//!
//! | Offset | Field |
//! | --- | --- |
//! | 0 | magic `TBTC` |
//! | 4 | u32 version ([`VERSION`]) |
//! | 8 | u128 checksum: FNV-1a 128 of every byte from offset 24 to the end |
//! | 24 | u32 pass-A entry count, u32 pass-B entry count |
//! | 32 | entries, pass A then pass B, each in ascending key order: u128 key, u64 length, payload |
//!
//! A payload is one tile's encoded result; its layout is internal to this version.

use std::collections::BTreeMap;

use super::grid::{Layer, SOLID};
use super::tile::{self, BOUNDARY, CellOut, Ctx, FLEX, Face, Flood, LABEL, Node, TileOut};
use super::{CellMode, SolidMode};

/// The cache bytes' magic.
pub(super) const MAGIC: [u8; 4] = *b"TBTC";
/// The cache bytes' version. Bump it whenever a key's material or a payload's layout changes,
/// or a tile pass changes its result; the test `cache_bytes_are_pinned_to_the_cache_version`
/// fails on any such change.
pub(super) const VERSION: u32 = 6;

/// Per-tile results of one bake, keyed by the inputs they depend on. Build one from an earlier
/// bake's bytes with [`TileCache::from_bytes`], pass it to [`super::bake`], and keep the
/// cache that call returns for the next bake. A returned cache holds the entries of its own bake
/// only, so it never grows past one bake's tiles.
#[derive(Clone, Debug, Default)]
pub(super) struct TileCache {
    flood: BTreeMap<u128, Vec<u8>>,
    tiles: BTreeMap<u128, Vec<u8>>,
    rejected: bool,
}

impl TileCache {
    /// An empty cache: every tile misses.
    pub(super) fn new() -> TileCache {
        TileCache::default()
    }

    /// Parses cache bytes. Bytes that are truncated, carry another magic or version, or fail
    /// the checksum give an empty cache marked [`TileCache::rejected`]; this never fails.
    pub(super) fn from_bytes(bytes: &[u8]) -> TileCache {
        parse(bytes).unwrap_or(TileCache {
            rejected: true,
            ..TileCache::default()
        })
    }

    /// The cache as bytes (layout in the module documentation). Deterministic: the same
    /// entries give the same bytes.
    pub(super) fn to_bytes(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&(self.flood.len() as u32).to_le_bytes());
        body.extend_from_slice(&(self.tiles.len() as u32).to_le_bytes());
        for map in [&self.flood, &self.tiles] {
            for (key, payload) in map {
                body.extend_from_slice(&key.to_le_bytes());
                body.extend_from_slice(&(payload.len() as u64).to_le_bytes());
                body.extend_from_slice(payload);
            }
        }
        let mut h = Hasher::new();
        h.bytes(&body);
        let mut out = Vec::with_capacity(24 + body.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&h.finish().to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Whether these bytes were refused by [`TileCache::from_bytes`] (the cache is then empty).
    pub(super) fn rejected(&self) -> bool {
        self.rejected
    }

    /// Entries per level: pass A (flood; none in parity mode), pass B. For a cache a bake
    /// returned, these are its tile-grid tile counts.
    #[cfg(test)]
    pub(super) fn entries(&self) -> [usize; 2] {
        [self.flood.len(), self.tiles.len()]
    }

    pub(super) fn flood_entry(&self, key: u128) -> Option<&[u8]> {
        self.flood.get(&key).map(Vec::as_slice)
    }

    pub(super) fn tile_entry(&self, key: u128) -> Option<&[u8]> {
        self.tiles.get(&key).map(Vec::as_slice)
    }

    pub(super) fn insert_flood(&mut self, key: u128, payload: Vec<u8>) {
        self.flood.insert(key, payload);
    }

    pub(super) fn insert_tile(&mut self, key: u128, payload: Vec<u8>) {
        self.tiles.insert(key, payload);
    }

    /// Applies `f` to every payload, keeping the keys (tests corrupt payloads with it).
    #[cfg(test)]
    pub(super) fn map_payloads(&mut self, mut f: impl FnMut(&mut Vec<u8>)) {
        self.flood
            .values_mut()
            .chain(self.tiles.values_mut())
            .for_each(&mut f);
    }
}

fn parse(bytes: &[u8]) -> Option<TileCache> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != MAGIC || r.u32()? != VERSION {
        return None;
    }
    let sum = r.u128()?;
    let mut h = Hasher::new();
    h.bytes(&bytes[r.at..]);
    if h.finish() != sum {
        return None;
    }
    let counts = [r.u32()? as usize, r.u32()? as usize];
    let mut cache = TileCache::default();
    for (level, &count) in counts.iter().enumerate() {
        let mut last = None;
        for _ in 0..count {
            let key = r.u128()?;
            // Keys are unique and ascending, as written.
            if last.is_some_and(|l| l >= key) {
                return None;
            }
            last = Some(key);
            let len = r.len()?;
            let payload = r.take(len)?.to_vec();
            if level == 0 {
                cache.flood.insert(key, payload);
            } else {
                cache.tiles.insert(key, payload);
            }
        }
    }
    r.done().then_some(cache)
}

/// FNV-1a with 128-bit state, over bytes. Values enter as little-endian bit patterns.
pub(super) struct Hasher(u128);

const FNV_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

impl Hasher {
    pub(super) fn new() -> Hasher {
        Hasher(FNV_OFFSET)
    }
    pub(super) fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u128::from(x)).wrapping_mul(FNV_PRIME);
        }
    }
    pub(super) fn u8(&mut self, x: u8) {
        self.bytes(&[x]);
    }
    pub(super) fn bool(&mut self, x: bool) {
        self.u8(u8::from(x));
    }
    pub(super) fn u32(&mut self, x: u32) {
        self.bytes(&x.to_le_bytes());
    }
    pub(super) fn u64(&mut self, x: u64) {
        self.bytes(&x.to_le_bytes());
    }
    pub(super) fn usize(&mut self, x: usize) {
        self.u64(x as u64);
    }
    pub(super) fn u128(&mut self, x: u128) {
        self.bytes(&x.to_le_bytes());
    }
    pub(super) fn f32(&mut self, x: f32) {
        self.u32(x.to_bits());
    }
    pub(super) fn finish(&self) -> u128 {
        self.0
    }
}

/// The per-bake part of every key.
pub(super) struct Keys {
    /// Grid and tile-stage settings.
    base: u128,
    /// Parity mode: the whole mesh.
    mesh: Option<u128>,
}

impl Keys {
    pub(super) fn new(ctx: &Ctx<'_>) -> Keys {
        let mut h = Hasher::new();
        h.bytes(&MAGIC);
        h.u32(VERSION);
        let g = &ctx.grid;
        for a in 0..3 {
            h.f32(g.origin[a]);
            h.usize(g.dims[a]);
            h.usize(ctx.tiles_n[a]);
        }
        h.f32(g.voxel);
        h.u32(ctx.layout.shift);
        h.usize(ctx.layout.n);
        let p = &ctx.input.params;
        h.u8(match p.solid {
            SolidMode::Flood => 0,
            SolidMode::Parity => 1,
        });
        match p.cells {
            CellMode::Boxes => h.u8(0),
            CellMode::Components { block } => {
                h.u8(1);
                h.f32(block);
            }
            CellMode::Merged { fill, block } => {
                h.u8(2);
                h.f32(fill);
                h.f32(block);
            }
        }
        h.f32(p.tile);
        h.f32(p.voxel);
        h.bool(p.surface_occludes);
        h.usize(p.max_cells_per_tile);
        h.f32(p.cell_merge);
        h.bool(p.midpoint_views);
        h.bool(p.view_mask);
        let mesh = ctx.soup.as_deref().map(|m| {
            let mut h = Hasher::new();
            h.usize(m.vertices.len());
            for v in &m.vertices {
                v.iter().for_each(|x| h.f32(*x));
            }
            h.usize(m.triangles.len());
            for t in &m.triangles {
                t.iter().for_each(|i| h.u32(*i));
            }
            h.finish()
        });
        Keys {
            base: h.finish(),
            mesh,
        }
    }

    /// Pass A's key of tile `t`.
    pub(super) fn a(&self, ctx: &Ctx<'_>, t: usize) -> u128 {
        let mut h = Hasher::new();
        h.u8(b'A');
        h.u128(self.base);
        for r in &ctx.tile_ranges[t] {
            h.usize(r[0]);
            h.usize(r[1]);
        }
        let mut tris: Vec<[u32; 9]> = ctx.buckets[t]
            .iter()
            .map(|&ti| {
                let c = ctx.tris.corners(ti);
                std::array::from_fn(|k| c[k / 3][k % 3].to_bits())
            })
            .collect();
        tris.sort_unstable();
        tris.dedup();
        h.usize(tris.len());
        for tri in &tris {
            tri.iter().for_each(|x| h.u32(*x));
        }
        let (seeds, none) = tile::starts(ctx, t);
        h.usize(seeds.len());
        seeds.iter().for_each(|s| h.usize(*s));
        h.bool(none);
        h.finish()
    }

    /// Pass B's key of tile `t`, from every tile's pass-A key, the post-join liveness (flood
    /// mode) and the tile's [`tile::overlaps`] list.
    pub(super) fn b(
        &self,
        ctx: &Ctx<'_>,
        t: usize,
        a_keys: &[u128],
        live: &[Vec<bool>],
        overlap: &[u32],
    ) -> u128 {
        let mut h = Hasher::new();
        h.u8(b'B');
        h.u128(self.base);
        h.u128(a_keys[t]);
        let liveness = |h: &mut Hasher, l: &[bool]| {
            h.usize(l.len());
            l.iter().for_each(|x| h.bool(*x));
        };
        match self.mesh {
            Some(mesh) => {
                h.u8(1);
                h.u128(mesh);
            }
            None => {
                h.u8(0);
                liveness(&mut h, &live[t]);
                for f in 0..6 {
                    match ctx.neighbour(t, f / 2, f % 2) {
                        Some(n) => {
                            h.u8(1);
                            h.u128(a_keys[n]);
                            liveness(&mut h, &live[n]);
                        }
                        None => h.u8(0),
                    }
                }
            }
        }
        if let Some(rg) = tile::view_range(ctx, t) {
            rg.iter().flatten().for_each(|k| h.usize(*k));
        }
        h.u32(ctx.level_shift(t));
        h.usize(overlap.len());
        for &o in overlap {
            let b = &ctx.input.targets[o as usize].bounds;
            b.min.iter().chain(&b.max).for_each(|x| h.f32(*x));
        }
        h.finish()
    }
}

/// Little-endian payload writer.
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, x: u8) {
        self.0.push(x);
    }
    fn u32(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn u64(&mut self, x: u64) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn usize(&mut self, x: usize) {
        self.u64(x as u64);
    }
    fn f32(&mut self, x: f32) {
        self.u32(x.to_bits());
    }
    fn layer(&mut self, l: &Layer) {
        self.u32(l.len);
        self.usize(l.runs.len());
        for &(s, v) in &l.runs {
            self.u32(s);
            self.u32(v);
        }
    }
}

/// Little-endian reader; every read fails rather than panics on short input.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, at: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }
    fn len(&mut self) -> Option<usize> {
        usize::try_from(self.u64()?).ok()
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    /// A count of items of at least `min` bytes each, refused when the rest cannot hold them.
    fn count(&mut self, min: usize) -> Option<usize> {
        let n = self.len()?;
        (n.checked_mul(min)? <= self.b.len() - self.at).then_some(n)
    }
    /// A layer of `len` positions whose values all pass `ok`.
    fn layer(&mut self, len: u32, ok: impl Fn(u32) -> bool) -> Option<Layer> {
        if self.u32()? != len {
            return None;
        }
        let n = self.count(8)?;
        let mut runs = Vec::with_capacity(n);
        for i in 0..n {
            let (s, v) = (self.u32()?, self.u32()?);
            let ordered = if i == 0 {
                s == 0
            } else {
                runs.last().is_some_and(|r: &(u32, u32)| r.0 < s)
            };
            if !ordered || s >= len || !ok(v) {
                return None;
            }
            runs.push((s, v));
        }
        (n > 0 || len == 0).then_some(Layer { runs, len })
    }
    fn done(&self) -> bool {
        self.at == self.b.len()
    }
}

/// Positions per tile face.
fn face_len(ctx: &Ctx<'_>) -> Option<u32> {
    let size = ctx.layout.tile();
    u32::try_from(size.checked_mul(size)?).ok()
}

pub(super) fn encode_flood(f: &Flood) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.usize(f.labels);
    w.usize(f.seeded.len());
    f.seeded.iter().for_each(|x| w.u32(*x));
    f.layers.iter().for_each(|l| w.layer(l));
    w.usize(f.bytes);
    w.u64(f.layer_bytes);
    w.0
}

/// Decodes a pass-A payload, checking every label against the label count, so a payload that
/// does not fit this bake is a miss and never a panic.
pub(super) fn decode_flood(ctx: &Ctx<'_>, b: &[u8]) -> Option<Flood> {
    let face = face_len(ctx)?;
    let mut r = Reader::new(b);
    let labels = r.len()?;
    if labels > LABEL as usize {
        return None;
    }
    let n = r.count(4)?;
    let mut seeded = Vec::with_capacity(n);
    for _ in 0..n {
        let l = r.u32()?;
        if l as usize >= labels {
            return None;
        }
        seeded.push(l);
    }
    let ok = |x: u32| x & LABEL == LABEL || ((x & LABEL) as usize) < labels;
    let mut layers: [Layer; 6] = Default::default();
    for l in &mut layers {
        *l = r.layer(face, ok)?;
    }
    let bytes = r.len()?;
    let layer_bytes = r.u64()?;
    r.done().then_some(Flood {
        labels,
        seeded,
        layers,
        bytes,
        layer_bytes,
    })
}

pub(super) fn encode_tile(o: &TileOut) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.usize(o.cells.len());
    for c in &o.cells {
        c.lo.iter().chain(&c.hi).for_each(|x| w.usize(*x));
        w.u64(c.voxels);
        w.usize(c.samples.len());
        c.samples.iter().flatten().for_each(|x| w.f32(*x));
    }
    w.usize(o.faces.len());
    for f in &o.faces {
        w.u32(f.c);
        w.u8(f.axis);
        w.u8(f.side);
        w.u32(f.nb);
        w.u32(f.k);
        f.rect.iter().for_each(|x| w.u32(*x));
    }
    o.layers.iter().for_each(|l| w.layer(l));
    w.usize(o.choices.len());
    for list in &o.choices {
        w.usize(list.len());
        list.iter().for_each(|c| w.u32(*c));
    }
    w.usize(o.tree.len());
    for node in &o.tree {
        match *node {
            Node::Leaf(c) => {
                w.u8(0);
                w.u32(c);
            }
            Node::Split { axis, at, kids } => {
                w.u8(1);
                w.u8(axis);
                w.f32(at);
                w.u32(kids[0]);
                w.u32(kids[1]);
            }
        }
    }
    w.usize(o.objects.len());
    for &(c, p) in &o.objects {
        w.u32(c);
        w.u32(p);
    }
    w.usize(o.surface);
    w.usize(o.solid);
    w.usize(o.bytes);
    w.u64(o.layer_bytes);
    w.0
}

/// Decodes a pass-B payload for a tile with `targets` overlapping targets, checking every cell,
/// node and position index, so a payload that does not fit this bake is a miss and never a
/// panic.
pub(super) fn decode_tile(ctx: &Ctx<'_>, b: &[u8], targets: usize) -> Option<TileOut> {
    let face = face_len(ctx)?;
    let dims = ctx.grid.dims;
    let mut r = Reader::new(b);
    let n = r.count(64)?;
    if n >= SOLID as usize {
        return None;
    }
    let mut cells = Vec::with_capacity(n);
    for _ in 0..n {
        let mut v = [0usize; 6];
        for x in &mut v {
            *x = r.len()?;
        }
        let (lo, hi) = ([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
        if (0..3).any(|a| lo[a] >= hi[a] || hi[a] > dims[a]) {
            return None;
        }
        let voxels = r.u64()?;
        let k = r.count(12)?;
        let mut samples = Vec::with_capacity(k);
        for _ in 0..k {
            samples.push([r.f32()?, r.f32()?, r.f32()?]);
        }
        cells.push(CellOut {
            lo,
            hi,
            voxels,
            samples,
        });
    }
    let cell = |c: u32| (c as usize) < n;
    let m = r.count(30)?;
    let mut faces = Vec::with_capacity(m);
    for _ in 0..m {
        let (c, axis, side, nb, k) = (r.u32()?, r.u8()?, r.u8()?, r.u32()?, r.u32()?);
        let rect = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        if !cell(c) || axis >= 3 || side >= 2 || !(nb == BOUNDARY || cell(nb)) {
            return None;
        }
        faces.push(Face {
            c,
            axis,
            side,
            nb,
            k,
            rect,
        });
    }
    let mut layers: [Layer; 6] = Default::default();
    for l in &mut layers {
        *l = r.layer(face, |x| x == SOLID || cell(x))?;
    }
    let m = r.count(1)?;
    let mut choices = Vec::with_capacity(m);
    for _ in 0..m {
        let k = r.count(4)?;
        let mut list = Vec::with_capacity(k);
        for _ in 0..k {
            let c = r.u32()?;
            if !cell(c) {
                return None;
            }
            list.push(c);
        }
        if list.len() < 2 {
            return None;
        }
        choices.push(list);
    }
    let m = r.count(5)?;
    let mut tree = Vec::with_capacity(m);
    for i in 0..m {
        let node = match r.u8()? {
            0 => {
                let c = r.u32()?;
                let flex = c != SOLID && c & FLEX != 0 && ((c & !FLEX) as usize) < choices.len();
                if !(c == SOLID || cell(c) || flex) {
                    return None;
                }
                Node::Leaf(c)
            }
            1 => {
                let (axis, at, kids) = (r.u8()?, r.f32()?, [r.u32()?, r.u32()?]);
                // Children follow their parent.
                let kid = |k: u32| (k as usize) > i && (k as usize) < m;
                if axis >= 3 || !kid(kids[0]) || !kid(kids[1]) {
                    return None;
                }
                Node::Split { axis, at, kids }
            }
            _ => return None,
        };
        tree.push(node);
    }
    if tree.is_empty() {
        return None;
    }
    let m = r.count(8)?;
    let mut objects = Vec::with_capacity(m);
    for _ in 0..m {
        let (c, p) = (r.u32()?, r.u32()?);
        if !cell(c) || p as usize >= targets {
            return None;
        }
        objects.push((c, p));
    }
    let surface = r.len()?;
    let solid = r.len()?;
    let bytes = r.len()?;
    let layer_bytes = r.u64()?;
    r.done().then_some(TileOut {
        cells,
        faces,
        layers,
        choices,
        tree,
        objects,
        surface,
        solid,
        bytes,
        layer_bytes,
    })
}
