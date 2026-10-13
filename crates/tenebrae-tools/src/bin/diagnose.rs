//! `diagnose SCENE SEEDS SEGMENTS OUT minx miny minz maxx maxy maxz tile voxel flood|flood-open|parity`
//!
//! The solid mode reads as in `bake`: `flood` is `Params::default()`, `flood-open` stops surface
//! voxels occluding, `flood-occluding` is an older spelling of `flood`.
//!
//! Voxelises like `bake` and classifies segments by the voxels they cross, excluding the voxels
//! of both end points. SEGMENTS: u32 count, then f32[6] (eye, point) per segment. OUT: one byte
//! per segment: 0 = crosses no solid voxel, 1 = crosses a solid voxel no triangle touches (flood
//! solid), 2 = crosses only solid voxels that triangles touch (surface solid), 3 = clear but the point's own voxel is solid.

use tenebrae::bake::{Input, Occluders, Params, SolidMode, voxelize};
use tenebrae::geom::Mesh;
use tenebrae::tome::Aabb;

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn le_f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(le_u32(b, at))
}
fn points(b: &[u8], per: usize) -> Vec<Vec<f32>> {
    (0..le_u32(b, 0) as usize)
        .map(|k| (0..per).map(|j| le_f32(b, 4 + 4 * (per * k + j))).collect())
        .collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 14 {
        eprintln!(
            "usage: diagnose SCENE SEEDS SEGMENTS OUT minx miny minz maxx maxy maxz tile voxel solid"
        );
        std::process::exit(2);
    }
    let f = |i: usize| a[i].parse::<f32>().expect("number");
    let scene = std::fs::read(&a[1]).expect("scene");
    let nv = le_u32(&scene, 0) as usize;
    let nt = le_u32(&scene, 4) as usize;
    let mut mesh = Mesh::default();
    for k in 0..nv {
        let o = 8 + 12 * k;
        mesh.vertices.push([
            le_f32(&scene, o),
            le_f32(&scene, o + 4),
            le_f32(&scene, o + 8),
        ]);
    }
    for k in 0..nt {
        let o = 8 + 12 * nv + 12 * k;
        mesh.triangles.push([
            le_u32(&scene, o),
            le_u32(&scene, o + 4),
            le_u32(&scene, o + 8),
        ]);
    }
    mesh.owners = vec![0; nt];
    let seeds: Vec<[f32; 3]> = points(&std::fs::read(&a[2]).expect("seeds"), 3)
        .into_iter()
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    let segs = points(&std::fs::read(&a[3]).expect("segments"), 6);
    let defaults = Params::default();
    let (solid, surface_occludes) = match a[13].as_str() {
        "parity" => (SolidMode::Parity, defaults.surface_occludes),
        "flood" | "flood-occluding" => (SolidMode::Flood, defaults.surface_occludes),
        "flood-open" => (SolidMode::Flood, false),
        other => {
            eprintln!("bad solid mode {other}: flood|flood-open|parity");
            std::process::exit(2)
        }
    };
    let params = Params {
        tile: f(11),
        voxel: f(12),
        solid,
        surface_occludes,
        ..defaults
    };
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &[],
        view_volume: Aabb {
            min: [f(5), f(6), f(7)],
            max: [f(8), f(9), f(10)],
        },
        seeds: &seeds,
        params,
    };
    let vox = voxelize(&input).expect("voxelize");
    let cell = |p: &[f32]| -> Option<[usize; 3]> {
        let mut v = [0usize; 3];
        for k in 0..3 {
            let x = ((p[k] - vox.origin[k]) / vox.voxel).floor();
            if x < 0.0 || x as usize >= vox.dims[k] {
                return None;
            }
            v[k] = x as usize;
        }
        Some(v)
    };
    let mut out = Vec::with_capacity(segs.len());
    let mut counts = [0usize; 4];
    for s in &segs {
        let (e, q) = (&s[0..3], &s[3..6]);
        let (ce, cq) = (cell(e), cell(q));
        let len = ((0..3).map(|k| (q[k] - e[k]).powi(2)).sum::<f32>()).sqrt();
        let steps = ((len / (vox.voxel * 0.1)).ceil() as usize).max(1);
        let mut class = 0u8;
        for i in 1..steps {
            let t = i as f32 / steps as f32;
            let p: Vec<f32> = (0..3).map(|k| e[k] + (q[k] - e[k]) * t).collect();
            let Some(v) = cell(&p) else { continue };
            if Some(v) == ce || Some(v) == cq {
                continue;
            }
            if vox.solid(v) {
                if !vox.surface(v) {
                    class = 1;
                    break;
                }
                class = 2;
            }
        }
        if class == 0 && cq.is_some_and(|v| vox.solid(v)) {
            class = 3;
        }
        counts[class as usize] += 1;
        out.push(class);
    }
    eprintln!(
        "segments {}: clear {} | flood solid {} | surface solid only {} | clear, end point in solid {}",
        segs.len(),
        counts[0],
        counts[1],
        counts[2],
        counts[3]
    );
    std::fs::write(&a[4], out).expect("write");
}
