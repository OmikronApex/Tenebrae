//! The benchmark scenes. Each one stresses one failure mode; all are built from fixed seeds.
//!
//! Units are metres, Z up. Tiles are 32 m, so planes at multiples of 32 are tile seams.

use tenebrae::bake::Instance;
use tenebrae::geom::{Bvh, Mesh, Vec3};
use tenebrae::tome::Aabb;

use crate::scene::{Builder, Rng, Scene, SceneTarget, bounds_of, box_mesh};

/// Cameras per scene.
pub const CAMERAS: usize = 200;

/// Every scene, in benchmark order.
pub fn all() -> Vec<Scene> {
    vec![
        rooms(),
        city(),
        thin_walls(),
        terrain(),
        maze(),
        seams(),
        instanced(),
        degenerate(),
        bistro(),
        sponza(),
    ]
}

/// The scene called `name`.
pub fn by_name(name: &str) -> Option<Scene> {
    let build: fn() -> Scene = match name {
        "rooms" => rooms,
        "city" => city,
        "thin_walls" => thin_walls,
        "terrain" => terrain,
        "maze" => maze,
        "seams" => seams,
        "instanced" => instanced,
        "degenerate" => degenerate,
        "bistro" => bistro,
        "sponza" => sponza,
        _ => return None,
    };
    Some(build())
}

/// Scene names, in benchmark order.
pub const NAMES: [&str; 10] = [
    "rooms",
    "city",
    "thin_walls",
    "terrain",
    "maze",
    "seams",
    "instanced",
    "degenerate",
    "bistro",
    "sponza",
];

fn aabb(min: Vec3, max: Vec3) -> Aabb {
    Aabb { min, max }
}

/// A wall along x (`axis` 0) or y (`axis` 1) from `a` to `b` at `at`, `thick` thick, `height`
/// tall, with doorways `(centre, width, height)`; every piece is a solid target.
#[allow(clippy::too_many_arguments)]
fn wall(
    bld: &mut Builder,
    axis: usize,
    at: f32,
    a: f32,
    b: f32,
    thick: f32,
    height: f32,
    doors: &[(f32, f32, f32)],
) {
    let piece = |bld: &mut Builder, lo: f32, hi: f32, z0: f32, z1: f32| {
        if hi - lo <= 0.0 || z1 - z0 <= 0.0 {
            return;
        }
        let (h0, h1) = (at - thick * 0.5, at + thick * 0.5);
        let (mn, mx) = if axis == 0 {
            ([lo, h0, z0], [hi, h1, z1])
        } else {
            ([h0, lo, z0], [h1, hi, z1])
        };
        bld.solid(mn, mx, true);
    };
    let mut from = a;
    for &(c, w, h) in doors {
        piece(bld, from, c - w * 0.5, 0.0, height);
        piece(bld, c - w * 0.5, c + w * 0.5, h, height);
        from = c + w * 0.5;
    }
    piece(bld, from, b, 0.0, height);
}

/// Seeds and eyes: room centres at standing height.
fn grid_points(x0: f32, y0: f32, pitch: f32, n: usize, z: f32) -> Vec<Vec3> {
    let mut out = Vec::new();
    for i in 0..n {
        for j in 0..n {
            out.push([
                x0 + pitch * (i as f32 + 0.5),
                y0 + pitch * (j as f32 + 0.5),
                z,
            ]);
        }
    }
    out
}

/// 3×3 rooms of 32 m behind 6 m walls with doorways; crates and props inside. Room walls sit
/// off the tile seams. Stresses portals through doorways.
pub fn rooms() -> Scene {
    let mut rng = Rng::new(0x0001_0001);
    let mut b = Builder::new();
    b.solid([-52.0, -52.0, -1.0], [52.0, 52.0, 0.0], true);
    let planes = [-48.0, -16.0, 16.0, 48.0];
    for (k, &p) in planes.iter().enumerate() {
        let outer = k == 0 || k == 3;
        for axis in 0..2 {
            let mut doors = Vec::new();
            if !outer {
                for r in 0..3 {
                    let c = -32.0 + 32.0 * r as f32 + rng.range(-8.0, 8.0);
                    doors.push((c, rng.range(2.0, 4.0), rng.range(2.5, 3.5)));
                }
            } else if k == 3 && axis == 0 {
                doors.push((0.0, 3.0, 3.0));
            }
            wall(&mut b, axis, p, -48.25, 48.25, 0.5, 6.0, &doors);
        }
    }
    for room in grid_points(-48.0, -48.0, 32.0, 3, 0.0) {
        for _ in 0..4 {
            let (sx, sy, sz) = (
                rng.range(1.0, 3.0),
                rng.range(1.0, 3.0),
                rng.range(1.0, 3.0),
            );
            let (x, y) = (
                room[0] + rng.range(-12.0, 12.0),
                room[1] + rng.range(-12.0, 12.0),
            );
            b.solid([x, y, 0.0], [x + sx, y + sy, sz], true);
        }
        for _ in 0..3 {
            let (x, y) = (
                room[0] + rng.range(-14.0, 14.0),
                room[1] + rng.range(-14.0, 14.0),
            );
            b.prop([x, y, 0.0], [x + 0.5, y + 0.5, 0.5]);
        }
    }
    let seeds = grid_points(-48.0, -48.0, 32.0, 3, 1.7);
    b.finish(
        "rooms",
        "3x3 rooms with doorways: portal correctness",
        aabb([-48.0, -48.0, 0.0], [48.0, 48.0, 12.0]),
        seeds,
        CAMERAS,
        &mut rng,
        |r| {
            [
                r.range(-47.0, 47.0),
                r.range(-47.0, 47.0),
                r.range(1.2, 2.5),
            ]
        },
    )
}

/// 6×6 city blocks of 24 m with 10 m streets, buildings of 6 to 40 m on lots, lamp posts as
/// target-only props. Stresses strong occlusion and cell merging (culling efficiency).
pub fn city() -> Scene {
    let mut rng = Rng::new(0x0002_0002);
    let mut b = Builder::new();
    let (pitch, block, n) = (34.0f32, 24.0f32, 6);
    let x0 = -(pitch * n as f32) * 0.5;
    // Streets are 10 m wide, centred on x0 + pitch * i; the outer ones lie inside the volume.
    let half = -x0 + 5.0;
    b.solid(
        [-half - 3.0, -half - 3.0, -1.0],
        [half + 3.0, half + 3.0, 0.0],
        true,
    );
    for i in 0..n {
        for j in 0..n {
            let (bx, by) = (x0 + 5.0 + pitch * i as f32, x0 + 5.0 + pitch * j as f32);
            // Lots: the block whole, halved or quartered.
            let split = rng.below(3);
            let lots: Vec<(f32, f32, f32, f32)> = match split {
                0 => vec![(bx, by, block, block)],
                1 => vec![
                    (bx, by, block * 0.5 - 1.0, block),
                    (bx + block * 0.5 + 1.0, by, block * 0.5 - 1.0, block),
                ],
                _ => {
                    let h = block * 0.5 - 1.0;
                    let o = block * 0.5 + 1.0;
                    vec![
                        (bx, by, h, h),
                        (bx + o, by, h, h),
                        (bx, by + o, h, h),
                        (bx + o, by + o, h, h),
                    ]
                }
            };
            for (lx, ly, sx, sy) in lots {
                let height = rng.range(6.0, 40.0);
                b.solid([lx, ly, 0.0], [lx + sx, ly + sy, height], true);
            }
        }
    }
    // Lamp posts along the streets.
    for i in 0..=n {
        let s = x0 + pitch * i as f32;
        let mut t = x0 + 3.0;
        while t < -x0 {
            b.prop([s + 1.0, t, 0.0], [s + 1.3, t + 0.3, 4.0]);
            t += rng.range(14.0, 20.0);
        }
    }
    let seeds = (0..=n)
        .flat_map(|i| (0..=n).map(move |j| (i, j)))
        .map(|(i, j)| {
            [
                x0 + 2.5 + pitch * i as f32,
                x0 + 2.5 + pitch * j as f32,
                1.7,
            ]
        })
        .collect();
    b.finish(
        "city",
        "street grid of tall buildings: strong occlusion, culling efficiency",
        aabb([-half, -half, 0.0], [half, half, 20.0]),
        seeds,
        CAMERAS,
        &mut rng,
        move |r| {
            // Eyes on the streets, mostly at standing height, some on a raised walkway.
            let along = r.range(-half, half);
            let street = x0 + pitch * r.below(n as u32 + 1) as f32 + r.range(-4.5, 4.5);
            let z = if r.chance(0.8) {
                r.range(1.2, 2.5)
            } else {
                r.range(5.0, 15.0)
            };
            if r.chance(0.5) {
                [along, street, z]
            } else {
                [street, along, z]
            }
        },
    )
}

/// Walls of thickness 0 (one two-sided quad), 1/16, 1/8, 1/4 and 1/2 m (against 0.125 m voxels),
/// each with no slit or a slit of 0.1, 0.15, 0.2, 0.25 or 0.4 m, its centre off the voxel lattice
/// by a varying fraction, a target cube behind each. Stresses leaks and the smallest-hole tolerance
/// (0.125 m), including the band up to two voxels where a slit off the lattice may close.
pub fn thin_walls() -> Scene {
    let mut rng = Rng::new(0x0003_0003);
    let mut b = Builder::new();
    b.solid([-4.0, -20.0, -1.0], [104.0, 166.0, 0.0], true);
    let thick = [0.0, 0.0625, 0.125, 0.25, 0.5];
    let slits = [0.0, 0.1, 0.15, 0.2, 0.25, 0.4];
    for (row, &slit) in slits.iter().enumerate() {
        let y = 30.0 * row as f32;
        for (col, &t) in thick.iter().enumerate() {
            let x = 4.0 + 20.0 * col as f32;
            // Offset off the voxel lattice by a varying fraction, so walls fall both on and off
            // voxel planes.
            let y = y + rng.below(8) as f32 * (0.125 / 8.0);
            let c = x + 6.0 + rng.below(8) as f32 * (0.125 / 8.0);
            let pieces = if slit > 0.0 {
                vec![(x, c - slit * 0.5), (c + slit * 0.5, x + 12.0)]
            } else {
                vec![(x, x + 12.0)]
            };
            for (a, c) in pieces {
                if t == 0.0 {
                    b.quad(
                        [[a, y, 0.0], [c, y, 0.0], [c, y, 6.0], [a, y, 6.0]],
                        true,
                        true,
                    );
                } else {
                    b.solid([a, y - t * 0.5, 0.0], [c, y + t * 0.5, 6.0], true);
                }
            }
            b.solid([x + 5.5, y + 3.0, 0.5], [x + 6.5, y + 4.0, 1.5], true);
        }
    }
    let seeds = (0..slits.len())
        .flat_map(|r| (0..5).map(move |c| [10.0 + 20.0 * c as f32, -6.0 + 30.0 * r as f32, 1.7]))
        .collect();
    let rows = slits.len() as u32;
    b.finish(
        "thin_walls",
        "walls 0 to 0.5 m thick with 0.1 to 0.4 m slits: leaks, smallest hole",
        aabb([-4.0, -20.0, 0.0], [104.0, 166.0, 10.0]),
        seeds,
        CAMERAS,
        &mut rng,
        move |r| {
            let row = r.below(rows) as f32;
            [
                r.range(0.0, 100.0),
                30.0 * row - r.range(3.0, 14.0),
                r.range(0.8, 3.0),
            ]
        },
    )
}

/// Smooth value noise in `[0, 1)` over a lattice of `cell` metres (no trigonometry).
fn value_noise(seed: u64, x: f32, y: f32, cell: f32) -> f32 {
    let lattice = |i: i64, j: i64| -> f32 {
        let key = (i as u64).wrapping_mul(0x9e37_79b9)
            ^ (j as u64).wrapping_mul(0x85eb_ca6b).rotate_left(17);
        Rng::new(seed ^ key).unit()
    };
    let (fx, fy) = (x / cell, y / cell);
    let (ix, iy) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - ix, fy - iy);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(tx), s(ty));
    let (i, j) = (ix as i64, iy as i64);
    let a = lattice(i, j) + (lattice(i + 1, j) - lattice(i, j)) * sx;
    let c = lattice(i, j + 1) + (lattice(i + 1, j + 1) - lattice(i, j + 1)) * sx;
    a + (c - a) * sy
}

/// A 192 m heightfield of hills (closed underneath by skirts and a bottom plate), rocks and
/// trees with canopies, markers on the ground. Stresses weak occlusion over a large open domain.
pub fn terrain() -> Scene {
    let mut rng = Rng::new(0x0004_0004);
    let mut b = Builder::new();
    let (half, step, n) = (96.0f32, 1.5f32, 128usize);
    let height =
        |x: f32, y: f32| 22.0 * value_noise(41, x, y, 48.0) + 6.0 * value_noise(42, x, y, 12.0);
    let at = |i: usize| -half + step * i as f32;
    // Terrain in 6×6 patches of 22 (the last 18) grid quads, one occluder object and target each.
    let patches = 6;
    let per = n.div_ceil(patches);
    let bottom = -4.0;
    for pi in 0..patches {
        for pj in 0..patches {
            let (i0, i1) = (pi * per, ((pi + 1) * per).min(n));
            let (j0, j1) = (pj * per, ((pj + 1) * per).min(n));
            let mut v = Vec::new();
            for i in i0..=i1 {
                for j in j0..=j1 {
                    v.push([at(i), at(j), height(at(i), at(j))]);
                }
            }
            let id = |i: usize, j: usize| ((i - i0) * (j1 - j0 + 1) + (j - j0)) as u32;
            let mut t = Vec::new();
            for i in i0..i1 {
                for j in j0..j1 {
                    let (a, c, d, e) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
                    t.push([a, c, d]);
                    t.push([a, d, e]);
                }
            }
            b.tris(&v, &t, true);
        }
    }
    // Skirts down to a bottom plate, so the ground is a closed solid.
    for side in 0..4 {
        let mut v = Vec::new();
        for k in 0..=n {
            let (x, y) = match side {
                0 => (at(k), -half),
                1 => (at(k), half),
                2 => (-half, at(k)),
                _ => (half, at(k)),
            };
            v.push([x, y, height(x, y)]);
            v.push([x, y, bottom]);
        }
        let t: Vec<[u32; 3]> = (0..n as u32)
            .flat_map(|k| [[2 * k, 2 * k + 1, 2 * k + 3], [2 * k, 2 * k + 3, 2 * k + 2]])
            .collect();
        b.tris(&v, &t, false);
    }
    b.quad(
        [
            [-half, -half, bottom],
            [-half, half, bottom],
            [half, half, bottom],
            [half, -half, bottom],
        ],
        true,
        false,
    );
    let ground = |x: f32, y: f32| height(x, y);
    for _ in 0..40 {
        let (x, y) = (rng.range(-90.0, 88.0), rng.range(-90.0, 88.0));
        let (sx, sy, sz) = (
            rng.range(1.0, 4.0),
            rng.range(1.0, 4.0),
            rng.range(1.0, 4.0),
        );
        let z = ground(x, y).min(ground(x + sx, y + sy)) - 0.5;
        b.solid([x, y, z], [x + sx, y + sy, z + sz + 0.5], true);
    }
    for _ in 0..60 {
        let (x, y) = (rng.range(-90.0, 90.0), rng.range(-90.0, 90.0));
        let z = ground(x, y) - 0.5;
        let h = rng.range(4.0, 8.0);
        let (tv, tt) = box_mesh([x - 0.3, y - 0.3, z], [x + 0.3, y + 0.3, z + h]);
        let (cv, ct) = box_mesh([x - 2.0, y - 2.0, z + h], [x + 2.0, y + 2.0, z + h + 3.0]);
        let mut v = tv.to_vec();
        v.extend_from_slice(&cv);
        let mut t = tt.to_vec();
        t.extend(ct.iter().map(|c| [c[0] + 8, c[1] + 8, c[2] + 8]));
        b.tris(&v, &t, true);
        b.solids.push(Aabb {
            min: [x - 0.3, y - 0.3, z],
            max: [x + 0.3, y + 0.3, z + h],
        });
        b.solids.push(Aabb {
            min: [x - 2.0, y - 2.0, z + h],
            max: [x + 2.0, y + 2.0, z + h + 3.0],
        });
    }
    for _ in 0..60 {
        let (x, y) = (rng.range(-90.0, 90.0), rng.range(-90.0, 90.0));
        let z = ground(x, y);
        b.prop([x, y, z], [x + 0.5, y + 0.5, z + 0.5]);
    }
    let seeds = grid_points(-96.0, -96.0, 32.0, 6, 0.0)
        .into_iter()
        .map(|p| [p[0], p[1], ground(p[0], p[1]) + 1.7])
        .collect();
    b.finish(
        "terrain",
        "hills, rocks and trees over 192 m: weak occlusion, large open domain",
        aabb([-96.0, -96.0, -2.0], [96.0, 96.0, 40.0]),
        seeds,
        CAMERAS,
        &mut rng,
        move |r| {
            let (x, y) = (r.range(-94.0, 94.0), r.range(-94.0, 94.0));
            [x, y, ground(x, y) + r.range(1.5, 2.5)]
        },
    )
}

/// A sealed 12×12 maze of 4 m corridors (floor, ceiling, closed outer wall), loops added.
/// Only seeds reach it. Stresses enclosed space and seed handling.
pub fn maze() -> Scene {
    let mut rng = Rng::new(0x0005_0005);
    let mut b = Builder::new();
    let (n, cell, h, t) = (12usize, 4.0f32, 3.0f32, 0.25f32);
    let side = cell * n as f32;
    // Perfect maze by randomised depth-first search; walls[k] true = wall present.
    // East walls: (i, j) between (i, j) and (i + 1, j); north walls likewise.
    let mut east = vec![true; n * n];
    let mut north = vec![true; n * n];
    let mut seen = vec![false; n * n];
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some(&(i, j)) = stack.last() {
        let mut next = Vec::new();
        if i + 1 < n && !seen[(i + 1) * n + j] {
            next.push((i + 1, j));
        }
        if i > 0 && !seen[(i - 1) * n + j] {
            next.push((i - 1, j));
        }
        if j + 1 < n && !seen[i * n + j + 1] {
            next.push((i, j + 1));
        }
        if j > 0 && !seen[i * n + j - 1] {
            next.push((i, j - 1));
        }
        if next.is_empty() {
            stack.pop();
            continue;
        }
        let (a, c) = next[rng.below(next.len() as u32) as usize];
        if a != i {
            east[a.min(i) * n + j] = false;
        } else {
            north[i * n + c.min(j)] = false;
        }
        seen[a * n + c] = true;
        stack.push((a, c));
    }
    for w in east.iter_mut().chain(north.iter_mut()) {
        if *w && rng.chance(0.1) {
            *w = false;
        }
    }
    b.solid([-t, -t, -0.5], [side + t, side + t, 0.0], true);
    b.solid([-t, -t, h], [side + t, side + t, h + 0.5], true);
    for k in 0..4 {
        let (mn, mx) = match k {
            0 => ([-t, -t, 0.0], [side + t, 0.0, h]),
            1 => ([-t, side, 0.0], [side + t, side + t, h]),
            2 => ([-t, 0.0, 0.0], [0.0, side, h]),
            _ => ([side, 0.0, 0.0], [side + t, side, h]),
        };
        b.solid(mn, mx, true);
    }
    for i in 0..n {
        for j in 0..n {
            let (x, y) = (cell * i as f32, cell * j as f32);
            if i + 1 < n && east[i * n + j] {
                b.solid(
                    [x + cell - t * 0.5, y, 0.0],
                    [x + cell + t * 0.5, y + cell, h],
                    true,
                );
            }
            if j + 1 < n && north[i * n + j] {
                b.solid(
                    [x, y + cell - t * 0.5, 0.0],
                    [x + cell, y + cell + t * 0.5, h],
                    true,
                );
            }
            if rng.chance(0.4) {
                let (px, py) = (x + rng.range(0.6, 2.9), y + rng.range(0.6, 2.9));
                b.prop([px, py, 0.0], [px + 0.5, py + 0.5, 0.6]);
            }
        }
    }
    let seeds = grid_points(0.0, 0.0, cell, n, 1.5);
    b.finish(
        "maze",
        "sealed maze of corridors under a ceiling: enclosed space, seeds",
        aabb([0.0, 0.0, 0.0], [side, side, h]),
        seeds,
        CAMERAS,
        &mut rng,
        move |r| {
            let (i, j) = (r.below(n as u32) as f32, r.below(n as u32) as f32);
            [
                cell * i + r.range(0.5, 3.5),
                cell * j + r.range(0.5, 3.5),
                r.range(1.2, 2.0),
            ]
        },
    )
}

/// Geometry on and across the 32 m tile seams: walls on seam planes (one zero-thickness quad
/// exactly on x = 32), boxes on tile corners, walls on and half a voxel off voxel planes,
/// objects touching the domain boundary, a target reaching past the view volume.
pub fn seams() -> Scene {
    let mut rng = Rng::new(0x0006_0006);
    let mut b = Builder::new();
    b.solid([-66.0, -66.0, -1.0], [66.0, 66.0, 0.0], true);
    // Walls with gaps, on the seams x = 0 and y = -32.
    b.solid([-0.125, -60.0, 0.0], [0.125, -4.0, 8.0], true);
    b.solid([-0.125, 4.0, 0.0], [0.125, 60.0, 8.0], true);
    b.solid([-60.0, -32.125, 0.0], [-36.0, -31.875, 8.0], true);
    b.solid([-28.0, -32.125, 0.0], [28.0, -31.875, 8.0], true);
    // A zero-thickness two-sided wall exactly on the seam x = 32.
    b.quad(
        [
            [32.0, -20.0, 0.0],
            [32.0, 20.0, 0.0],
            [32.0, 20.0, 8.0],
            [32.0, -20.0, 8.0],
        ],
        true,
        true,
    );
    // Walls on a voxel plane and half a voxel off it.
    b.solid([16.0, 24.0, 0.0], [16.25, 56.0, 8.0], true);
    b.solid([-16.0625, 24.0, 0.0], [-15.8125, 56.0, 8.0], true);
    // Boxes centred on tile corners, one spanning a seam in z (tile 0..32 into 32..64).
    for &(x, y) in &[(0.0, 0.0), (32.0, 32.0), (-32.0, 32.0), (32.0, -32.0)] {
        b.solid([x - 3.0, y - 3.0, 0.0], [x + 3.0, y + 3.0, 4.0], true);
    }
    b.solid([-34.0, -2.0, 0.0], [-30.0, 2.0, 40.0], true);
    // Touching the domain boundary, and a target reaching past the view volume.
    b.solid([62.0, -10.0, 0.0], [64.0, 10.0, 6.0], true);
    b.solid([-64.0, 40.0, 0.0], [-60.0, 44.0, 16.0], true);
    b.prop([58.0, 40.0, 0.0], [70.0, 44.0, 3.0]);
    // Small props scattered across seams.
    for _ in 0..80 {
        let (x, y) = (
            32.0 * rng.below(4) as f32 - 48.0 + rng.range(-1.0, 1.0),
            rng.range(-62.0, 62.0),
        );
        b.prop([x, y, 0.0], [x + 0.4, y + 0.4, 0.8]);
    }
    let seeds = grid_points(-64.0, -64.0, 32.0, 4, 1.7);
    b.finish(
        "seams",
        "geometry on tile seams, voxel planes and the domain boundary",
        aabb([-64.0, -64.0, 0.0], [64.0, 64.0, 16.0]),
        seeds,
        CAMERAS,
        &mut rng,
        |r| {
            [
                r.range(-63.0, 63.0),
                r.range(-63.0, 63.0),
                r.range(1.2, 6.0),
            ]
        },
    )
}

/// 300 rotated, scaled copies of one L-shaped wall over a 160 m floor, given both as instances
/// and as their applied soup. Stresses the instanced input path (must bake byte-identically to
/// the soup).
pub fn instanced() -> Scene {
    let mut rng = Rng::new(0x0007_0007);
    let (av, at) = box_mesh([0.0, 0.0, 0.0], [6.0, 0.5, 4.0]);
    let (bv, bt) = box_mesh([0.0, 0.5, 0.0], [0.5, 5.0, 4.0]);
    let mut wall = Mesh::default();
    wall.vertices.extend_from_slice(&av);
    wall.vertices.extend_from_slice(&bv);
    wall.triangles.extend_from_slice(&at);
    wall.triangles
        .extend(bt.iter().map(|t| [t[0] + 8, t[1] + 8, t[2] + 8]));
    wall.owners = vec![0; wall.triangles.len()];
    let (fv, ft) = box_mesh([-84.0, -84.0, -1.0], [84.0, 84.0, 0.0]);
    let floor = Mesh {
        vertices: fv.to_vec(),
        triangles: ft.to_vec(),
        owners: vec![0; 12],
    };
    let meshes = vec![floor, wall];
    let mut instances = vec![Instance {
        mesh: 0,
        transform: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
    }];
    for _ in 0..300 {
        // A rotation about z from the rational parametrisation (exact, no trigonometry).
        let t = rng.range(-1.0, 1.0);
        let d = 1.0 + t * t;
        let (c, s) = ((1.0 - t * t) / d, 2.0 * t / d);
        let (c, s) = if rng.chance(0.5) { (-c, -s) } else { (c, s) };
        let k = rng.range(0.6, 1.4);
        let (x, y) = (rng.range(-76.0, 76.0), rng.range(-76.0, 76.0));
        instances.push(Instance {
            mesh: 1,
            transform: [
                [c * k, -s * k, 0.0, x],
                [s * k, c * k, 0.0, y],
                [0.0, 0.0, k, 0.0],
            ],
        });
    }
    let mut b = Builder::new();
    for inst in &instances {
        let m = &meshes[inst.mesh as usize];
        let v: Vec<Vec3> = m.vertices.iter().map(|&p| inst.apply(p)).collect();
        b.tris(&v, &m.triangles, true);
        if inst.mesh == 1 {
            // The two boxes' world boxes (larger than the boxes once rotated) keep eyes out.
            b.solids.push(bounds_of(&v[..8]));
            b.solids.push(bounds_of(&v[8..]));
        } else {
            b.solids.push(bounds_of(&v));
        }
    }
    let seeds = grid_points(-80.0, -80.0, 32.0, 5, 1.7);
    let mut scene = b.finish(
        "instanced",
        "300 rotated copies of one mesh: instanced input equals its soup",
        aabb([-80.0, -80.0, 0.0], [80.0, 80.0, 10.0]),
        seeds,
        CAMERAS,
        &mut rng,
        |r| {
            [
                r.range(-78.0, 78.0),
                r.range(-78.0, 78.0),
                r.range(1.2, 2.5),
            ]
        },
    );
    scene.instanced = Some((meshes, instances));
    scene
}

/// Bad geometry over a floor: open boxes, coplanar overlapping quads, duplicate and zero-area
/// triangles, slivers, flipped winding, non-manifold edges, tiny triangles, a huge triangle
/// reaching past the domain. Stresses robustness.
pub fn degenerate() -> Scene {
    let mut rng = Rng::new(0x0008_0008);
    let mut b = Builder::new();
    b.solid([-50.0, -50.0, -1.0], [50.0, 50.0, 0.0], true);
    // Open boxes: no top, and one missing a side.
    let (v, t) = box_mesh([-40.0, -40.0, 0.0], [-30.0, -30.0, 5.0]);
    b.tris(
        &v,
        &[t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8], t[9]],
        true,
    );
    let (v, t) = box_mesh([-20.0, -40.0, 0.0], [-10.0, -30.0, 5.0]);
    b.tris(
        &v,
        &[t[0], t[1], t[2], t[3], t[6], t[7], t[8], t[9], t[10], t[11]],
        true,
    );
    // Two coplanar overlapping quads.
    b.quad(
        [
            [0.0, -40.0, 0.0],
            [10.0, -40.0, 0.0],
            [10.0, -40.0, 5.0],
            [0.0, -40.0, 5.0],
        ],
        true,
        true,
    );
    b.quad(
        [
            [5.0, -40.0, 0.0],
            [15.0, -40.0, 0.0],
            [15.0, -40.0, 5.0],
            [5.0, -40.0, 5.0],
        ],
        true,
        true,
    );
    // A box with every triangle twice, a zero-area triangle and a repeated index.
    let (v, t) = box_mesh([25.0, -40.0, 0.0], [32.0, -33.0, 4.0]);
    let mut tris = t.to_vec();
    tris.extend_from_slice(&t);
    tris.push([0, 0, 1]);
    let mut verts = v.to_vec();
    verts.extend([[28.0, -36.0, 1.0], [29.0, -36.0, 1.0], [30.0, -36.0, 1.0]]);
    tris.push([8, 9, 10]);
    b.tris(&verts, &tris, true);
    b.solids.push(Aabb {
        min: [25.0, -40.0, 0.0],
        max: [32.0, -33.0, 4.0],
    });
    // Slivers: a fan of long thin triangles forming a wall.
    let mut v = Vec::new();
    for k in 0..200u32 {
        let x = -40.0 + 0.05 * k as f32;
        v.extend([
            [x, -10.0, 0.0],
            [x + 0.05, -10.0, 0.0],
            [x + 0.025, -10.0, 6.0],
        ]);
    }
    let tris: Vec<[u32; 3]> = (0..200u32).map(|k| [3 * k, 3 * k + 1, 3 * k + 2]).collect();
    b.tris(&v, &tris, true);
    // A flipped-winding box.
    let (v, t) = box_mesh([0.0, -15.0, 0.0], [6.0, -9.0, 5.0]);
    let flipped: Vec<[u32; 3]> = t.iter().map(|x| [x[0], x[2], x[1]]).collect();
    b.solids.push(Aabb {
        min: v[0],
        max: v[7],
    });
    b.tris(&v, &flipped, true);
    // Non-manifold: three quads sharing one edge.
    b.tris(
        &[
            [20.0, -10.0, 0.0],
            [20.0, -10.0, 5.0],
            [26.0, -10.0, 0.0],
            [26.0, -10.0, 5.0],
            [20.0, -4.0, 0.0],
            [20.0, -4.0, 5.0],
            [16.0, -14.0, 0.0],
            [16.0, -14.0, 5.0],
        ],
        &[
            [0, 2, 3],
            [0, 3, 1],
            [0, 4, 5],
            [0, 5, 1],
            [0, 6, 7],
            [0, 7, 1],
        ],
        true,
    );
    // Tiny triangles (1 mm) in a cloud.
    let mut v = Vec::new();
    for _ in 0..500 {
        let (x, y, z) = (
            rng.range(30.0, 40.0),
            rng.range(-10.0, 0.0),
            rng.range(0.5, 4.0),
        );
        v.extend([[x, y, z], [x + 0.001, y, z], [x, y + 0.001, z]]);
    }
    let t: Vec<[u32; 3]> = (0..500u32).map(|k| [3 * k, 3 * k + 1, 3 * k + 2]).collect();
    b.tris(&v, &t, true);
    // A huge sloped triangle reaching far past the domain.
    b.tris(
        &[[-500.0, 20.0, 0.5], [500.0, 20.0, 0.5], [0.0, 900.0, 300.0]],
        &[[0, 1, 2]],
        false,
    );
    // Plain crates as a control, and props.
    for _ in 0..12 {
        let (x, y) = (rng.range(-45.0, 40.0), rng.range(0.0, 15.0));
        b.solid([x, y, 0.0], [x + 2.0, y + 2.0, 2.0], true);
    }
    for _ in 0..30 {
        let (x, y) = (rng.range(-45.0, 45.0), rng.range(-45.0, 45.0));
        b.prop([x, y, 0.0], [x + 0.5, y + 0.5, 0.5]);
    }
    let seeds = grid_points(-48.0, -48.0, 24.0, 4, 1.7);
    let scene = b.finish(
        "degenerate",
        "open, overlapping, duplicate, zero-area, sliver, flipped and non-manifold geometry",
        aabb([-48.0, -48.0, 0.0], [48.0, 48.0, 10.0]),
        seeds,
        CAMERAS,
        &mut rng,
        |r| {
            [
                r.range(-47.0, 47.0),
                r.range(-47.0, 47.0),
                r.range(1.2, 2.5),
            ]
        },
    );
    debug_assert!(
        scene
            .targets
            .iter()
            .all(|t: &SceneTarget| t.bounds.min[0] <= t.bounds.max[0])
    );
    scene
}

/// The committed real-world asset: Amazon Lumberyard Bistro, exterior and interior in one space
/// (`assets/bistro.tbsc`, CC BY 4.0, attribution and changes in `assets/BISTRO.md`). Streets,
/// facades, terraces, and the bistro's interior opening onto the street through its glass, which
/// does not occlude. Stresses real-world density, object sizes and the indoor/outdoor transition.
pub fn bistro() -> Scene {
    let d = asset("bistro.tbsc");
    let bvh = Bvh::build(&d.mesh);
    let mut rng = Rng::new(0x0009_0009);
    let (lo, hi) = ([-62.0f32, -96.0f32], [111.0f32, 87.5f32]);
    // The first surface straight down from `top` at (x, y), if any.
    let mut hits = Vec::new();
    let mut first_below = move |x: f32, y: f32, top: f32| -> Option<f32> {
        hits.clear();
        bvh.crossings([x, y, top], [x, y, top - 20.0], &mut hits);
        hits.iter()
            .copied()
            .min_by(f32::total_cmp)
            .map(|t| top - 20.0 * t)
    };
    let mut b = Builder::new();
    b.mesh = d.mesh;
    b.targets = d.targets;
    let mut scene = b.finish(
        "bistro",
        "Amazon Lumberyard Bistro (CC BY 4.0), exterior and interior: real-world geometry",
        aabb([lo[0], lo[1], -0.5], [hi[0], hi[1], 6.0]),
        Vec::new(),
        CAMERAS,
        &mut rng,
        move |r| {
            if r.chance(0.25) {
                // Inside the bistro: the floor (z 0.2..0.6) under a point just below the ceiling.
                let (x, y) = (r.range(-1.8, 13.3), r.range(-5.2, 13.7));
                match first_below(x, y, 3.9) {
                    Some(z) if (0.2..0.6).contains(&z) => [x, y, z + r.range(1.4, 2.0)],
                    _ => [f32::NAN; 3],
                }
            } else {
                // Outdoors: street level (z -0.5..1.5) is the first surface under the open sky.
                let (x, y) = (r.range(lo[0], hi[0]), r.range(lo[1], hi[1]));
                match first_below(x, y, 60.0) {
                    Some(z) if (-0.5..1.5).contains(&z) => [x, y, z + r.range(1.5, 2.5)],
                    _ => [f32::NAN; 3],
                }
            }
        },
    );
    // Seeds: the first 40 camera eyes (open space the camera reaches, indoors and out).
    scene.seeds = scene.cameras.iter().take(40).map(|c| c.eye).collect();
    scene
}

/// A committed TBSC asset from `assets/`.
fn asset(name: &str) -> crate::tbsc::Decoded {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    crate::tbsc::decode(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `p` is enclosed: each of the six axis rays from it crosses the geometry an odd number of times
/// (inside a closed column, wall or slab).
fn enclosed(bvh: &Bvh, p: Vec3, hits: &mut Vec<f32>) -> bool {
    (0..6).all(|r| {
        let mut far = p;
        far[r / 2] += if r % 2 == 0 { 1000.0 } else { -1000.0 };
        hits.clear();
        bvh.crossings(p, far, hits);
        hits.len() % 2 == 1
    })
}

/// The committed real-world asset: Crytek Sponza (Frank Meinl, 2010, with Morgan McGuire's 2011
/// cleanup; CC BY 3.0, attribution and changes in `assets/SPONZA.md`). An atrium open to the sky,
/// ringed by a ground-floor colonnade and two galleries, with hanging fabric. Stresses arcades
/// (many openings between pillars) and vertical sight lines across levels.
pub fn sponza() -> Scene {
    let d = asset("sponza.tbsc");
    let bvh = Bvh::build(&d.mesh);
    let mut rng = Rng::new(0x000a_000a);
    let (lo, hi) = ([-18.9f32, -10.8f32], [17.7f32, 11.5f32]);
    // Levels: (ray start just under the slab above, floor heights accepted, measured with
    // first-surface probes: courtyard -1.2, ground floor -0.1, galleries 4.1 and 8.4-8.6).
    let levels: [(f32, f32, f32); 3] = [(3.0, -1.3, 0.0), (7.0, 3.9, 4.3), (11.5, 8.3, 8.7)];
    let mut hits = Vec::new();
    let mut b = Builder::new();
    b.mesh = d.mesh;
    b.targets = d.targets;
    let mut scene = b.finish(
        "sponza",
        "Crytek Sponza (CC BY 3.0): atrium, colonnade and two galleries; real-world geometry",
        aabb([lo[0], lo[1], -1.3], [hi[0], hi[1], 11.0]),
        Vec::new(),
        CAMERAS,
        &mut rng,
        move |r| {
            // Half the eyes on the ground floor, a quarter on each gallery.
            let level = match r.below(4) {
                0 | 1 => 0,
                2 => 1,
                _ => 2,
            };
            let (top, f0, f1) = levels[level];
            let (x, y) = (r.range(lo[0], hi[0]), r.range(lo[1], hi[1]));
            hits.clear();
            bvh.crossings([x, y, top], [x, y, top - 20.0], &mut hits);
            let floor = hits
                .iter()
                .copied()
                .min_by(f32::total_cmp)
                .map(|t| top - 20.0 * t);
            match floor {
                Some(z) if (f0..f1).contains(&z) => {
                    let eye = [x, y, z + r.range(1.4, 2.0)];
                    if enclosed(&bvh, eye, &mut hits) {
                        [f32::NAN; 3]
                    } else {
                        eye
                    }
                }
                _ => [f32::NAN; 3],
            }
        },
    );
    scene.seeds = scene.cameras.iter().take(40).map(|c| c.eye).collect();
    scene
}
