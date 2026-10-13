//! Storage for the tile-streamed bake: the voxel grid's coordinates, brick-sparse per-tile fields,
//! run-length boundary layers and a union-find.
//!
//! A tile's voxels are stored as cubic bricks of `2^shift` voxels per edge. A brick is either one
//! value for all of its voxels or a dense array, so memory follows the bricks geometry touches and
//! not the tile's volume. Bools pack into `u64` words.

use tenebrae_geom::Vec3;

/// No cell: solid space, or (in a cell layer) a solid voxel.
pub(super) const SOLID: u32 = u32::MAX;

/// The whole voxel grid. Coordinates are always computed from global voxel indices, so a tile
/// derives exactly the floats the whole-grid baker did.
#[derive(Clone, Copy, Debug)]
pub(super) struct Grid {
    pub(super) origin: Vec3,
    pub(super) voxel: f32,
    pub(super) dims: [usize; 3],
}

impl Grid {
    pub(super) fn centre(&self, v: [usize; 3]) -> Vec3 {
        [
            self.origin[0] + (v[0] as f32 + 0.5) * self.voxel,
            self.origin[1] + (v[1] as f32 + 0.5) * self.voxel,
            self.origin[2] + (v[2] as f32 + 0.5) * self.voxel,
        ]
    }
    pub(super) fn coord(&self, axis: usize, k: usize) -> f32 {
        self.origin[axis] + k as f32 * self.voxel
    }
    /// Voxel range `[lo, hi)` covering world interval `[a, b]` on `axis`, clamped.
    pub(super) fn range(&self, axis: usize, a: f32, b: f32) -> (usize, usize) {
        let lo = ((a - self.origin[axis]) / self.voxel).floor().max(0.0) as usize;
        let hi = (((b - self.origin[axis]) / self.voxel).floor() as i64 + 1)
            .clamp(0, self.dims[axis] as i64) as usize;
        (lo.min(self.dims[axis]), hi)
    }
}

/// Brick layout of one tile: `n` bricks per axis, `1 << shift` voxels per brick edge.
#[derive(Clone, Copy, Debug)]
pub(super) struct Bricks {
    pub(super) shift: u32,
    pub(super) n: usize,
}

impl Bricks {
    pub(super) fn edge(&self) -> usize {
        1 << self.shift
    }
    /// Voxels per brick.
    pub(super) fn volume(&self) -> usize {
        1 << (3 * self.shift)
    }
    pub(super) fn count(&self) -> usize {
        self.n * self.n * self.n
    }
    /// Voxels per tile edge.
    pub(super) fn tile(&self) -> usize {
        self.n << self.shift
    }
    pub(super) fn index(&self, b: [usize; 3]) -> usize {
        (b[2] * self.n + b[1]) * self.n + b[0]
    }
    pub(super) fn coords(&self, i: usize) -> [usize; 3] {
        [i % self.n, (i / self.n) % self.n, i / (self.n * self.n)]
    }
    /// Brick index and in-brick index of tile-local voxel `v`.
    pub(super) fn locate(&self, v: [usize; 3]) -> (usize, usize) {
        let s = self.shift;
        let m = self.edge() - 1;
        let b = self.index([v[0] >> s, v[1] >> s, v[2] >> s]);
        let l = ((((v[2] & m) << s) | (v[1] & m)) << s) | (v[0] & m);
        (b, l)
    }
    /// In-brick voxel coordinates of in-brick index `l`.
    pub(super) fn local(&self, l: usize) -> [usize; 3] {
        let s = self.shift;
        let m = self.edge() - 1;
        [l & m, (l >> s) & m, l >> (2 * s)]
    }
    /// In-brick index of in-brick coordinates.
    pub(super) fn inner(&self, v: [usize; 3]) -> usize {
        let s = self.shift;
        (((v[2] << s) | v[1]) << s) | v[0]
    }
    /// Words of a dense bit brick.
    pub(super) fn words(&self) -> usize {
        self.volume().div_ceil(64)
    }
}

/// One brick of a field.
#[derive(Clone, Debug)]
pub(super) enum Brick<T> {
    /// Every voxel holds this value.
    Uniform(T),
    /// One value per voxel, in-brick index order (x fastest).
    Dense(Box<[T]>),
    /// At most 256 distinct values: a palette and one palette index per voxel.
    Packed(Box<[T]>, Box<[u8]>),
}

impl<T: Copy + PartialEq> Brick<T> {
    /// The smallest brick holding `values` (one per voxel): uniform, packed or dense.
    pub(super) fn of(values: Vec<T>) -> Brick<T> {
        let mut palette: Vec<T> = Vec::new();
        let mut index = Vec::with_capacity(values.len());
        let mut last = 0usize;
        for &x in &values {
            if palette.get(last) != Some(&x) {
                match palette.iter().position(|p| *p == x) {
                    Some(i) => last = i,
                    None if palette.len() < 256 => {
                        palette.push(x);
                        last = palette.len() - 1;
                    }
                    None => return Brick::Dense(values.into_boxed_slice()),
                }
            }
            index.push(last as u8);
        }
        if palette.len() == 1 {
            return Brick::Uniform(palette[0]);
        }
        Brick::Packed(palette.into_boxed_slice(), index.into_boxed_slice())
    }
}

/// A per-voxel value over one tile, brick-sparse.
#[derive(Clone, Debug)]
pub(super) struct Field<T> {
    pub(super) layout: Bricks,
    pub(super) bricks: Vec<Brick<T>>,
}

impl<T: Copy + PartialEq> Field<T> {
    pub(super) fn new(layout: Bricks, fill: T) -> Self {
        Field {
            layout,
            bricks: (0..layout.count()).map(|_| Brick::Uniform(fill)).collect(),
        }
    }
    pub(super) fn get(&self, v: [usize; 3]) -> T {
        let (b, l) = self.layout.locate(v);
        self.at(b, l)
    }
    pub(super) fn at(&self, b: usize, l: usize) -> T {
        match &self.bricks[b] {
            Brick::Uniform(x) => *x,
            Brick::Dense(d) => d[l],
            Brick::Packed(p, i) => p[i[l] as usize],
        }
    }
    pub(super) fn set(&mut self, v: [usize; 3], x: T) {
        let (b, l) = self.layout.locate(v);
        self.set_at(b, l, x);
    }
    pub(super) fn set_at(&mut self, b: usize, l: usize, x: T) {
        let volume = self.layout.volume();
        let brick = &mut self.bricks[b];
        match brick {
            Brick::Uniform(u) => {
                if *u == x {
                    return;
                }
                *brick = Brick::Dense(vec![*u; volume].into_boxed_slice());
            }
            Brick::Packed(p, i) => {
                let d: Vec<T> = i.iter().map(|k| p[*k as usize]).collect();
                *brick = Brick::Dense(d.into_boxed_slice());
            }
            Brick::Dense(_) => {}
        }
        if let Brick::Dense(d) = brick {
            d[l] = x;
        }
    }
    pub(super) fn uniform(&self, b: usize) -> Option<T> {
        match &self.bricks[b] {
            Brick::Uniform(x) => Some(*x),
            _ => None,
        }
    }
    /// Stores every non-uniform brick in its smallest form.
    pub(super) fn compact(&mut self) {
        for brick in &mut self.bricks {
            if let Brick::Dense(d) = brick {
                *brick = Brick::of(d.to_vec());
            } else if let Brick::Packed(p, i) = brick {
                let d: Vec<T> = i.iter().map(|k| p[*k as usize]).collect();
                *brick = Brick::of(d);
            }
        }
    }
    /// Applies `f` to every value.
    pub(super) fn map(&mut self, mut f: impl FnMut(T) -> T) {
        for brick in &mut self.bricks {
            match brick {
                Brick::Uniform(x) => *x = f(*x),
                Brick::Dense(d) => d.iter_mut().for_each(|x| *x = f(*x)),
                Brick::Packed(p, _) => p.iter_mut().for_each(|x| *x = f(*x)),
            }
        }
        // Distinct values may have become equal.
        for brick in &mut self.bricks {
            if let Brick::Packed(p, i) = brick
                && p.iter().all(|x| *x == p[0])
            {
                let x = p[0];
                let _ = i;
                *brick = Brick::Uniform(x);
            }
        }
    }
    /// Calls `f(value, voxels)` for the values of brick `b`.
    pub(super) fn counts(&self, b: usize, mut f: impl FnMut(T, usize)) {
        match &self.bricks[b] {
            Brick::Uniform(x) => f(*x, self.layout.volume()),
            Brick::Dense(d) => d.iter().for_each(|x| f(*x, 1)),
            Brick::Packed(p, i) => {
                let mut n = [0usize; 256];
                for k in i.iter() {
                    n[*k as usize] += 1;
                }
                for (x, c) in p.iter().zip(n) {
                    if c > 0 {
                        f(*x, c);
                    }
                }
            }
        }
    }
    pub(super) fn bytes(&self) -> usize {
        let volume = self.layout.volume();
        self.layout.count() * size_of::<Brick<T>>()
            + self
                .bricks
                .iter()
                .map(|b| match b {
                    Brick::Uniform(_) => 0,
                    Brick::Dense(_) => volume * size_of::<T>(),
                    Brick::Packed(p, _) => p.len() * size_of::<T>() + volume,
                })
                .sum::<usize>()
    }
}

/// Bytes of a value field with `dense` dense bricks.
pub(super) fn field_bytes<T>(layout: Bricks, dense: usize) -> usize {
    layout
        .count()
        .saturating_mul(size_of::<Brick<T>>())
        .saturating_add(
            dense
                .saturating_mul(layout.volume())
                .saturating_mul(size_of::<T>()),
        )
}

/// Bytes of a bit field with `dense` dense bricks.
pub(super) fn bits_bytes(layout: Bricks, dense: usize) -> usize {
    layout
        .count()
        .saturating_mul(size_of::<BitBrick>())
        .saturating_add(dense.saturating_mul(layout.words()).saturating_mul(8))
}

/// One brick of a bit field.
#[derive(Clone, Debug)]
pub(super) enum BitBrick {
    /// Every voxel holds this value.
    Uniform(bool),
    /// One bit per voxel, in-brick index order.
    Dense(Box<[u64]>),
}

/// A per-voxel bool over one tile, brick-sparse, dense bricks packed in `u64` words.
#[derive(Clone, Debug)]
pub(super) struct Bits {
    pub(super) layout: Bricks,
    pub(super) bricks: Vec<BitBrick>,
}

impl Bits {
    pub(super) fn new(layout: Bricks, fill: bool) -> Self {
        Bits {
            layout,
            bricks: (0..layout.count())
                .map(|_| BitBrick::Uniform(fill))
                .collect(),
        }
    }
    pub(super) fn get(&self, v: [usize; 3]) -> bool {
        let (b, l) = self.layout.locate(v);
        self.at(b, l)
    }
    pub(super) fn at(&self, b: usize, l: usize) -> bool {
        match &self.bricks[b] {
            BitBrick::Uniform(x) => *x,
            BitBrick::Dense(w) => (w[l >> 6] >> (l & 63)) & 1 == 1,
        }
    }
    pub(super) fn set(&mut self, v: [usize; 3], x: bool) {
        let (b, l) = self.layout.locate(v);
        self.set_at(b, l, x);
    }
    pub(super) fn set_at(&mut self, b: usize, l: usize, x: bool) {
        let n = self.layout.words();
        let brick = &mut self.bricks[b];
        if let BitBrick::Uniform(u) = brick {
            if *u == x {
                return;
            }
            // Bits past the brick's volume (bricks under 64 voxels) are never read.
            let fill = if *u { u64::MAX } else { 0 };
            *brick = BitBrick::Dense(vec![fill; n].into_boxed_slice());
        }
        if let BitBrick::Dense(w) = brick {
            if x {
                w[l >> 6] |= 1 << (l & 63);
            } else {
                w[l >> 6] &= !(1 << (l & 63));
            }
        }
    }
    pub(super) fn uniform(&self, b: usize) -> Option<bool> {
        match &self.bricks[b] {
            BitBrick::Uniform(x) => Some(*x),
            BitBrick::Dense(_) => None,
        }
    }
    /// Turns dense bricks whose voxels all agree into uniform ones.
    pub(super) fn compact(&mut self) {
        let volume = self.layout.volume();
        for brick in &mut self.bricks {
            if let BitBrick::Dense(w) = brick {
                let ones = count_bits(w, volume);
                if ones == 0 {
                    *brick = BitBrick::Uniform(false);
                } else if ones == volume {
                    *brick = BitBrick::Uniform(true);
                }
            }
        }
    }
    pub(super) fn dense(&self) -> usize {
        self.bricks
            .iter()
            .filter(|b| matches!(b, BitBrick::Dense(_)))
            .count()
    }
    pub(super) fn bytes(&self) -> usize {
        bits_bytes(self.layout, self.dense())
    }
    /// Set voxels.
    pub(super) fn count(&self) -> usize {
        let volume = self.layout.volume();
        self.bricks
            .iter()
            .map(|brick| match brick {
                BitBrick::Uniform(true) => volume,
                BitBrick::Uniform(false) => 0,
                BitBrick::Dense(w) => count_bits(w, volume),
            })
            .sum()
    }
}

/// Set bits among the first `volume` bits of `w`.
fn count_bits(w: &[u64], volume: usize) -> usize {
    let mut n = 0usize;
    for (i, word) in w.iter().enumerate() {
        let used = (volume - 64 * i).min(64);
        let mask = if used == 64 {
            u64::MAX
        } else {
            (1u64 << used) - 1
        };
        n += (word & mask).count_ones() as usize;
    }
    n
}

/// Run-length values over one face of a tile, row-major: `a1 = (axis + 1) % 3` fastest, then
/// `a2 = (axis + 2) % 3`. Each run is `(first position, value)`.
#[derive(Clone, Debug, Default)]
pub(super) struct Layer {
    pub(super) runs: Vec<(u32, u32)>,
    pub(super) len: u32,
}

impl Layer {
    pub(super) fn push(&mut self, value: u32, count: usize) {
        if self.runs.last().is_none_or(|r| r.1 != value) {
            self.runs.push((self.len, value));
        }
        self.len += count as u32;
    }
    /// Value at position `p`.
    pub(super) fn at(&self, p: u32) -> u32 {
        let i = self.runs.partition_point(|r| r.0 <= p);
        self.runs[i - 1].1
    }
    /// Calls `f(start, end, value)` for each run.
    pub(super) fn each(&self, mut f: impl FnMut(u32, u32, u32)) {
        for (i, r) in self.runs.iter().enumerate() {
            let end = self.runs.get(i + 1).map_or(self.len, |n| n.0);
            f(r.0, end, r.1);
        }
    }
    pub(super) fn bytes(&self) -> usize {
        self.runs.capacity() * 8 + size_of::<Layer>()
    }
}

/// Calls `f(start, end, a, b)` over the maximal position ranges where both layers are constant.
pub(super) fn zip(a: &Layer, b: &Layer, mut f: impl FnMut(u32, u32, u32, u32)) {
    let (mut i, mut j) = (0usize, 0usize);
    let mut p = 0u32;
    let len = a.len.min(b.len);
    while p < len {
        let ea = a.runs.get(i + 1).map_or(a.len, |r| r.0);
        let eb = b.runs.get(j + 1).map_or(b.len, |r| r.0);
        let e = ea.min(eb);
        f(p, e, a.runs[i].1, b.runs[j].1);
        p = e;
        if ea == e {
            i += 1;
        }
        if eb == e {
            j += 1;
        }
    }
}

/// Rectangles exactly covering layer positions `[start, end)` in rows of `row`: the partial first
/// row, the whole rows between and the partial last row.
pub(super) fn run_rects(start: u32, end: u32, row: u32) -> impl Iterator<Item = [u32; 4]> {
    let (r0, r1) = (start / row, (end - 1) / row);
    let (c0, c1) = (start % row, (end - 1) % row + 1);
    let parts = if r0 == r1 {
        [Some([c0, c1, r0, r0 + 1]), None, None]
    } else {
        let first = if c0 == 0 { r0 } else { r0 + 1 };
        let last = if c1 == row { r1 + 1 } else { r1 };
        [
            (c0 != 0).then_some([c0, row, r0, r0 + 1]),
            (first < last).then_some([0, row, first, last]),
            (c1 != row).then_some([0, c1, r1, r1 + 1]),
        ]
    };
    parts.into_iter().flatten()
}

/// Builds the layer of face `(axis, side)` of a tile: `whole(brick)` gives a brick's value when it
/// is the same over the brick's part of the face, `voxel(v)` the value of tile-local voxel `v`.
pub(super) fn face_layer(
    layout: Bricks,
    axis: usize,
    side: usize,
    whole: impl Fn(usize) -> Option<u32>,
    voxel: impl Fn([usize; 3]) -> u32,
) -> Layer {
    let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
    let size = layout.tile();
    let edge = layout.edge();
    let at = if side == 1 { size - 1 } else { 0 };
    let mut layer = Layer::default();
    for j2 in 0..size {
        for k1 in 0..layout.n {
            let mut b = [0usize; 3];
            b[axis] = at >> layout.shift;
            b[a1] = k1;
            b[a2] = j2 >> layout.shift;
            if let Some(x) = whole(layout.index(b)) {
                layer.push(x, edge);
                continue;
            }
            for j1 in k1 * edge..(k1 + 1) * edge {
                let mut v = [0usize; 3];
                v[axis] = at;
                v[a1] = j1;
                v[a2] = j2;
                layer.push(voxel(v), 1);
            }
        }
    }
    layer
}

/// Union-find with path halving; the smaller index becomes the root.
pub(super) struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    pub(super) fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n as u32).collect(),
        }
    }
    pub(super) fn find(&mut self, mut c: u32) -> u32 {
        while self.parent[c as usize] != c {
            let g = self.parent[self.parent[c as usize] as usize];
            self.parent[c as usize] = g;
            c = g;
        }
        c
    }
    pub(super) fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            let (lo, hi) = (a.min(b), a.max(b));
            self.parent[hi as usize] = lo;
        }
    }
}

/// Umbra's `rect_coalesce`: a cover of the disjoint
/// rectangles `rects` (`[a1 lo, a1 hi, a2 lo, a2 hi]`) by at most four rectangles, the cheapest by
/// total area plus `t` per rectangle among the single bounding rectangle and, for every corner of
/// every input rectangle, the bounding rectangles of the input clipped to the four quadrants
/// around it. Over 128 inputs are first binned into an 8 × 8 grid over their bounding rectangle
/// (each bin the bounding rectangle of the pieces in it, `rect_binGrid`). The cover may include
/// space the inputs do not (solid faces): a portal stays conservative. `None` gives the bounding
/// rectangle alone.
pub(super) fn coalesce(rects: &[[u32; 4]], t: Option<f64>) -> Vec<[u32; 4]> {
    let bound = |rs: &mut dyn Iterator<Item = [u32; 4]>| {
        rs.fold(None, |b: Option<[u32; 4]>, r| {
            Some(b.map_or(r, |b| {
                [
                    b[0].min(r[0]),
                    b[1].max(r[1]),
                    b[2].min(r[2]),
                    b[3].max(r[3]),
                ]
            }))
        })
    };
    let Some(bb) = bound(&mut rects.iter().copied()) else {
        return Vec::new();
    };
    let Some(t) = t else {
        return vec![bb];
    };
    if rects.len() == 1 {
        return rects.to_vec();
    }
    let area = |r: &[u32; 4]| f64::from(r[1] - r[0]) * f64::from(r[3] - r[2]);
    let clip = |r: &[u32; 4], c: [u32; 4]| -> Option<[u32; 4]> {
        let x = [
            r[0].max(c[0]),
            r[1].min(c[1]),
            r[2].max(c[2]),
            r[3].min(c[3]),
        ];
        (x[0] < x[1] && x[2] < x[3]).then_some(x)
    };
    let binned: Vec<[u32; 4]>;
    let rs: &[[u32; 4]] = if rects.len() > 128 {
        let edge = |lo: u32, hi: u32, i: u64| lo + ((u64::from(hi - lo) * i) / 8) as u32;
        let mut bins = Vec::new();
        for j in 0..8 {
            for i in 0..8 {
                let cell = [
                    edge(bb[0], bb[1], i),
                    edge(bb[0], bb[1], i + 1),
                    edge(bb[2], bb[3], j),
                    edge(bb[2], bb[3], j + 1),
                ];
                if let Some(b) = bound(&mut rects.iter().filter_map(|r| clip(r, cell))) {
                    bins.push(b);
                }
            }
        }
        binned = bins;
        &binned
    } else {
        rects
    };
    let mut best = vec![bb];
    let mut best_cost = area(&bb) + t;
    for r in rs {
        for (x, y) in [(r[0], r[2]), (r[1], r[2]), (r[0], r[3]), (r[1], r[3])] {
            let quads = [
                [bb[0], x, bb[2], y],
                [x, bb[1], bb[2], y],
                [bb[0], x, y, bb[3]],
                [x, bb[1], y, bb[3]],
            ];
            let cover: Vec<[u32; 4]> = quads
                .iter()
                .filter_map(|&q| bound(&mut rs.iter().filter_map(|s| clip(s, q))))
                .collect();
            let cost = cover.iter().map(area).sum::<f64>() + t * cover.len() as f64;
            if cost < best_cost {
                best_cost = cost;
                best = cover;
            }
        }
    }
    best
}

/// Coalesces the rectangles of every run of equal keys in `links` (sorted by key), see
/// [`coalesce`].
pub(super) fn coalesce_runs<K: Copy + PartialEq>(links: &mut Vec<(K, [u32; 4])>, t: Option<f64>) {
    let mut out = Vec::with_capacity(links.len());
    let mut i = 0;
    while i < links.len() {
        let key = links[i].0;
        let mut j = i;
        while j < links.len() && links[j].0 == key {
            j += 1;
        }
        let rects: Vec<[u32; 4]> = links[i..j].iter().map(|l| l.1).collect();
        out.extend(coalesce(&rects, t).into_iter().map(|r| (key, r)));
        i = j;
    }
    *links = out;
}
