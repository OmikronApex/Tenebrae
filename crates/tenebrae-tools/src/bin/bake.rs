//! `bake SCENE TARGETS OUT.tome minx miny minz maxx maxy maxz [tile voxel [boxes|components[B]|mergedF[bB] [flood|flood-open|parity [SEEDS [threads [reach-threshold [collapse-size [memory-limit-MiB [empty-leaves [cell-merge [global-groups [midpoint-views [cache-file [keep-every-target [lod-tiles [base-distance [domain-margin [group-cost]]]]]]]]]]]]]]]]]]`
//!
//! Positionals are append-only; every omitted one is `Params::default()`. B (block) is in metres.
//! `flood` uses `Params::default()` (surface voxels occlude); `flood-open` turns that off;
//! `flood-occluding` is an older spelling of `flood`. `threads` is the worker count,
//! `reach-threshold` the unreachable-cell share, `collapse-size` the leaf-tile collapse extent in
//! metres, `memory-limit-MiB` the grid storage limit. The Umbra-parity levers: `empty-leaves`
//! (0/1, large leaf tiles over empty space), `cell-merge` (metres, per-tile cell merge, 0 = off),
//! `global-groups` (0/1, global cells as connected groups of a tile's cells), `midpoint-views`
//! (0/1, every view-tree split an implicit midpoint, no split floats stored).
//!
//! `keep-every-target` (0/1): object `i` is target `i` (`Params::keep_every_target`); the map
//! blob is then the identity and no target is dropped. `lod-tiles` (0/1): lever L5, coarse LOD
//! tiles on inner top-tree nodes (`Params::lod_tiles`). `base-distance` (metres): header `+0x0C`
//! and the LOD distances (`Params::base_distance`). `domain-margin` (metres): how far past the
//! view volume the domain grows over the targets (`Params::domain_margin`, 0 = none).
//! `group-cost`: Umbra's object grouping of the targets (`Params::group_cost`, 0 = off); the
//! map blob then names each target's group.
//!
//! `cache-file` turns on the incremental re-bake (`bake` with a cache): when the file exists it is read
//! as the tile cache of an earlier bake (a malformed cache, or one of another version, is noted on
//! stderr and ignored, and the bake is then a full one; a cache of another grid or other tile
//! settings simply misses), and after the bake the file is replaced with this bake's cache
//! (written to `<cache-file>.tmp`, then renamed; a failure is a warning, the tome is kept). An
//! empty `cache-file` means no cache.
//! The cache blob: magic `TBTC`, u32 version 1, u128 checksum (FNV-1a 128 of every byte after
//! it), u32 pass-A entry count, u32 pass-B entry count, then per entry (pass A, then pass B, keys
//! ascending) u128 key, u64 length and an opaque payload. Its contents are internal to the
//! version; only the library reads them. The hits per pass are printed on stderr.
//!
//! SEEDS: u32 count then f32[3] per point (open space the camera reaches).
//!
//! SCENE: the `truth` scene format (owners ignored). TARGETS (little endian): u32 count, then per
//! target f32[6] box (min, max) and u32 user id. The view volume is given on the command line.
//!
//! Writes OUT.tome, OUT.tome.samples.bin (u32 global-cell count, then per global cell, in the
//! tome's `+0x80` order, u32 count and f32[3] per open-voxel sample; with global groups a global
//! cell is a group of a tile's cells, so this count can be below the `cells` the report prints,
//! which counts tile cells) and OUT.tome.map.bin (the object map: magic `TBMP`, u32 version 1, u32
//! target count, then per target u32 object index, `u32::MAX` when the target was dropped).
//! Prints `scene loaded` on stderr once the inputs are read, before baking.

use std::time::Instant;

use tenebrae::bake::{CellMode, Input, Occluders, Params, SolidMode, Target, bake};
use tenebrae::geom::Mesh;
use tenebrae::tome::Aabb;

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn le_f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(le_u32(b, at))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 10 {
        eprintln!("usage: bake SCENE TARGETS OUT.tome minx miny minz maxx maxy maxz [tile voxel]");
        std::process::exit(2);
    }
    let f = |i: usize| {
        a[i].parse::<f32>().unwrap_or_else(|_| {
            eprintln!("bad number {}", a[i]);
            std::process::exit(2)
        })
    };
    let scene = std::fs::read(&a[1]).expect("scene");
    let nv = le_u32(&scene, 0) as usize;
    let nt = le_u32(&scene, 4) as usize;
    let mut mesh = Mesh::default();
    let mut at = 8;
    for _ in 0..nv {
        mesh.vertices.push([
            le_f32(&scene, at),
            le_f32(&scene, at + 4),
            le_f32(&scene, at + 8),
        ]);
        at += 12;
    }
    for _ in 0..nt {
        mesh.triangles.push([
            le_u32(&scene, at),
            le_u32(&scene, at + 4),
            le_u32(&scene, at + 8),
        ]);
        at += 12;
    }
    mesh.owners = vec![0; nt];
    let tb = std::fs::read(&a[2]).expect("targets");
    let n = le_u32(&tb, 0) as usize;
    let users: Vec<u32> = (0..n).map(|k| le_u32(&tb, 4 + 28 * k + 24)).collect();
    let targets: Vec<Target> = (0..n)
        .map(|k| {
            let o = 4 + 28 * k;
            Target {
                bounds: Aabb {
                    min: [le_f32(&tb, o), le_f32(&tb, o + 4), le_f32(&tb, o + 8)],
                    max: [
                        le_f32(&tb, o + 12),
                        le_f32(&tb, o + 16),
                        le_f32(&tb, o + 20),
                    ],
                },
                users: std::slice::from_ref(&users[k]),
            }
        })
        .collect();
    let mut params = Params::default();
    if a.len() >= 12 {
        params.tile = f(10);
        params.voxel = f(11);
    }
    if a.len() >= 13 {
        params.cells = match a[12].as_str() {
            "boxes" => CellMode::Boxes,
            m if m.starts_with("components") => CellMode::Components {
                // componentsN: components inside blocks of N metres (default: the tile).
                block: m["components".len()..].parse::<f32>().unwrap_or(0.0),
            },
            m => {
                // mergedF[bB]: merge to fill F, seeded by components of B-metre blocks (default:
                // greedy boxes).
                let spec = m.strip_prefix("merged").unwrap_or("");
                let (fill, block) = spec.split_once('b').unwrap_or((spec, ""));
                CellMode::Merged {
                    fill: fill.parse().unwrap_or(0.9),
                    block: block.parse::<f32>().unwrap_or(0.0),
                }
            }
        };
    }
    if a.len() >= 14 {
        match a[13].as_str() {
            "parity" => params.solid = SolidMode::Parity,
            "flood" | "flood-occluding" => params.solid = SolidMode::Flood,
            "flood-open" => {
                params.solid = SolidMode::Flood;
                params.surface_occludes = false;
            }
            other => {
                eprintln!("bad solid mode {other}: flood|flood-open|parity");
                std::process::exit(2)
            }
        }
    }
    // Tuning knobs for experiments, kept out of the positional interface.
    if let Some(cap) = std::env::var("TENEBRAE_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        params.max_cells_per_tile = cap;
    }
    let seeds: Vec<[f32; 3]> = if a.len() >= 15 {
        let sb = std::fs::read(&a[14]).expect("seeds");
        (0..le_u32(&sb, 0) as usize)
            .map(|k| {
                [
                    le_f32(&sb, 4 + 12 * k),
                    le_f32(&sb, 8 + 12 * k),
                    le_f32(&sb, 12 + 12 * k),
                ]
            })
            .collect()
    } else {
        Vec::new()
    };
    if a.len() >= 16 {
        params.threads = a[15].parse().unwrap_or_else(|_| {
            eprintln!("bad thread count {}", a[15]);
            std::process::exit(2)
        });
    }
    if a.len() >= 17 {
        params.reachability_threshold = f(16);
    }
    if a.len() >= 18 {
        params.collapse_size = f(17);
    }
    if a.len() >= 19 {
        params.memory_limit = a[18].parse::<u64>().map_or_else(
            |_| {
                eprintln!("bad memory limit {}", a[18]);
                std::process::exit(2)
            },
            |m| {
                m.checked_mul(1 << 20).unwrap_or_else(|| {
                    eprintln!("memory limit {m} MiB overflows u64");
                    std::process::exit(2)
                })
            },
        );
    }
    let lever = |i: usize| match a[i].as_str() {
        "0" => false,
        "1" => true,
        other => {
            eprintln!("bad lever {other}: 0|1");
            std::process::exit(2)
        }
    };
    if a.len() >= 20 {
        params.empty_leaves = lever(19);
    }
    if a.len() >= 21 {
        params.cell_merge = f(20);
    }
    if a.len() >= 22 {
        params.global_groups = lever(21);
    }
    if a.len() >= 23 {
        params.midpoint_views = lever(22);
    }
    if a.len() >= 25 {
        params.keep_every_target = lever(24);
    }
    if a.len() >= 26 {
        params.lod_tiles = lever(25);
    }
    if a.len() >= 27 {
        params.base_distance = f(26);
    }
    if a.len() >= 28 {
        params.domain_margin = f(27);
    }
    if a.len() >= 29 {
        params.group_cost = f(28);
    }
    eprintln!(
        "scene loaded: {} vertices, {} triangles, {} targets, {} seeds",
        mesh.vertices.len(),
        mesh.triangles.len(),
        targets.len(),
        seeds.len()
    );
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &targets,
        view_volume: Aabb {
            min: [f(4), f(5), f(6)],
            max: [f(7), f(8), f(9)],
        },
        seeds: &seeds,
        params,
    };
    // An empty positional 23 means no cache, so scripts can pass it through unconditionally.
    let cache_path = a.get(23).filter(|p| !p.is_empty());
    let cache = match cache_path.map(std::fs::read) {
        Some(Ok(bytes)) => {
            eprintln!("cache loaded: {} bytes", bytes.len());
            bytes
        }
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("cache {} unreadable ({e}): full bake", a[23]);
            Vec::new()
        }
        _ => Vec::new(),
    };
    let t0 = Instant::now();
    // Without a cache file, the plain bake: no keys hashed, no tile encoded.
    match bake(&input, cache_path.map(|_| cache.as_slice()), None) {
        Ok(baked) => {
            let (tome, report, map, next) =
                (baked.tome, baked.report, baked.object_map, baked.cache);
            let bytes = tenebrae::write(&tome);
            std::fs::write(&a[3], &bytes).expect("write");
            // OUT.samples.bin: u32 cells, then per cell u32 count and f32[3] per open-voxel sample.
            let mut s = Vec::new();
            s.extend_from_slice(&(report.cell_samples.len() as u32).to_le_bytes());
            for list in &report.cell_samples {
                s.extend_from_slice(&(list.len() as u32).to_le_bytes());
                for p in list {
                    for v in p {
                        s.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
            std::fs::write(format!("{}.samples.bin", a[3]), s).expect("write samples");
            let mut m = Vec::with_capacity(12 + 4 * map.len());
            m.extend_from_slice(b"TBMP");
            m.extend_from_slice(&1u32.to_le_bytes());
            m.extend_from_slice(&(map.len() as u32).to_le_bytes());
            for o in &map {
                m.extend_from_slice(&o.unwrap_or(u32::MAX).to_le_bytes());
            }
            std::fs::write(format!("{}.map.bin", a[3]), m).expect("write object map");
            eprintln!(
                "Report {{ tiles: {}, voxels: {:?}, cells: {}, portals: {}, buried_targets: {} }}",
                report.tiles, report.voxels, report.cells, report.portals, report.buried_targets
            );
            let mut per_tile: Vec<usize> =
                tome.tiles.iter().flatten().map(|t| t.cells.len()).collect();
            per_tile.sort_unstable();
            eprintln!(
                "grid_bytes {} ({} MiB), dropped targets {}, cells per tile: median {}, p90 {}, max {}",
                report.grid_bytes,
                report.grid_bytes >> 20,
                report.dropped_targets,
                per_tile.get(per_tile.len() / 2).copied().unwrap_or(0),
                per_tile.get(per_tile.len() * 9 / 10).copied().unwrap_or(0),
                per_tile.last().copied().unwrap_or(0)
            );
            if let (Some(path), Some(cached)) = (cache_path, next) {
                // Written beside the target and renamed over it, so an interrupted write never
                // leaves a half cache; a failure only costs the next bake its cache.
                let tmp = format!("{path}.tmp");
                if let Err(e) =
                    std::fs::write(&tmp, &cached).and_then(|()| std::fs::rename(&tmp, path))
                {
                    eprintln!("warning: cache {path} not written: {e}");
                    let _ = std::fs::remove_file(&tmp);
                }
                let (h, m) = (report.cache_hits, report.cache_misses);
                eprintln!(
                    "cache hits: pass A {}/{}, pass B {}/{}{}; cache {} bytes",
                    h[0],
                    h[0] + m[0],
                    h[1],
                    h[1] + m[1],
                    if report.cache_rejected {
                        ", loaded cache rejected (malformed or another version): full bake"
                    } else {
                        ""
                    },
                    cached.len()
                );
            }
            eprintln!(
                "{} bytes in {:.1} s",
                bytes.len(),
                t0.elapsed().as_secs_f32()
            );
        }
        Err(e) => {
            eprintln!("bake failed: {e}");
            std::process::exit(1);
        }
    }
}
