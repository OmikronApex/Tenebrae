//! Geometry core: triangle soup with per-triangle owners, a bounding-volume hierarchy, and
//! segment occlusion queries. Used as exact visibility ground truth and by the baker.

/// A 3-component vector.
pub type Vec3 = [f32; 3];

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Axis-aligned box used by the hierarchy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Bounds {
    /// The empty box (absorbs nothing, grows from the first point).
    pub const EMPTY: Bounds = Bounds {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    /// Grows the box to contain `p`.
    pub fn grow(&mut self, p: Vec3) {
        for (a, v) in p.iter().enumerate() {
            self.min[a] = self.min[a].min(*v);
            self.max[a] = self.max[a].max(*v);
        }
    }
    /// Grows the box to contain `b`.
    pub fn merge(&mut self, b: &Bounds) {
        self.grow(b.min);
        self.grow(b.max);
    }
    /// Surface area (0 for the empty box).
    pub fn area(&self) -> f32 {
        let d = sub(self.max, self.min);
        if d[0] < 0.0 {
            return 0.0;
        }
        2.0 * (d[0] * d[1] + d[1] * d[2] + d[2] * d[0])
    }
    /// Centre.
    pub fn centre(&self) -> Vec3 {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }
}

/// Triangle soup. Each triangle carries an owner id (an instance, an object), so queries can
/// ignore the geometry of the thing whose visibility is being tested.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// Vertex positions.
    pub vertices: Vec<Vec3>,
    /// Triangles as vertex indices.
    pub triangles: Vec<[u32; 3]>,
    /// One owner id per triangle.
    pub owners: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Bounds,
    /// Leaf: first triangle in the reordered list. Inner: index of the second child (the first
    /// child follows the node).
    start: u32,
    /// Leaf: triangle count (> 0). Inner: 0.
    count: u32,
}

/// Bounding-volume hierarchy over a [`Mesh`] (binned surface-area heuristic).
#[derive(Clone, Debug)]
pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangles in leaf order: the three corners and the owner.
    tris: Vec<([Vec3; 3], u32)>,
}

const BINS: usize = 16;
const LEAF_SIZE: usize = 4;

impl Bvh {
    /// Builds the hierarchy. Degenerate triangles are kept; they never report a hit.
    pub fn build(mesh: &Mesh) -> Bvh {
        let mut items: Vec<(Bounds, Vec3, usize)> = mesh
            .triangles
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut b = Bounds::EMPTY;
                for &v in t {
                    b.grow(mesh.vertices[v as usize]);
                }
                (b, b.centre(), i)
            })
            .collect();
        let mut nodes = Vec::with_capacity(2 * items.len() / LEAF_SIZE + 1);
        if !items.is_empty() {
            let len = items.len();
            build_range(&mut items, 0, len, &mut nodes);
        }
        let tris = items
            .iter()
            .map(|&(_, _, i)| {
                let t = mesh.triangles[i];
                (
                    [
                        mesh.vertices[t[0] as usize],
                        mesh.vertices[t[1] as usize],
                        mesh.vertices[t[2] as usize],
                    ],
                    mesh.owners.get(i).copied().unwrap_or(u32::MAX),
                )
            })
            .collect();
        Bvh { nodes, tris }
    }

    /// Number of triangles.
    pub fn len(&self) -> usize {
        self.tris.len()
    }

    /// `true` when there are no triangles.
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// `true` when the open segment `a -> b` (end points excluded by a small margin) crosses a
    /// triangle whose owner is not in `skip`.
    pub fn segment_blocked(&self, a: Vec3, b: Vec3, skip: &[u32]) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let d = sub(b, a);
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(index) = stack.pop() {
            let node = self.nodes[index as usize];
            if !slab(&node.bounds, a, inv) {
                continue;
            }
            if node.count > 0 {
                for (corners, owner) in
                    &self.tris[node.start as usize..(node.start + node.count) as usize]
                {
                    if !skip.contains(owner) && hits(corners, a, d) {
                        return true;
                    }
                }
            } else {
                // Inner node: the first child follows it, the second is at `start`.
                stack.push(index + 1);
                stack.push(node.start);
            }
        }
        false
    }
}

fn slab(b: &Bounds, o: Vec3, inv: Vec3) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    for a in 0..3 {
        let mut near = (b.min[a] - o[a]) * inv[a];
        let mut far = (b.max[a] - o[a]) * inv[a];
        if near > far {
            core::mem::swap(&mut near, &mut far);
        }
        if near.is_nan() || far.is_nan() {
            if o[a] < b.min[a] || o[a] > b.max[a] {
                return false;
            }
            continue;
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

impl Bvh {
    /// Segment parameters in `[0, 1]` of every triangle crossing of `a -> b`, unsorted.
    pub fn crossings(&self, a: Vec3, b: Vec3, out: &mut Vec<f32>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let d = sub(b, a);
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(index) = stack.pop() {
            let node = self.nodes[index as usize];
            if !slab(&node.bounds, a, inv) {
                continue;
            }
            if node.count > 0 {
                for (corners, _) in
                    &self.tris[node.start as usize..(node.start + node.count) as usize]
                {
                    if let Some(t) = hit_param(corners, a, d) {
                        out.push(t);
                    }
                }
            } else {
                stack.push(index + 1);
                stack.push(node.start);
            }
        }
    }

    /// Triangles (corners) whose bounds overlap `b`, appended to `out`.
    pub fn overlapping(&self, b: &Bounds, out: &mut Vec<[Vec3; 3]>) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(index) = stack.pop() {
            let node = self.nodes[index as usize];
            if !overlaps(&node.bounds, b) {
                continue;
            }
            if node.count > 0 {
                for (corners, _) in
                    &self.tris[node.start as usize..(node.start + node.count) as usize]
                {
                    let mut tb = Bounds::EMPTY;
                    for c in corners {
                        tb.grow(*c);
                    }
                    if overlaps(&tb, b) {
                        out.push(*corners);
                    }
                }
            } else {
                stack.push(index + 1);
                stack.push(node.start);
            }
        }
    }
}

/// `true` when the two boxes overlap (touching counts).
pub fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    (0..3).all(|k| a.min[k] <= b.max[k] && b.min[k] <= a.max[k])
}

/// Triangle / axis-aligned box overlap (separating axis test, Akenine-Moller).
pub fn triangle_box(t: &[Vec3; 3], b: &Bounds) -> bool {
    let c = b.centre();
    let h = [
        (b.max[0] - b.min[0]) * 0.5,
        (b.max[1] - b.min[1]) * 0.5,
        (b.max[2] - b.min[2]) * 0.5,
    ];
    let v = [sub(t[0], c), sub(t[1], c), sub(t[2], c)];
    let e = [sub(v[1], v[0]), sub(v[2], v[1]), sub(v[0], v[2])];
    // Nine edge cross-axis tests.
    for edge in &e {
        for axis in 0..3 {
            let mut a = [0.0f32; 3];
            a[axis] = 1.0;
            let n = cross(a, *edge);
            let p = [dot(v[0], n), dot(v[1], n), dot(v[2], n)];
            let r = h[0] * n[0].abs() + h[1] * n[1].abs() + h[2] * n[2].abs();
            let lo = p[0].min(p[1]).min(p[2]);
            let hi = p[0].max(p[1]).max(p[2]);
            if lo > r || hi < -r {
                return false;
            }
        }
    }
    // Box face normals.
    for axis in 0..3 {
        let lo = v[0][axis].min(v[1][axis]).min(v[2][axis]);
        let hi = v[0][axis].max(v[1][axis]).max(v[2][axis]);
        if lo > h[axis] || hi < -h[axis] {
            return false;
        }
    }
    // Triangle plane.
    let n = cross(e[0], e[1]);
    let d = dot(n, v[0]);
    let r = h[0] * n[0].abs() + h[1] * n[1].abs() + h[2] * n[2].abs();
    d.abs() <= r
}

/// Segment parameter of a crossing of `o + s*d`, `s` in `[0, 1]`.
fn hit_param(t: &[Vec3; 3], o: Vec3, d: Vec3) -> Option<f32> {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let h = cross(d, e2);
    let det = dot(e1, h);
    if det.abs() < 1e-12 {
        return None;
    }
    let f = 1.0 / det;
    let s = sub(o, t[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = f * dot(d, q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * dot(e2, q);
    (0.0..=1.0).contains(&t).then_some(t)
}

/// Moller-Trumbore, hit strictly inside the segment (parameter in (1e-4, 1 - 1e-4)).
fn hits(t: &[Vec3; 3], o: Vec3, d: Vec3) -> bool {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let h = cross(d, e2);
    let det = dot(e1, h);
    if det.abs() < 1e-12 {
        return false;
    }
    let f = 1.0 / det;
    let s = sub(o, t[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross(s, e1);
    let v = f * dot(d, q);
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let t = f * dot(e2, q);
    t > 1e-4 && t < 1.0 - 1e-4
}

fn build_range(
    items: &mut [(Bounds, Vec3, usize)],
    start: usize,
    end: usize,
    nodes: &mut Vec<Node>,
) -> usize {
    let index = nodes.len();
    let mut bounds = Bounds::EMPTY;
    let mut centres = Bounds::EMPTY;
    for it in &items[start..end] {
        bounds.merge(&it.0);
        centres.grow(it.1);
    }
    nodes.push(Node {
        bounds,
        start: start as u32,
        count: (end - start) as u32,
    });
    if end - start <= LEAF_SIZE {
        return index;
    }
    // Binned SAH on the widest centroid axis.
    let ext = sub(centres.max, centres.min);
    let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
        0
    } else if ext[1] >= ext[2] {
        1
    } else {
        2
    };
    if ext[axis] <= 0.0 {
        return index;
    }
    let scale = BINS as f32 / ext[axis];
    let bin_of = |c: Vec3| (((c[axis] - centres.min[axis]) * scale) as usize).min(BINS - 1);
    let mut bins = [(Bounds::EMPTY, 0usize); BINS];
    for it in &items[start..end] {
        let b = &mut bins[bin_of(it.1)];
        b.0.merge(&it.0);
        b.1 += 1;
    }
    let mut best = (f32::INFINITY, 0usize);
    for split in 1..BINS {
        let (mut lb, mut lc, mut rb, mut rc) = (Bounds::EMPTY, 0usize, Bounds::EMPTY, 0usize);
        for (k, b) in bins.iter().enumerate() {
            if k < split {
                lb.merge(&b.0);
                lc += b.1;
            } else {
                rb.merge(&b.0);
                rc += b.1;
            }
        }
        if lc == 0 || rc == 0 {
            continue;
        }
        let cost = lb.area() * lc as f32 + rb.area() * rc as f32;
        if cost < best.0 {
            best = (cost, split);
        }
    }
    let mid = if best.0.is_finite() {
        let mut i = start;
        let mut j = end;
        while i < j {
            if bin_of(items[i].1) < best.1 {
                i += 1;
            } else {
                j -= 1;
                items.swap(i, j);
            }
        }
        i
    } else {
        let m = (start + end) / 2;
        items[start..end].select_nth_unstable_by(m - start, |a, b| a.1[axis].total_cmp(&b.1[axis]));
        m
    };
    if mid == start || mid == end {
        return index;
    }
    build_range(items, start, mid, nodes);
    let second = build_range(items, mid, end, nodes);
    nodes[index].start = second as u32;
    nodes[index].count = 0;
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
        }
        fn point(&mut self, scale: f32) -> Vec3 {
            [
                self.next() * scale,
                self.next() * scale,
                self.next() * scale,
            ]
        }
    }

    #[test]
    fn bvh_agrees_with_brute_force() {
        let mut r = Lcg(7);
        let mut mesh = Mesh::default();
        for i in 0..2000u32 {
            let c = r.point(100.0);
            for _ in 0..3 {
                let o = r.point(4.0);
                mesh.vertices.push([c[0] + o[0], c[1] + o[1], c[2] + o[2]]);
            }
            mesh.triangles.push([3 * i, 3 * i + 1, 3 * i + 2]);
            mesh.owners.push(i % 50);
        }
        let bvh = Bvh::build(&mesh);
        for q in 0..2000 {
            let a = r.point(100.0);
            let b = r.point(100.0);
            let skip = [q % 50];
            let brute = mesh.triangles.iter().zip(&mesh.owners).any(|(t, o)| {
                !skip.contains(o)
                    && hits(
                        &[
                            mesh.vertices[t[0] as usize],
                            mesh.vertices[t[1] as usize],
                            mesh.vertices[t[2] as usize],
                        ],
                        a,
                        sub(b, a),
                    )
            });
            assert_eq!(bvh.segment_blocked(a, b, &skip), brute, "query {q}");
        }
    }
}

#[cfg(test)]
mod overlap_tests {
    use super::*;

    #[test]
    fn thin_slabs_only_overlap_triangles_that_reach_them() {
        let floor = [[-50.0, -50.0, 0.0], [50.0, -50.0, 0.0], [50.0, 50.0, 0.0]];
        let slab = |z: f32| Bounds {
            min: [0.001, 0.001, z - 0.001],
            max: [0.999, 0.999, z + 0.001],
        };
        assert!(triangle_box(&floor, &slab(0.0)));
        assert!(!triangle_box(&floor, &slab(5.0)));
        assert!(!triangle_box(&floor, &slab(1.0)));
        let wall = [[0.0, -50.0, 0.0], [0.0, 50.0, 0.0], [0.0, 50.0, 12.0]];
        let xslab = |x: f32| Bounds {
            min: [x - 0.001, 10.001, 1.001],
            max: [x + 0.001, 10.999, 1.999],
        };
        assert!(triangle_box(&wall, &xslab(0.0)));
        assert!(!triangle_box(&wall, &xslab(3.0)));
    }
}
