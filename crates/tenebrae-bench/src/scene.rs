//! A benchmark scene, its builder and its file forms.
//!
//! File forms (little endian), the formats `bake` and `truth` read:
//!
//! * `scene.bin`: the `truth` scene format: u32 vertex count, u32 triangle count, f32[3] per
//!   vertex, u32[3] per triangle, u32 owner per triangle.
//! * `targets.bin`: the `bake` target format: u32 count, then per target f32[6] box (min, max)
//!   and u32 user id. The user id is the target index.
//! * `seeds.bin`: u32 count, then f32[3] per point.
//! * `cameras.bin`: u32 count, then per camera f32[3] eye and f32[3] look-at point.
//! * `scene.txt`: `key value...` lines: `name`, `generator`, `hash`, `volume` (six floats),
//!   `counts` (vertices, triangles, targets, seeds, cameras).

use tenebrae::bake::{Instance, Target};
use tenebrae::geom::{Bounds, Bvh, Mesh, Vec3, triangle_box};
use tenebrae::tome::Aabb;

use crate::GENERATOR;

/// One object the tome answers for. Its user id is its index in [`Scene::targets`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneTarget {
    /// World box.
    pub bounds: Aabb,
    /// The occluder owner whose triangles are this object, if any: those triangles never hide
    /// the object from itself in the truth. `None` for a target with no geometry (a prop).
    pub owner: Option<u32>,
}

/// A camera: the eye and a point it looks at (FOV 70°, aspect 16:9, Z up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Eye position.
    pub eye: Vec3,
    /// Look-at point.
    pub target: Vec3,
}

/// A generated scene.
#[derive(Clone, Debug)]
pub struct Scene {
    /// Short name, also the export directory.
    pub name: &'static str,
    /// What the scene stresses.
    pub about: &'static str,
    /// Occluder triangles in world space; owners number the occluder objects.
    pub mesh: Mesh,
    /// The same occluders as meshes and placements, when the scene is built from instances
    /// (`mesh` is then exactly their applied soup).
    pub instanced: Option<(Vec<Mesh>, Vec<Instance>)>,
    /// Objects, user id = index.
    pub targets: Vec<SceneTarget>,
    /// Where the camera may be.
    pub view_volume: Aabb,
    /// Open space the camera reaches.
    pub seeds: Vec<Vec3>,
    /// Benchmark views.
    pub cameras: Vec<Camera>,
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// FNV-1a, 64 bits.
pub fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in *part {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

impl Scene {
    /// `scene.bin`.
    pub fn scene_bytes(&self) -> Vec<u8> {
        let m = &self.mesh;
        let mut out = Vec::with_capacity(8 + 12 * m.vertices.len() + 16 * m.triangles.len());
        put_u32(&mut out, m.vertices.len() as u32);
        put_u32(&mut out, m.triangles.len() as u32);
        for v in &m.vertices {
            put_f32s(&mut out, v);
        }
        for t in &m.triangles {
            for &i in t {
                put_u32(&mut out, i);
            }
        }
        for &o in &m.owners {
            put_u32(&mut out, o);
        }
        out
    }

    /// `targets.bin`.
    pub fn targets_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 28 * self.targets.len());
        put_u32(&mut out, self.targets.len() as u32);
        for (i, t) in self.targets.iter().enumerate() {
            put_f32s(&mut out, &t.bounds.min);
            put_f32s(&mut out, &t.bounds.max);
            put_u32(&mut out, i as u32);
        }
        out
    }

    /// `seeds.bin`.
    pub fn seeds_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 12 * self.seeds.len());
        put_u32(&mut out, self.seeds.len() as u32);
        for s in &self.seeds {
            put_f32s(&mut out, s);
        }
        out
    }

    /// `cameras.bin`.
    pub fn cameras_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 24 * self.cameras.len());
        put_u32(&mut out, self.cameras.len() as u32);
        for c in &self.cameras {
            put_f32s(&mut out, &c.eye);
            put_f32s(&mut out, &c.target);
        }
        out
    }

    /// The instanced form's bytes (empty for a soup scene): per mesh its vertices and
    /// triangles, then per instance its mesh and transform.
    fn instanced_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some((meshes, instances)) = &self.instanced {
            put_u32(&mut out, meshes.len() as u32);
            for m in meshes {
                put_u32(&mut out, m.vertices.len() as u32);
                put_u32(&mut out, m.triangles.len() as u32);
                for v in &m.vertices {
                    put_f32s(&mut out, v);
                }
                for t in &m.triangles {
                    for &i in t {
                        put_u32(&mut out, i);
                    }
                }
            }
            put_u32(&mut out, instances.len() as u32);
            for inst in instances {
                put_u32(&mut out, inst.mesh);
                for row in &inst.transform {
                    put_f32s(&mut out, row);
                }
            }
        }
        out
    }

    /// Hash of everything a bake or a score reads: the generator version, the name, every file
    /// form, the instanced form and the view volume.
    pub fn hash(&self) -> u64 {
        let mut volume = Vec::new();
        put_f32s(&mut volume, &self.view_volume.min);
        put_f32s(&mut volume, &self.view_volume.max);
        fnv64(&[
            &GENERATOR.to_le_bytes(),
            self.name.as_bytes(),
            &self.scene_bytes(),
            &self.targets_bytes(),
            &self.seeds_bytes(),
            &self.cameras_bytes(),
            &self.instanced_bytes(),
            &volume,
        ])
    }

    /// `scene.txt`.
    pub fn manifest(&self) -> String {
        let (lo, hi) = (self.view_volume.min, self.view_volume.max);
        format!(
            "name {}\ngenerator {}\nhash {:016x}\nvolume {} {} {} {} {} {}\ncounts {} {} {} {} {}\n",
            self.name,
            GENERATOR,
            self.hash(),
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
            self.mesh.vertices.len(),
            self.mesh.triangles.len(),
            self.targets.len(),
            self.seeds.len(),
            self.cameras.len()
        )
    }

    /// The user ids [`Scene::bake_targets`] borrows: target `i` has user `i`.
    pub fn users(&self) -> Vec<u32> {
        (0..self.targets.len() as u32).collect()
    }

    /// The bake's targets, one user each (`users` from [`Scene::users`]).
    pub fn bake_targets<'a>(&self, users: &'a [u32]) -> Vec<Target<'a>> {
        self.targets
            .iter()
            .zip(users)
            .map(|(t, u)| Target {
                bounds: t.bounds,
                users: std::slice::from_ref(u),
            })
            .collect()
    }
}

/// SplitMix64: a small, fixed, platform-independent generator.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator from `seed`.
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in `[0, 1)`, exactly representable (24 bits).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }
    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    /// Uniform integer in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * u64::from(n)) >> 32) as u32
    }
    /// `true` with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
}

/// The 8 corners and 12 triangles of a box, (vertex
/// index `4 ix + 2 iy + iz`, outward counter-clockwise).
pub fn box_mesh(mn: Vec3, mx: Vec3) -> ([Vec3; 8], [[u32; 3]; 12]) {
    let mut v = [[0.0; 3]; 8];
    for (i, c) in v.iter_mut().enumerate() {
        *c = [
            if i & 4 == 0 { mn[0] } else { mx[0] },
            if i & 2 == 0 { mn[1] } else { mx[1] },
            if i & 1 == 0 { mn[2] } else { mx[2] },
        ];
    }
    let quads = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let mut t = [[0; 3]; 12];
    for (k, [a, b, c, d]) in quads.into_iter().enumerate() {
        t[2 * k] = [a, b, c];
        t[2 * k + 1] = [a, c, d];
    }
    (v, t)
}

/// The box around `points`.
pub fn bounds_of(points: &[Vec3]) -> Aabb {
    let mut b = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for p in points {
        for (k, &v) in p.iter().enumerate() {
            b.min[k] = b.min[k].min(v);
            b.max[k] = b.max[k].max(v);
        }
    }
    b
}

/// Builds a scene's occluders and targets object by object.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    /// Occluders so far.
    pub mesh: Mesh,
    /// Targets so far.
    pub targets: Vec<SceneTarget>,
    /// Closed solid boxes (a camera is never placed in one).
    pub solids: Vec<Aabb>,
    owners: u32,
}

impl Builder {
    /// An empty builder.
    pub fn new() -> Builder {
        Builder::default()
    }

    /// Adds triangles as one occluder object and returns its owner; with `target`, the object's
    /// box is also a target.
    pub fn tris(&mut self, verts: &[Vec3], tris: &[[u32; 3]], target: bool) -> u32 {
        let owner = self.owners;
        self.owners += 1;
        let base = self.mesh.vertices.len() as u32;
        self.mesh.vertices.extend_from_slice(verts);
        for t in tris {
            self.mesh
                .triangles
                .push([t[0] + base, t[1] + base, t[2] + base]);
            self.mesh.owners.push(owner);
        }
        if target {
            let used: Vec<Vec3> = tris.iter().flatten().map(|&i| verts[i as usize]).collect();
            self.targets.push(SceneTarget {
                bounds: bounds_of(&used),
                owner: Some(owner),
            });
        }
        owner
    }

    /// A closed box occluder (solid space), optionally a target.
    pub fn solid(&mut self, mn: Vec3, mx: Vec3, target: bool) -> u32 {
        let (v, t) = box_mesh(mn, mx);
        self.solids.push(Aabb { min: mn, max: mx });
        self.tris(&v, &t, target)
    }

    /// A target with no geometry.
    pub fn prop(&mut self, mn: Vec3, mx: Vec3) {
        self.targets.push(SceneTarget {
            bounds: Aabb { min: mn, max: mx },
            owner: None,
        });
    }

    /// A planar quad `a b c d` (in order around it), both windings when `two_sided`.
    pub fn quad(&mut self, q: [Vec3; 4], two_sided: bool, target: bool) -> u32 {
        let mut t = vec![[0, 1, 2], [0, 2, 3]];
        if two_sided {
            t.extend([[0, 2, 1], [0, 3, 2]]);
        }
        self.tris(&q, &t, target)
    }

    /// Finishes the scene: cameras are drawn by `sample` (an eye candidate per call) and kept
    /// when the eye is inside the view volume, at least 0.3 m from every triangle and outside
    /// every solid grown by it.
    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        self,
        name: &'static str,
        about: &'static str,
        view_volume: Aabb,
        seeds: Vec<Vec3>,
        n_cameras: usize,
        rng: &mut Rng,
        mut sample: impl FnMut(&mut Rng) -> Vec3,
    ) -> Scene {
        let bvh = Bvh::build(&self.mesh);
        let mut cameras = Vec::with_capacity(n_cameras);
        let mut near = Vec::new();
        let mut tries = 0usize;
        while cameras.len() < n_cameras {
            tries += 1;
            assert!(
                tries < 1000 * n_cameras.max(1),
                "{name}: camera sampler finds no open space"
            );
            let eye = sample(rng);
            let clear = 0.3;
            // The camera never leaves the view volume (a domain snapped to coarse tiles would
            // hide an eye past it, a fine one would not). A sampler returns NaN for no candidate.
            if (0..3).any(|k| !(eye[k] >= view_volume.min[k] && eye[k] <= view_volume.max[k])) {
                continue;
            }
            if self
                .solids
                .iter()
                .any(|s| (0..3).all(|k| eye[k] > s.min[k] - clear && eye[k] < s.max[k] + clear))
            {
                continue;
            }
            let probe = Bounds {
                min: [eye[0] - clear, eye[1] - clear, eye[2] - clear],
                max: [eye[0] + clear, eye[1] + clear, eye[2] + clear],
            };
            near.clear();
            bvh.overlapping(&probe, &mut near);
            if near.iter().any(|t| triangle_box(t, &probe)) {
                continue;
            }
            // A heading in the unit disk (no trigonometry), a slight pitch.
            let (dx, dy) = loop {
                let (x, y) = (rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
                let r2 = x * x + y * y;
                if (0.01..=1.0).contains(&r2) {
                    let r = r2.sqrt();
                    break (x / r, y / r);
                }
            };
            let dz = rng.range(-0.4, 0.2);
            cameras.push(Camera {
                eye,
                target: [eye[0] + dx, eye[1] + dy, eye[2] + dz],
            });
        }
        Scene {
            name,
            about,
            mesh: self.mesh,
            instanced: None,
            targets: self.targets,
            view_volume,
            seeds,
            cameras,
        }
    }
}
