//! Exact visibility for a scene's cameras: which targets some sight line reaches.
//!
//! The camera model: row vectors, D3D 0..1 depth, Z up, FOV 70°, aspect 16:9, near 0.1, far
//! 4000, the model the Umbra reference was measured with.
//! A target is truly visible when one of its sample points lies in the frustum and the open
//! segment from the eye to it crosses no triangle of another object. The samples are
//! a 6×6 grid on each face of the target's box that the eye is
//! outside of. Umbra answers for object bounds, so the box, not the object's triangles, is what
//! the truth samples.
//!
//! Each camera gets two truths ([`Visible`]): strict, through any gap, and through a hole at least
//! the smallest hole wide. Both bakers close gaps under the smallest hole by design, so misses of
//! the strict truth alone are the tolerance's cost, and misses of the hole truth are errors.

use tenebrae::geom::{Bvh, Vec3};

use crate::scene::{Camera, Scene};

/// Samples per face edge.
pub const GRID: usize = 6;

/// `1 / tan(35°)`: the projection's y scale at FOV 70°, as a constant so no platform `tan` is
/// involved.
const YS: f64 = 1.428_148_006_742_114_3;

/// The y scale the reference harness computed (`1 / tan(70° / 2)` in f64), one f64 step above
/// [`YS`]: [`camera`] uses it, so the query's camera is the one the reference was measured with.
const YS_HARNESS: f64 = 1.428_148_006_742_114_6;

/// The row-vector `p * M` view-projection matrix of a camera.
pub fn world_to_clip(c: &Camera) -> [[f64; 4]; 4] {
    view_projection(c, YS)
}

/// The view-projection with y scale `ys`, in the reference harness's operation
/// order (sums start from zero, left to right).
fn view_projection(c: &Camera, ys: f64) -> [[f64; 4]; 4] {
    let (eye, target) = (c.eye.map(f64::from), c.target.map(f64::from));
    let norm = |v: [f64; 3]| {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let f = norm([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
    let up = if f[2].abs() < 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let r = norm(cross(up, f));
    let u = cross(f, r);
    let v = [
        [r[0], u[0], f[0], 0.0],
        [r[1], u[1], f[1], 0.0],
        [r[2], u[2], f[2], 0.0],
        [-dot(r, eye), -dot(u, eye), -dot(f, eye), 1.0],
    ];
    let (near, far) = (0.1, 4000.0);
    let xs = ys / (16.0 / 9.0);
    let p = [
        [xs, 0.0, 0.0, 0.0],
        [0.0, ys, 0.0, 0.0],
        [0.0, 0.0, far / (far - near), 1.0],
        [0.0, 0.0, -near * far / (far - near), 0.0],
    ];
    let mut m = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = (0..4).fold(0.0, |s, k| s + v[i][k] * p[k][j]);
        }
    }
    m
}

/// The row-vector view-projection the reference harness queried with, rounded to `f32`.
pub fn clip_matrix(c: &Camera) -> [[f32; 4]; 4] {
    view_projection(c, YS_HARNESS).map(|row| row.map(|x| x as f32))
}

/// The camera the reference harness queried with: its view-projection rounded to `f32`, and
/// the eye.
pub fn camera(c: &Camera) -> tenebrae::runtime::Camera {
    tenebrae::runtime::Camera::from_world_to_clip_rows(clip_matrix(c), c.eye)
}

/// `p` lies in the frustum of `m`.
pub fn in_view(m: &[[f64; 4]; 4], p: Vec3) -> bool {
    let p = p.map(f64::from);
    let c: [f64; 4] =
        std::array::from_fn(|j| p[0] * m[0][j] + p[1] * m[1][j] + p[2] * m[2][j] + m[3][j]);
    c[3] > 0.0
        && -c[3] <= c[0]
        && c[0] <= c[3]
        && -c[3] <= c[1]
        && c[1] <= c[3]
        && 0.0 <= c[2]
        && c[2] <= c[3]
}

/// The sample points on the faces of `min..max` that `eye` is outside of.
pub fn surface_samples(min: Vec3, max: Vec3, eye: Vec3, out: &mut Vec<Vec3>) {
    out.clear();
    for a in 0..3 {
        for (side, outward) in [(min[a], -1.0f32), (max[a], 1.0)] {
            if (eye[a] - side) * outward <= 0.0 {
                continue;
            }
            let (b, c) = ((a + 1) % 3, (a + 2) % 3);
            for i in 0..GRID {
                for j in 0..GRID {
                    let mut p = [0.0; 3];
                    p[a] = side;
                    p[b] = min[b] + (max[b] - min[b]) * (i as f32 + 0.5) / GRID as f32;
                    p[c] = min[c] + (max[c] - min[c]) * (j as f32 + 0.5) / GRID as f32;
                    out.push(p);
                }
            }
        }
    }
}

/// The smallest hole (Umbra's `SH`, 0.125 m in the stock tomes): gaps narrower than this are closed by design
/// in both bakers, so [`Visible::hole`] only counts sight lines through openings this wide.
pub const HOLE: f32 = 0.125;

/// The two truths of one camera, ascending target indices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visible {
    /// Strict: some sample is reached by a clear segment, through any gap.
    pub strict: Vec<u32>,
    /// Through a hole: some sample is reached by a clear segment that stays clear when moved by
    /// [`HOLE`] / 2 either way along both axes across it (a beam [`HOLE`] wide). A subset of
    /// `strict`; the difference is what the smallest-hole tolerance may cull.
    pub hole: Vec<u32>,
}

/// `eye -> p` and its four copies moved by `HOLE / 2` across the segment are all clear.
fn beam_clear(bvh: &Bvh, eye: Vec3, p: Vec3, skip: &[u32]) -> bool {
    let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len <= 0.0 {
        return true;
    }
    let axis = if d[2].abs() < 0.9 * len {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let cross = |a: Vec3, b: Vec3| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let unit = |a: Vec3| {
        let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        [a[0] / n, a[1] / n, a[2] / n]
    };
    let u = unit(cross(d, axis));
    let v = unit(cross(d, u));
    let h = HOLE * 0.5;
    [(h, 0.0), (-h, 0.0), (0.0, h), (0.0, -h)]
        .iter()
        .all(|&(a, b)| {
            let o = |q: Vec3| -> Vec3 { std::array::from_fn(|k| q[k] + a * u[k] + b * v[k]) };
            !bvh.segment_blocked(o(eye), o(p), skip)
        })
}

/// Per camera, the truly visible targets (strict and through a hole).
pub fn visible(scene: &Scene, threads: usize) -> Vec<Visible> {
    let bvh = Bvh::build(&scene.mesh);
    let one = |c: &Camera| -> Visible {
        let m = world_to_clip(c);
        let mut pts = Vec::new();
        let mut out = Visible::default();
        for (i, t) in scene.targets.iter().enumerate() {
            surface_samples(t.bounds.min, t.bounds.max, c.eye, &mut pts);
            let skip: &[u32] = match &t.owner {
                Some(o) => std::slice::from_ref(o),
                None => &[],
            };
            let mut clear = pts
                .iter()
                .filter(|&&p| in_view(&m, p) && !bvh.segment_blocked(c.eye, p, skip))
                .peekable();
            if clear.peek().is_some() {
                out.strict.push(i as u32);
                if clear.any(|&p| beam_clear(&bvh, c.eye, p, skip)) {
                    out.hole.push(i as u32);
                }
            }
        }
        out
    };
    let threads = threads.max(1);
    let chunk = scene.cameras.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = scene
            .cameras
            .chunks(chunk)
            .map(|cams| s.spawn(move || cams.iter().map(one).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("truth worker"))
            .collect()
    })
}

/// `truth.bin`: u32 camera count, then per camera the strict list (u32 count, u32 target
/// indices), then the through-a-hole list in the same form.
pub fn bytes(visible: &[Visible]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(visible.len() as u32).to_le_bytes());
    for v in visible {
        for list in [&v.strict, &v.hole] {
            out.extend_from_slice(&(list.len() as u32).to_le_bytes());
            for i in list {
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
    }
    out
}
