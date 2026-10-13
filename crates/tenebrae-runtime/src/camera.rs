//! The query camera: a world-to-clip transform turned into the six frustum planes, the near
//! quad and the frustum's bounding box.

/// The clip-space depth range of a projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// Direct3D: the near plane is `z = 0`, the far plane `z = w`.
    ZeroToOne,
    /// OpenGL: the near plane is `z = -w`, the far plane `z = w`.
    MinusOneToOne,
}

/// A camera for queries: its frustum, position and near plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Row-vector world-to-clip matrix: `clip = [p, 1] * m`, with D3D depth.
    pub(crate) m: [[f32; 4]; 4],
    pub(crate) position: [f32; 3],
    /// Near, far, left (`w + x`), right (`w - x`), bottom (`w + y`), top (`w - y`), each as
    /// `(n, d)` with `n . p + d >= 0` inside. The near plane's normal has unit length.
    pub(crate) planes: [[f32; 4]; 6],
    /// Where near meets (left, bottom), (right, bottom), (right, top), (left, top).
    pub(crate) near_corners: [[f32; 3]; 4],
    /// The frustum's bounding box; infinite when the far plane is unusable.
    pub(crate) frustum_min: [f32; 3],
    pub(crate) frustum_max: [f32; 3],
    /// The matrix cannot project: its upper-left 3x3 is a multiple of the identity (including
    /// zero), or it is not finite.
    pub(crate) degenerate: bool,
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The point where three planes meet, or `None` when they do not meet in one point.
fn meet(p: [f32; 4], q: [f32; 4], r: [f32; 4]) -> Option<[f32; 3]> {
    let (n1, n2, n3) = ([p[0], p[1], p[2]], [q[0], q[1], q[2]], [r[0], r[1], r[2]]);
    let det = dot(n1, cross(n2, n3));
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let (c23, c31, c12) = (cross(n2, n3), cross(n3, n1), cross(n1, n2));
    let x = std::array::from_fn(|k| (-p[3] * c23[k] - q[3] * c31[k] - r[3] * c12[k]) / det);
    x.iter().all(|v: &f32| v.is_finite()).then_some(x)
}

/// `a * b` for row-vector 4x4 matrices.
fn mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| (0..4).fold(0.0f32, |s, k| s + a[i][k] * b[k][j]))
    })
}

impl Camera {
    /// A camera from a row-vector world-to-clip matrix (`clip = [p, 1] * m`) with Direct3D
    /// depth (`0 <= z <= w` inside), as the engine passes it, and the camera's position.
    pub fn from_world_to_clip_rows(m: [[f32; 4]; 4], position: [f32; 3]) -> Camera {
        Self::build(m, position)
    }

    /// A camera from a column-vector world-to-clip matrix (`clip = m * [p, 1]`) with Direct3D
    /// depth, and the camera's position.
    pub fn from_world_to_clip_cols(m: [[f32; 4]; 4], position: [f32; 3]) -> Camera {
        Self::build(
            std::array::from_fn(|i| std::array::from_fn(|j| m[j][i])),
            position,
        )
    }

    /// A camera from row-vector view and projection matrices (`clip = [p, 1] * view * proj`)
    /// whose projection uses the given depth range. The position is the view matrix's inverse
    /// translation, which must exist (a rigid view).
    pub fn from_view_projection(view: [[f32; 4]; 4], proj: [[f32; 4]; 4], depth: Depth) -> Camera {
        // Position: the view's rotation is orthonormal, so the eye is -t * R^T.
        let t = [view[3][0], view[3][1], view[3][2]];
        let position =
            std::array::from_fn(|k| -(t[0] * view[k][0] + t[1] * view[k][1] + t[2] * view[k][2]));
        let proj = match depth {
            Depth::ZeroToOne => proj,
            // z' = (z + w) / 2 maps -w..w to 0..w: add half of column 3 to column 2.
            Depth::MinusOneToOne => {
                let mut p = proj;
                for row in &mut p {
                    row[2] = (row[2] + row[3]) * 0.5;
                }
                p
            }
        };
        Self::build(mul(&view, &proj), position)
    }

    fn build(m: [[f32; 4]; 4], position: [f32; 3]) -> Camera {
        let finite =
            m.iter().flatten().all(|v| v.is_finite()) && position.iter().all(|v| v.is_finite());
        let identity_multiple = (0..3).all(|i| (0..3).all(|j| i == j || m[i][j] == 0.0))
            && m[0][0] == m[1][1]
            && m[1][1] == m[2][2];
        let col = |k: usize| [m[0][k], m[1][k], m[2][k], m[3][k]];
        let (x, y, z, w) = (col(0), col(1), col(2), col(3));
        let sub = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] { std::array::from_fn(|k| a[k] - b[k]) };
        let add = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] { std::array::from_fn(|k| a[k] + b[k]) };
        let mut near = z;
        let len = (near[0] * near[0] + near[1] * near[1] + near[2] * near[2]).sqrt();
        if len > 0.0 && len.is_finite() {
            for v in &mut near {
                *v /= len;
            }
        }
        let planes = [near, sub(w, z), add(w, x), sub(w, x), add(w, y), sub(w, y)];
        let [_, far, left, right, bottom, top] = planes;
        let corner = |p: [f32; 4], a: [f32; 4], b: [f32; 4]| meet(p, a, b).unwrap_or(position);
        let near_corners = [
            corner(near, left, bottom),
            corner(near, right, bottom),
            corner(near, right, top),
            corner(near, left, top),
        ];
        let far_corners = [
            meet(far, left, bottom),
            meet(far, right, bottom),
            meet(far, right, top),
            meet(far, left, top),
        ];
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let mut grow = |p: [f32; 3]| {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        };
        near_corners.iter().for_each(|&p| grow(p));
        let (frustum_min, frustum_max) = if far_corners.iter().all(Option::is_some) {
            far_corners.iter().flatten().for_each(|&p| grow(p));
            (lo, hi)
        } else {
            ([f32::NEG_INFINITY; 3], [f32::INFINITY; 3])
        };
        Camera {
            m,
            position,
            planes,
            near_corners,
            frustum_min,
            frustum_max,
            degenerate: !finite || identity_multiple,
        }
    }

    /// The camera's position.
    pub fn position(&self) -> [f32; 3] {
        self.position
    }

    /// This camera with every frustum plane moved outward by `radius` and the frustum box
    /// grown by it: the frustum of any eye within `radius` of the position lies inside it.
    pub(crate) fn grown(&self, radius: f32) -> Camera {
        if radius <= 0.0 {
            return *self;
        }
        let mut c = *self;
        for p in &mut c.planes {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            p[3] += radius * len;
        }
        c.frustum_min = c.frustum_min.map(|v| v - radius);
        c.frustum_max = c.frustum_max.map(|v| v + radius);
        c
    }

    /// `(x, y, w)` of `p` in clip space.
    pub(crate) fn project(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.m;
        let at = |k: usize| p[0] * m[0][k] + p[1] * m[1][k] + p[2] * m[2][k] + m[3][k];
        [at(0), at(1), at(3)]
    }

    /// The box `min..max` is not wholly outside any frustum plane and overlaps the frustum's
    /// bounding box.
    pub(crate) fn box_in_frustum(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        for k in 0..3 {
            if min[k] > self.frustum_max[k] || max[k] < self.frustum_min[k] {
                return false;
            }
        }
        self.planes.iter().all(|p| {
            let c = |k: usize| if p[k] >= 0.0 { max[k] } else { min[k] };
            p[0] * c(0) + p[1] * c(1) + p[2] * c(2) + p[3] >= 0.0
        })
    }
}
