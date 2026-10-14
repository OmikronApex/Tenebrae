use super::*;

fn cube(mesh: &mut Mesh, min: Vec3, max: Vec3) {
    let base = mesh.vertices.len() as u32;
    for i in 0..8u32 {
        mesh.vertices.push([
            if i & 4 != 0 { max[0] } else { min[0] },
            if i & 2 != 0 { max[1] } else { min[1] },
            if i & 1 != 0 { max[2] } else { min[2] },
        ]);
    }
    for q in [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ] {
        mesh.triangles.push([base + q[0], base + q[1], base + q[2]]);
        mesh.triangles.push([base + q[0], base + q[2], base + q[3]]);
        mesh.owners.extend([0, 0]);
    }
}

fn tri(mesh: &mut Mesh, a: Vec3, b: Vec3, c: Vec3) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.extend([a, b, c]);
    mesh.triangles.push([base, base + 1, base + 2]);
    mesh.owners.push(0);
}

fn aabb(min: Vec3, max: Vec3) -> Aabb {
    Aabb { min, max }
}

fn target(min: Vec3, max: Vec3, user: u32) -> Target<'static> {
    Target {
        bounds: aabb(min, max),
        users: Box::leak(Box::new([user])),
    }
}

/// [`bake`] without a cache or hook: the tome and report.
fn bake(input: &Input<'_>) -> Result<(Tome, Report), Error> {
    let b = super::bake(input, None, None)?;
    Ok((b.tome, b.report))
}

/// [`bake`] without a cache or hook: the tome, report and object map.
fn bake_map(input: &Input<'_>) -> (Tome, Report, Vec<Option<u32>>) {
    let b = super::bake(input, None, None).expect("bake");
    (b.tome, b.report, b.object_map)
}

/// The cached bake as the M3c tests drive it.
fn bake_cached(input: &Input<'_>, cache: &TileCache) -> Result<(Tome, Report, TileCache), Error> {
    let (tome, _, report, next) = bake_with(input, Some(cache), &mut Progressor::none())?;
    Ok((tome, report, next.unwrap_or_default()))
}

/// A floor, a wall with a doorway, a sealed hollow box, a closed pillar, a slanted sheet and a
/// target beyond the domain: open, closed, sealed and thin geometry across several tiles.
fn scene() -> (Mesh, Vec<Target<'static>>) {
    let mut m = Mesh::default();
    cube(&mut m, [-20.0, -20.0, -1.0], [20.0, 20.0, 0.0]);
    cube(&mut m, [-1.0, -20.0, 0.0], [1.0, -2.0, 9.0]);
    cube(&mut m, [-1.0, 2.0, 0.0], [1.0, 20.0, 9.0]);
    cube(&mut m, [-1.0, -2.0, 4.0], [1.0, 2.0, 9.0]);
    // Sealed room: six slabs.
    let (lo, hi) = ([5.0, 5.0, 0.0], [13.0, 13.0, 6.0]);
    cube(&mut m, [lo[0], lo[1], lo[2]], [hi[0], hi[1], lo[2] + 0.5]);
    cube(&mut m, [lo[0], lo[1], hi[2] - 0.5], [hi[0], hi[1], hi[2]]);
    cube(&mut m, [lo[0], lo[1], lo[2]], [lo[0] + 0.5, hi[1], hi[2]]);
    cube(&mut m, [hi[0] - 0.5, lo[1], lo[2]], [hi[0], hi[1], hi[2]]);
    cube(&mut m, [lo[0], lo[1], lo[2]], [hi[0], lo[1] + 0.5, hi[2]]);
    cube(&mut m, [lo[0], hi[1] - 0.5, lo[2]], [hi[0], hi[1], hi[2]]);
    cube(&mut m, [-12.0, 6.0, 0.0], [-9.5, 8.7, 7.3]);
    tri(
        &mut m,
        [-15.0, -15.0, 0.3],
        [-5.0, -12.0, 5.7],
        [-9.0, -4.0, 2.1],
    );
    tri(
        &mut m,
        [3.1, -14.2, 0.0],
        [3.1, -4.4, 0.0],
        [3.1, -9.0, 6.6],
    );
    let targets = vec![
        target([-15.0, -2.0, 0.0], [-13.0, 2.0, 2.0], 7),
        target([14.0, -2.0, 0.0], [16.0, 2.0, 2.0], 9),
        target([8.0, 8.0, 1.0], [9.0, 9.0, 2.0], 11),
        target([-11.0, 7.0, 1.0], [-10.5, 7.5, 2.0], 13),
        target([30.0, -1.0, 1.0], [40.0, 1.0, 3.0], 15),
        target([-3.0, -3.0, 10.0], [3.0, 3.0, 25.0], 17),
    ];
    (m, targets)
}

fn params(tile: f32, voxel: f32, cells: CellMode, solid: SolidMode) -> Params {
    Params {
        tile,
        voxel,
        cells,
        solid,
        threads: 1,
        ..Params::levers_off()
    }
}

/// Bakes with the tile-streamed baker and the dense reference and asserts equal bytes. The view
/// volume is the whole tile-aligned domain and the reachability threshold 0, so unreachable-cell
/// removal keeps every cell, and the collapse size is one tile, so no tiles collapse, as in the
/// reference.
fn same_as_reference(p: Params, seeds: &[Vec3]) {
    let (mesh, targets) = scene();
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &targets,
        view_volume: aabb([-16.0, -16.0, -16.0], [16.0, 16.0, 16.0]),
        seeds,
        params: Params {
            reachability_threshold: 0.0,
            collapse_size: p.tile,
            ..p.clone()
        },
    };
    let (tome, report) = bake(&input).expect("bake");
    let (rtome, rreport) = reference::bake(&input).expect("reference bake");
    let what = format!("{p:?} seeds {}", seeds.len());
    assert_eq!(report.tiles, rreport.tiles, "{what}");
    assert_eq!(report.voxels, rreport.voxels, "{what}");
    assert_eq!(report.cells, rreport.cells, "{what}");
    assert_eq!(report.portals, rreport.portals, "{what}");
    assert_eq!(report.buried_targets, rreport.buried_targets, "{what}");
    assert_eq!(report.cell_samples, rreport.cell_samples, "{what}");
    assert!(
        tenebrae_tome::write(&tome) == tenebrae_tome::write(&rtome),
        "tome bytes differ: {what}"
    );
    let vox = voxelize(&input).expect("voxelize");
    let rvox = reference::voxelize(&input).expect("reference voxelize");
    assert_eq!(vox.dims, rvox.dims);
    for z in 0..vox.dims[2] {
        for y in 0..vox.dims[1] {
            for x in 0..vox.dims[0] {
                let v = [x, y, z];
                let i = rvox.index(v);
                assert_eq!(vox.surface(v), rvox.surface[i], "surface {v:?} {what}");
                assert_eq!(vox.solid(v), rvox.solid[i], "solid {v:?} {what}");
            }
        }
    }
}

const SEEDS: [Vec3; 3] = [[-10.0, 0.0, 3.0], [10.0, -10.0, 3.0], [9.0, 9.0, 3.0]];

#[test]
fn sparse_matches_dense_merged_components() {
    let m = CellMode::Merged {
        fill: 0.95,
        block: 6.0,
    };
    same_as_reference(params(8.0, 0.5, m, SolidMode::Flood), &SEEDS);
    same_as_reference(params(16.0, 1.0, m, SolidMode::Flood), &SEEDS);
}

#[test]
fn sparse_matches_dense_without_seeds() {
    let m = CellMode::Merged {
        fill: 0.9,
        block: 4.0,
    };
    same_as_reference(params(8.0, 0.5, m, SolidMode::Flood), &[]);
}

#[test]
fn sparse_matches_dense_boxes_and_components() {
    same_as_reference(params(8.0, 0.5, CellMode::Boxes, SolidMode::Flood), &SEEDS);
    let m = CellMode::Merged {
        fill: 0.9,
        block: 0.0,
    };
    same_as_reference(params(8.0, 0.5, m, SolidMode::Flood), &SEEDS);
    same_as_reference(
        params(
            8.0,
            0.5,
            CellMode::Components { block: 2.0 },
            SolidMode::Flood,
        ),
        &SEEDS,
    );
    same_as_reference(
        params(
            8.0,
            0.5,
            CellMode::Components { block: 0.0 },
            SolidMode::Flood,
        ),
        &SEEDS,
    );
}

#[test]
fn sparse_matches_dense_open_flood_and_parity() {
    let m = CellMode::Merged {
        fill: 0.95,
        block: 6.0,
    };
    let mut p = params(8.0, 0.5, m, SolidMode::Flood);
    p.surface_occludes = false;
    same_as_reference(p, &SEEDS);
    same_as_reference(params(8.0, 0.5, m, SolidMode::Parity), &SEEDS);
}

#[test]
fn sparse_matches_dense_with_a_tight_cell_cap() {
    let m = CellMode::Merged {
        fill: 0.99,
        block: 2.0,
    };
    let mut p = params(8.0, 0.25, m, SolidMode::Flood);
    p.max_cells_per_tile = 3;
    same_as_reference(p, &SEEDS);
}

#[test]
fn a_walled_scene_bakes_into_a_stable_tome() {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [-40.0, -40.0, -1.0], [40.0, 40.0, 0.0]);
    cube(&mut mesh, [-2.0, -20.0, 0.0], [2.0, 20.0, 12.0]);
    let targets = [
        target([-20.0, -2.0, 0.0], [-18.0, 2.0, 2.0], 7),
        target([18.0, -2.0, 0.0], [20.0, 2.0, 2.0], 9),
    ];
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &targets,
        view_volume: aabb([-32.0, -32.0, 0.0], [32.0, 32.0, 16.0]),
        seeds: &[[-10.0, 0.0, 4.0], [10.0, 0.0, 4.0]],
        params: Params {
            tile: 16.0,
            voxel: 1.0,
            ..Params::default()
        },
    };
    let (tome, report, map) = bake_map(&input);
    assert!(
        report.voxels[2] > 0,
        "the wall must yield solid voxels: {report:?}"
    );
    assert_eq!(report.buried_targets, 0);
    assert_eq!(tome.users.ids, vec![7, 9]);
    assert_eq!(map, vec![Some(0), Some(1)]);
    assert!(report.grid_bytes > 0);
    let bytes = tenebrae_tome::write(&tome);
    let again = tenebrae_tome::read(&bytes).expect("read back");
    assert_eq!(tenebrae_tome::write(&again), bytes);
}

#[test]
fn open_room_floods() {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [-60.0, -60.0, -1.0], [60.0, 60.0, 0.0]);
    cube(&mut mesh, [-1.0, -60.0, 0.0], [1.0, 60.0, 12.0]);
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &[],
        view_volume: aabb([-60.0, -60.0, 0.0], [60.0, 60.0, 25.0]),
        seeds: &[[-45.0, -45.0, 2.0]],
        params: Params {
            tile: 32.0,
            voxel: 1.0,
            ..Params::default()
        },
    };
    let (_, report) = bake(&input).expect("bake");
    assert!(report.voxels[2] < report.voxels[0] / 2, "{report:?}");
}

fn small_input<'a>(mesh: &'a Mesh, targets: &'a [Target]) -> Input<'a> {
    Input {
        occluders: Occluders::Soup(mesh),
        targets,
        view_volume: aabb([0.0, 0.0, 0.0], [8.0, 8.0, 8.0]),
        seeds: &[],
        params: Params {
            tile: 8.0,
            voxel: 1.0,
            threads: 1,
            ..Params::default()
        },
    }
}

fn bad(input: &Input<'_>) -> Error {
    match bake(input) {
        Err(e) => {
            assert!(!e.to_string().is_empty());
            assert!(matches!(voxelize(input), Err(ref v) if *v == e));
            e
        }
        Ok(_) => panic!("bake accepted invalid input"),
    }
}

#[test]
fn invalid_input_is_an_error() {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]);
    // Triangle index out of range.
    let mut m = mesh.clone();
    m.triangles.push([0, 1, 99]);
    m.owners.push(0);
    assert!(matches!(bad(&small_input(&m, &[])), Error::Params(_)));
    // Owners and triangles disagree.
    let mut m = mesh.clone();
    m.owners.pop();
    assert!(matches!(bad(&small_input(&m, &[])), Error::Params(_)));
    // Non-finite vertex.
    for x in [f32::NAN, f32::INFINITY] {
        let mut m = mesh.clone();
        m.vertices[3][1] = x;
        assert!(matches!(bad(&small_input(&m, &[])), Error::Params(_)));
    }
    // Target boxes: non-finite, inverted.
    for t in [
        target([0.0, f32::NAN, 0.0], [1.0, 1.0, 1.0], 0),
        target([0.0, 0.0, 0.0], [1.0, f32::INFINITY, 1.0], 0),
        target([2.0, 0.0, 0.0], [1.0, 1.0, 1.0], 0),
    ] {
        assert!(matches!(bad(&small_input(&mesh, &[t])), Error::Params(_)));
    }
    // View volume: non-finite, inverted, empty.
    for vv in [
        aabb([0.0, 0.0, f32::NEG_INFINITY], [8.0, 8.0, 8.0]),
        aabb([0.0, f32::NAN, 0.0], [8.0, 8.0, 8.0]),
        aabb([9.0, 0.0, 0.0], [8.0, 8.0, 8.0]),
        aabb([0.0, 0.0, 0.0], [8.0, 0.0, 8.0]),
    ] {
        let mut input = small_input(&mesh, &[]);
        input.view_volume = vv;
        assert!(matches!(bad(&input), Error::Params(_)));
    }
    // Settings.
    let mut input = small_input(&mesh, &[]);
    input.params.threads = 0;
    assert!(matches!(bad(&input), Error::Params(_)));
    let mut input = small_input(&mesh, &[]);
    input.params.voxel = 3.0;
    assert!(matches!(bad(&input), Error::Params(_)));
    // Flood seeds given, none inside the domain.
    let mut input = small_input(&mesh, &[]);
    let far = [[100.0, 0.0, 0.0]];
    input.seeds = &far;
    assert!(matches!(bad(&input), Error::Params(_)));
}

#[test]
fn oversized_grids_are_refused() {
    let mesh = Mesh::default();
    // Voxel count overflows.
    let mut input = small_input(&mesh, &[]);
    input.view_volume = aabb([-3e38, -3e38, -3e38], [3e38, 3e38, 3e38]);
    input.params.tile = 1.0;
    input.params.voxel = 0.5;
    assert!(matches!(
        bad(&input),
        Error::TooLarge {
            needed: u64::MAX,
            ..
        }
    ));
    // Over the limit: a big domain at a fine voxel, or a tiny limit.
    let mut input = small_input(&mesh, &[]);
    input.view_volume = aabb([0.0, 0.0, 0.0], [8192.0, 8192.0, 8192.0]);
    input.params.tile = 32.0;
    input.params.voxel = 0.125;
    let e = bad(&input);
    assert!(
        matches!(e, Error::TooLarge { needed, limit } if needed > limit),
        "{e:?}"
    );
    let mut input = small_input(&mesh, &[]);
    input.params.memory_limit = 1024;
    let e = bad(&input);
    assert!(
        matches!(e, Error::TooLarge { needed, limit: 1024 } if needed > 1024),
        "{e:?}"
    );
}

#[test]
fn tome_version_error_is_described() {
    let e = tenebrae_tome::Error::Version(17);
    assert!(e.to_string().contains("17"));
}

#[test]
fn thread_count_does_not_change_the_bake() {
    let (mesh, targets) = scene();
    for solid in [SolidMode::Flood, SolidMode::Parity] {
        let mut outs = Vec::new();
        for threads in [1, 4, 16] {
            let input = Input {
                occluders: Occluders::Soup(&mesh),
                targets: &targets,
                view_volume: aabb([-18.0, -18.0, -0.5], [18.0, 18.0, 11.0]),
                seeds: &SEEDS,
                params: Params {
                    threads,
                    ..params(
                        8.0,
                        0.5,
                        CellMode::Merged {
                            fill: 0.95,
                            block: 6.0,
                        },
                        solid,
                    )
                },
            };
            let (tome, report, map) = bake_map(&input);
            outs.push((
                tenebrae_tome::write(&tome),
                report.cell_samples,
                map,
                report.voxels,
            ));
        }
        assert!(outs[0] == outs[1], "1 vs 4 threads ({solid:?})");
        assert!(outs[0] == outs[2], "1 vs 16 threads ({solid:?})");
    }
}

/// A sealed room filling one tile, seeded inside, next to a large open area: the room is a small
/// component, so reachability removes it, its tile keeps no cells and its target is dropped.
/// A ground slab, a sealed 8 m room (a seed inside it) and a thick closed block; targets in the
/// open, in the room, inside the block (buried) and in the open.
fn sealed_pocket_scene() -> (Mesh, [Target<'static>; 4]) {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [-40.0, -40.0, -1.0], [40.0, 40.0, 0.0]);
    let (lo, hi) = ([8.0, 8.0, 0.0], [16.0, 16.0, 8.0]);
    cube(
        &mut mesh,
        [lo[0], lo[1], lo[2]],
        [hi[0], hi[1], lo[2] + 0.5],
    );
    cube(
        &mut mesh,
        [lo[0], lo[1], hi[2] - 0.5],
        [hi[0], hi[1], hi[2]],
    );
    cube(
        &mut mesh,
        [lo[0], lo[1], lo[2]],
        [lo[0] + 0.5, hi[1], hi[2]],
    );
    cube(
        &mut mesh,
        [hi[0] - 0.5, lo[1], lo[2]],
        [hi[0], hi[1], hi[2]],
    );
    cube(
        &mut mesh,
        [lo[0], lo[1], lo[2]],
        [hi[0], lo[1] + 0.5, hi[2]],
    );
    cube(
        &mut mesh,
        [lo[0], hi[1] - 0.5, lo[2]],
        [hi[0], hi[1], hi[2]],
    );
    // A thick closed block: a target inside it touches no cell (buried) and is kept.
    cube(&mut mesh, [-20.0, -20.0, 0.0], [-12.0, -12.0, 6.0]);
    let targets = [
        target([-4.0, -4.0, 1.0], [-3.0, -3.0, 2.0], 1),
        target([11.0, 11.0, 2.0], [12.0, 12.0, 3.0], 2),
        target([-17.0, -17.0, 2.0], [-16.0, -16.0, 3.0], 3),
        target([0.0, 4.0, 1.0], [1.0, 5.0, 2.0], 4),
    ];
    (mesh, targets)
}

/// [`sealed_pocket_scene`] at 8 m tiles, 0.5 m voxels, no collapse, every cell kept.
fn sealed_pocket_input<'a>(mesh: &'a Mesh, targets: &'a [Target<'a>]) -> Input<'a> {
    let mut input = Input {
        occluders: Occluders::Soup(mesh),
        targets,
        view_volume: aabb([-32.0, -32.0, 0.0], [32.0, 32.0, 16.0]),
        seeds: &POCKET_SEEDS,
        params: params(
            8.0,
            0.5,
            CellMode::Merged {
                fill: 0.95,
                block: 4.0,
            },
            SolidMode::Flood,
        ),
    };
    input.params.reachability_threshold = 0.0;
    input.params.collapse_size = 8.0;
    input
}

const POCKET_SEEDS: [Vec3; 2] = [[0.0, 0.0, 4.0], [12.0, 12.0, 4.0]];

#[test]
fn reachability_drops_a_sealed_pocket_and_its_target() {
    let (mesh, targets) = sealed_pocket_scene();
    let mut input = sealed_pocket_input(&mesh, &targets);
    let (_, all_report, all_map) = bake_map(&input);
    assert_eq!(all_map, vec![Some(0), Some(1), Some(2), Some(3)]);
    assert_eq!(all_report.buried_targets, 1);
    assert_eq!(all_report.dropped_targets, 0);
    input.params.reachability_threshold = 0.1;
    let (tome, report, map) = bake_map(&input);
    assert_eq!(map, vec![Some(0), None, Some(1), Some(2)]);
    assert_eq!((report.dropped_targets, report.sealed_targets), (1, 0));
    assert_eq!(tome.users.ids, vec![1, 3, 4]);
    assert_eq!(tome.objects.len(), 3);
    assert!(
        report.cells < all_report.cells,
        "{} {}",
        report.cells,
        all_report.cells
    );
    assert_eq!(report.cell_samples.len(), report.cells);
    // The room's tile: a leaf with no cells and a one-node view tree of value 1.
    let room = tome
        .tiles
        .iter()
        .flatten()
        .find(|t| t.bounds.min == [8.0, 8.0, 0.0])
        .expect("room tile");
    assert!(room.cells.is_empty());
    let view = room.view.as_ref().expect("view tree");
    assert_eq!(view.tree.codes, vec![3]);
    assert_eq!(view.leaf_values, vec![1]);
    assert!(room.portals.is_empty());
    // No portal of any tile leads into a removed cell: every target cell index is in range.
    for t in tome.tiles.iter().flatten() {
        for p in &t.portals {
            let boundary = (p.0[0] >> 28) & 1 == 1;
            if !boundary {
                let node = (p.0[0] & 0x3FF_FFFF) as usize;
                let local = (p.0[1] >> 16) as usize;
                let target = tome.tiles[node].as_ref().expect("portal target tile");
                assert!(local < target.cells.len());
            }
        }
    }
    let bytes = tenebrae_tome::write(&tome);
    assert_eq!(
        tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read")),
        bytes
    );
}

/// Two open 16 m tiles under one top node collapse into one 32 m tile whose seam merge joins their
/// cells; with a smaller collapse size they stay apart.
#[test]
fn two_16m_tiles_collapse() {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [-8.0, -8.0, -1.0], [40.0, 24.0, 0.0]);
    let targets = [
        target([4.0, 4.0, 1.0], [5.0, 5.0, 2.0], 1),
        target([20.0, 4.0, 1.0], [21.0, 5.0, 2.0], 2),
    ];
    let seeds = [[8.0, 8.0, 8.0]];
    let mut input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &targets,
        view_volume: aabb([0.0, 0.0, 0.0], [32.0, 16.0, 16.0]),
        seeds: &seeds,
        params: params(
            16.0,
            1.0,
            CellMode::Merged {
                fill: 0.95,
                block: 6.0,
            },
            SolidMode::Flood,
        ),
    };
    input.params.collapse_size = 16.0;
    let (apart, apart_report) = bake(&input).expect("bake");
    assert_eq!(apart_report.tiles, 2);
    assert_eq!(apart.top.codes.len(), 3);
    input.params.collapse_size = 32.0;
    let (tome, report) = bake(&input).expect("bake");
    assert_eq!(report.tiles, 1);
    assert_eq!(tome.top.codes, vec![3]);
    assert!(
        report.cells < apart_report.cells,
        "{} vs {}",
        report.cells,
        apart_report.cells
    );
    assert_eq!(report.cell_samples.len(), report.cells);
    let tile = tome.tiles[0].as_ref().expect("merged tile");
    assert_eq!(tile.bounds.min, [0.0, 0.0, 0.0]);
    assert_eq!(tile.bounds.max, [32.0, 16.0, 16.0]);
    assert_eq!(tile.cells.len(), report.cells);
    // Every portal of the merged tile leads to a cell of its own tile or to the boundary.
    for p in &tile.portals {
        if (p.0[0] >> 28) & 1 == 0 {
            assert_eq!(p.0[0] & 0x3FF_FFFF, 0);
            assert!(((p.0[1] >> 16) as usize) < tile.cells.len());
        }
    }
    assert_eq!(tome.users.ids, vec![1, 2]);
    let bytes = tenebrae_tome::write(&tome);
    assert_eq!(
        tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read")),
        bytes
    );
}

#[test]
fn default_voxel_is_umbras_smallest_hole() {
    assert_eq!(Params::default().voxel, 0.125);
    assert_eq!(Params::default().tile, 32.0);
    assert_eq!(Params::default().cells, CellMode::Components { block: 8.0 });
}

/// Lever L1: a domain of 8×8×4 tiles with one small block of geometry. Tiles the block's grown box
/// reaches stay single tiles; the empty rest becomes a few large leaves.
#[test]
fn l1_makes_large_leaves_over_empty_space() {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [20.0, 20.0, 4.0], [27.0, 27.0, 11.0]);
    let targets = [target([40.0, 40.0, 4.0], [41.0, 41.0, 5.0], 1)];
    let seeds = [[2.0, 2.0, 2.0]];
    let mut input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &targets,
        view_volume: aabb([0.0, 0.0, 0.0], [64.0, 64.0, 32.0]),
        seeds: &seeds,
        params: params(
            8.0,
            1.0,
            CellMode::Merged {
                fill: 0.95,
                block: 4.0,
            },
            SolidMode::Flood,
        ),
    };
    input.params.collapse_size = 8.0;
    let (unit, unit_report) = bake(&input).expect("bake");
    assert_eq!(unit_report.tiles, 256);
    input.params.empty_leaves = true;
    let (tome, report, map) = bake_map(&input);
    assert!(report.tiles < 40, "{} tiles", report.tiles);
    let leaves: Vec<&tenebrae_tome::Tile> = tome
        .top
        .codes
        .iter()
        .zip(&tome.tiles)
        .filter(|(c, _)| **c == 3)
        .map(|(_, t)| t.as_ref().expect("every leaf has a tile"))
        .collect();
    assert_eq!(leaves.len(), report.tiles);
    let grown = aabb([19.0, 19.0, 3.0], [28.0, 28.0, 12.0]);
    let mut large = 0;
    for t in &leaves {
        let ext = (0..3)
            .map(|a| t.bounds.max[a] - t.bounds.min[a])
            .fold(0.0, f32::max);
        let touches =
            (0..3).all(|a| t.bounds.min[a] < grown.max[a] && t.bounds.max[a] > grown.min[a]);
        if touches {
            assert_eq!(ext, 8.0, "a tile with triangles stays a single tile");
        }
        if ext > 8.0 {
            large += 1;
            assert!(t.cells.len() <= 1);
        }
    }
    assert!(large > 0);
    assert!(tenebrae_tome::write(&tome).len() < tenebrae_tome::write(&unit).len());
    assert_eq!(map, vec![Some(0)]);
    let bytes = tenebrae_tome::write(&tome);
    assert_eq!(
        tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read")),
        bytes
    );
}

/// Lever L2: the per-tile merge removes cells the best-first merge kept, the same way on every
/// thread count.
#[test]
fn l2_merges_cells_identically_on_any_thread_count() {
    let (mesh, targets) = scene();
    let mut outs = Vec::new();
    for (merge, threads) in [(0.0, 1), (6.0, 1), (6.0, 4)] {
        let mut p = params(
            8.0,
            0.5,
            CellMode::Merged {
                fill: 0.99,
                block: 2.0,
            },
            SolidMode::Flood,
        );
        p.cell_merge = merge;
        p.threads = threads;
        let input = Input {
            occluders: Occluders::Soup(&mesh),
            targets: &targets,
            view_volume: aabb([-18.0, -18.0, -0.5], [18.0, 18.0, 11.0]),
            seeds: &SEEDS,
            params: p,
        };
        let (tome, report) = bake(&input).expect("bake");
        outs.push((
            report.cells,
            tenebrae_tome::write(&tome),
            report.cell_samples,
        ));
    }
    assert!(outs[1].0 < outs[0].0, "{} vs {}", outs[1].0, outs[0].0);
    assert!(outs[1] == outs[2], "threads 1 vs 4");
    let tome = tenebrae_tome::read(&outs[1].1).expect("read");
    assert_eq!(tenebrae_tome::write(&tome), outs[1].1);
}

/// Lever L3: global cells are connected groups of one tile's cells; global portals join
/// distinct groups.
#[test]
fn l3_groups_connected_tile_cells() {
    let (mesh, targets) = scene();
    let mut p = Params {
        threads: 2,
        ..params(
            8.0,
            0.5,
            CellMode::Merged {
                fill: 0.95,
                block: 2.0,
            },
            SolidMode::Flood,
        )
    };
    p.global_groups = true;
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &targets,
        view_volume: aabb([-18.0, -18.0, -0.5], [18.0, 18.0, 11.0]),
        seeds: &SEEDS,
        params: p,
    };
    let (tome, report) = bake(&input).expect("bake");
    let groups = tome.global_cells.len();
    assert!(
        groups < report.cells,
        "{groups} groups for {} cells",
        report.cells
    );
    assert_eq!(report.cell_samples.len(), groups);
    // Members of each group: (node, local).
    let mut members: Vec<Vec<(usize, usize)>> = vec![Vec::new(); groups];
    for (node, t) in tome.tiles.iter().enumerate() {
        let Some(t) = t else { continue };
        for (i, c) in t.cells.iter().enumerate() {
            assert_eq!(c.0[5], 0);
            members[c.0[4] as usize].push((node, i));
        }
    }
    for (gc, m) in members.iter().enumerate() {
        assert!(!m.is_empty(), "group {gc} has cells");
        let node = m[0].0;
        assert!(m.iter().all(|x| x.0 == node), "group {gc} spans tiles");
        // Connected over in-tile portals.
        let t = tome.tiles[node].as_ref().expect("tile");
        let mut reached = vec![m[0].1];
        let mut k = 0;
        while k < reached.len() {
            let c = &t.cells[reached[k]];
            for p in &t.portals[c.0[0] as usize..(c.0[0] + c.0[1]) as usize] {
                let boundary = (p.0[0] >> 28) & 1 == 1;
                let local = (p.0[1] >> 16) as usize;
                if !boundary
                    && (p.0[0] & 0x3FF_FFFF) as usize == node
                    && m.iter().any(|x| x.1 == local)
                    && !reached.contains(&local)
                {
                    reached.push(local);
                }
            }
            k += 1;
        }
        assert_eq!(reached.len(), m.len(), "group {gc} is connected");
        let g = &tome.global_cells[gc];
        let gportals = &tome.global_portals
            [g.portal_start as usize..(g.portal_start + g.portal_count) as usize];
        for p in gportals {
            assert_ne!((p.0[0] & 0x3FF_FFFF) as usize, gc);
        }
        // Every tile portal into another group has a global portal to that group.
        let mut neighbours = false;
        for &(node, i) in m {
            let tile = tome.tiles[node].as_ref().expect("tile");
            let c = &tile.cells[i];
            for p in &tile.portals[c.0[0] as usize..(c.0[0] + c.0[1]) as usize] {
                if (p.0[0] >> 28) & 1 == 1 {
                    continue;
                }
                let target = tome.tiles[(p.0[0] & 0x3FF_FFFF) as usize]
                    .as_ref()
                    .expect("target tile");
                let gn = target.cells[(p.0[1] >> 16) as usize].0[4];
                if gn as usize != gc {
                    neighbours = true;
                    assert!(
                        gportals.iter().any(|q| q.0[0] & 0x3FF_FFFF == gn),
                        "group {gc} lacks a global portal to {gn}"
                    );
                }
            }
        }
        if neighbours {
            assert!(g.portal_count > 0);
        }
    }
    let bytes = tenebrae_tome::write(&tome);
    assert_eq!(
        tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read")),
        bytes
    );
}

/// All three levers on, threads 1 vs 4.
#[test]
fn levers_on_are_thread_independent() {
    let (mesh, targets) = scene();
    let mut outs = Vec::new();
    for threads in [1, 4] {
        let input = Input {
            occluders: Occluders::Soup(&mesh),
            targets: &targets,
            view_volume: aabb([-30.0, -30.0, -0.5], [30.0, 30.0, 30.0]),
            seeds: &SEEDS,
            params: Params {
                tile: 8.0,
                voxel: 0.5,
                threads,
                ..Params::default()
            },
        };
        let (tome, report, map) = bake_map(&input);
        assert_valid(&tome);
        outs.push((tenebrae_tome::write(&tome), report.cell_samples, map));
    }
    assert!(outs[0] == outs[1]);
}

/// The tome round-trips byte for byte, every non-boundary tile portal names an existing tile and
/// a local cell inside it, and every view-tree leaf value is a cell of its tile or the solid
/// value. Inner nodes may carry coarse LOD tiles, which have no view tree.
fn assert_valid(tome: &crate::Tome) {
    let bytes = tenebrae_tome::write(tome);
    assert_eq!(
        tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read back")),
        bytes
    );
    for (node, t) in tome.tiles.iter().enumerate() {
        let Some(t) = t else { continue };
        if tome.top.codes[node] != 3 {
            assert!(t.view.is_none(), "LOD tile without a view tree");
            assert!(tome.node_distances[node] > 0.0, "LOD tile with a distance");
        }
        for p in &t.portals {
            if (p.0[0] >> 28) & 1 == 1 {
                continue;
            }
            let target = tome.tiles[(p.0[0] & 0x3FF_FFFF) as usize]
                .as_ref()
                .expect("portal target has a tile");
            assert!(((p.0[1] >> 16) as usize) < target.cells.len());
        }
        if tome.top.codes[node] != 3 {
            continue;
        }
        let view = t.view.as_ref().expect("leaf tile has a view tree");
        let solid = (1u32 << view.leaf_bits) - 1;
        assert!(t.cells.len() <= solid as usize);
        for &v in &view.leaf_values {
            assert!((v as usize) < t.cells.len() || v == solid, "leaf value {v}");
        }
    }
    for (node, c) in tome.top.codes.iter().enumerate() {
        if *c == 3 {
            assert!(tome.tiles[node].is_some(), "every leaf has a tile");
        }
    }
}

/// The estimate bounds what the bake measures: each tile's peak brick bytes stay within its bound,
/// and the reported grid bytes within the estimate.
#[test]
fn estimates_bound_the_measured_bytes() {
    let (mesh, targets) = scene();
    for solid in [SolidMode::Flood, SolidMode::Parity] {
        let input = Input {
            occluders: Occluders::Soup(&mesh),
            targets: &targets,
            view_volume: aabb([-18.0, -18.0, -0.5], [18.0, 18.0, 11.0]),
            seeds: &SEEDS,
            params: Params {
                threads: 2,
                ..params(
                    8.0,
                    0.5,
                    CellMode::Merged {
                        fill: 0.95,
                        block: 6.0,
                    },
                    solid,
                )
            },
        };
        let prep = prepare(&input, &mut Progressor::none()).expect("prepare");
        let (floods, live) = flood(&prep);
        let ctx = &prep.ctx;
        for t in 0..ctx.tile_ranges.len() {
            let (surface, solid_bits, peak) = tile::solid_pass(ctx, t, &live, &floods);
            let overlap = tile::overlaps(ctx, t);
            let out = tile::cell_pass(ctx, t, surface.count(), solid_bits, peak, &overlap);
            let mut measured = out.bytes as u64;
            if let Some(f) = floods.get(t) {
                measured = measured.max(f.bytes as u64);
            }
            assert!(
                measured <= prep.tile_bounds[t],
                "tile {t}: {measured} > {} ({solid:?})",
                prep.tile_bounds[t]
            );
        }
        let (_, report) = bake(&input).expect("bake");
        assert!(report.grid_bytes > 0);
        assert!(
            report.grid_bytes <= prep.estimate,
            "{} > {} ({solid:?})",
            report.grid_bytes,
            prep.estimate
        );
    }
}

/// Lever L1 never merges solid empty tiles with open ones: two solid tiles next to two open
/// tiles, none with triangles, give one solid and one open leaf.
#[test]
fn l1_keeps_solid_and_open_empty_tiles_apart() {
    let mesh = Mesh::default();
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &[],
        view_volume: aabb([0.0, 0.0, 0.0], [32.0, 8.0, 8.0]),
        seeds: &[],
        params: params(
            8.0,
            1.0,
            CellMode::Merged {
                fill: 0.95,
                block: 4.0,
            },
            SolidMode::Flood,
        ),
    };
    let prep = prepare(&input, &mut Progressor::none()).expect("prepare");
    let ctx = &prep.ctx;
    let size = ctx.layout.tile();
    let n = ctx.tile_ranges.len();
    assert_eq!(n, 4);
    let mut tiles = Vec::new();
    for t in 0..n {
        let x = ctx.tile_ranges[t][0][0];
        let lo = ctx.lo(t);
        let open = x >= 2;
        let cells = if open {
            vec![global::GCell {
                uid: t as u32,
                lo,
                hi: lo.map(|k| k + size),
                voxels: (size * size * size) as u64,
                samples: Vec::new(),
                links: Vec::new(),
                objects: Vec::new(),
            }]
        } else {
            Vec::new()
        };
        tiles.push(Some(global::GTile {
            range: ctx.tile_ranges[t],
            tree: vec![tile::Node::Leaf(if open { 0 } else { grid::SOLID })],
            choices: Vec::new(),
            cells,
            explicit: false,
            shift: 0,
        }));
    }
    let mut scene = global::Scene {
        top: prep.top.clone(),
        tiles,
        alias: (0..n as u32).collect(),
        touched: Vec::new(),
    };
    global::empty_leaves(ctx, &mut scene, &[false; 4]);
    let leaves: Vec<&global::GTile> = scene.tiles.iter().flatten().collect();
    assert_eq!(leaves.len(), 2, "one solid and one open leaf");
    for t in leaves {
        let solid = t.range[0][1] <= 2;
        let open = t.range[0][0] >= 2;
        assert!(
            solid || open,
            "leaf {:?} mixes solid and open tiles",
            t.range
        );
        assert_eq!(t.cells.len(), usize::from(open));
    }
}

/// Lever L2 refuses a union whose box pulls in a wall: two pockets that only meet through a
/// one-voxel hole in a 1 m wall stay separate cells at `cell_merge` 6.
#[test]
fn l2_keeps_pockets_behind_a_wall_apart() {
    let mut mesh = Mesh::default();
    // Wall x 3.5..4.5 over the whole tile, with a 0.5 m hole at y, z 3.5..4.
    cube(&mut mesh, [3.5, 0.0, 0.0], [4.5, 3.5, 8.0]);
    cube(&mut mesh, [3.5, 4.0, 0.0], [4.5, 8.0, 8.0]);
    cube(&mut mesh, [3.5, 3.5, 0.0], [4.5, 4.0, 3.5]);
    cube(&mut mesh, [3.5, 3.5, 4.0], [4.5, 4.0, 8.0]);
    let seeds = [[1.0, 1.0, 1.0], [7.0, 7.0, 7.0]];
    let bake_with = |merge: f32| {
        let mut p = params(
            8.0,
            0.5,
            CellMode::Merged {
                fill: 0.95,
                block: 4.0,
            },
            SolidMode::Flood,
        );
        p.cell_merge = merge;
        let input = Input {
            occluders: Occluders::Soup(&mesh),
            targets: &[],
            view_volume: aabb([0.0, 0.0, 0.0], [8.0, 8.0, 8.0]),
            seeds: &seeds,
            params: p,
        };
        bake(&input).expect("bake")
    };
    let (_, off) = bake_with(0.0);
    let (tome, on) = bake_with(6.0);
    assert!(off.cells > 2, "{} cells without L2", off.cells);
    assert_eq!(on.cells, 2, "the two pockets stay apart");
    let t = tome.tiles.iter().flatten().next().expect("tile");
    // One cell left of the wall, one right of it (cell boxes: min x = hi16 of word 6, max x =
    // lo16 of word 7, against the tile frame).
    let frame = t.bounds;
    let x = |q: u32| frame.min[0] + q as f32 / 65535.0 * (frame.max[0] - frame.min[0]);
    let (lo0, hi0) = (x(t.cells[0].0[6] >> 16), x(t.cells[0].0[7] & 0xFFFF));
    let (lo1, hi1) = (x(t.cells[1].0[6] >> 16), x(t.cells[1].0[7] & 0xFFFF));
    assert!(lo0 < 1.0 && hi0 <= 4.0 + 1e-3, "{lo0}..{hi0}");
    assert!(lo1 >= 4.0 - 1e-3 && hi1 > 7.0, "{lo1}..{hi1}");
}

/// Lever L2 keeps a wall inside a tall tile as a cell boundary: a 6 m wall with a doorway across a
/// 32 m tile, open above. The union of one side with the air above it has a box that wraps the
/// other side's open space; counting solid alone (0.4 m of wall per metre of opening) accepted it
/// and left one cell per tile (the benchmark's `rooms` at 32 m).
#[test]
fn l2_keeps_a_wall_in_a_tall_tile() {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [0.0, 0.0, -1.0], [32.0, 32.0, 0.0]);
    cube(&mut mesh, [15.75, 0.0, 0.0], [16.25, 14.0, 6.0]);
    cube(&mut mesh, [15.75, 17.0, 0.0], [16.25, 32.0, 6.0]);
    cube(&mut mesh, [15.75, 14.0, 3.0], [16.25, 17.0, 6.0]);
    let seeds = [[8.0, 24.0, 1.7], [24.0, 24.0, 1.7]];
    let mut p = params(
        32.0,
        0.5,
        CellMode::Merged {
            fill: 0.95,
            block: 6.0,
        },
        SolidMode::Flood,
    );
    p.cell_merge = 6.0;
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &[],
        view_volume: aabb([0.0, 0.0, 0.0], [32.0, 32.0, 32.0]),
        seeds: &seeds,
        params: p,
    };
    let (tome, _) = bake(&input).expect("bake");
    let left = tenebrae_tome::locate::find_cell(&tome, [8.0, 24.0, 1.7]).expect("left in a cell");
    let right =
        tenebrae_tome::locate::find_cell(&tome, [24.0, 24.0, 1.7]).expect("right in a cell");
    let above =
        tenebrae_tome::locate::find_cell(&tome, [16.0, 24.0, 20.0]).expect("above in a cell");
    assert_ne!(left, right, "the wall separates the two sides");
    assert_ne!(left, above);
    assert_ne!(right, above);
}

/// Objects are also listed in every cell whose box overlaps their box grown by one voxel: a target
/// whose grown box holds no open voxel (here inside a closed block, standing in for a small object
/// its surface voxels engulf) is listed in the whole-tile cell around it instead of in none.
#[test]
fn objects_are_listed_in_cells_whose_box_overlaps() {
    let mut mesh = Mesh::default();
    cube(&mut mesh, [0.0, 0.0, -1.0], [8.0, 8.0, 0.0]);
    cube(&mut mesh, [2.0, 2.0, 0.0], [6.0, 6.0, 4.0]);
    let targets = [
        target([3.9, 3.9, 1.0], [4.1, 4.1, 2.0], 1),
        target([0.5, 0.5, 1.0], [1.0, 1.0, 2.0], 2),
    ];
    let seeds = [[1.0, 1.0, 6.0]];
    let mut p = params(
        8.0,
        0.5,
        CellMode::Components { block: 0.0 },
        SolidMode::Flood,
    );
    p.keep_every_target = false;
    let input = Input {
        occluders: Occluders::Soup(&mesh),
        targets: &targets,
        view_volume: aabb([0.0, 0.0, 0.0], [8.0, 8.0, 8.0]),
        seeds: &seeds,
        params: p,
    };
    let (_, report, map) = bake_map(&input);
    assert_eq!(report.cells, 1);
    assert_eq!(report.buried_targets, 0);
    assert_eq!(map, [Some(0), Some(1)]);
}

/// Collapse's 96-cell cap: two 16 m tiles of 64 component cells each stay apart; with 8 cells
/// each they collapse.
#[test]
fn collapse_respects_the_96_cell_cap() {
    let mesh = Mesh::default();
    for (block, tiles) in [(4.0, 2), (8.0, 1)] {
        let input = Input {
            occluders: Occluders::Soup(&mesh),
            targets: &[],
            view_volume: aabb([0.0, 0.0, 0.0], [32.0, 16.0, 16.0]),
            seeds: &[[1.0, 1.0, 1.0]],
            params: Params {
                collapse_size: 32.0,
                reachability_threshold: 0.0,
                ..params(16.0, 1.0, CellMode::Components { block }, SolidMode::Flood)
            },
        };
        let (tome, report) = bake(&input).expect("bake");
        assert_eq!(report.tiles, tiles, "block {block}: {} cells", report.cells);
        assert_valid(&tome);
    }
}

/// Collapse's seam gate: a 2 m wall around the shared plane, open only through a hole, pulls
/// solid into every cross-seam union, so the seam merge leaves the cells as they were; without
/// the wall the open space merges across the seam.
#[test]
fn collapse_seam_merge_respects_fill() {
    let mut walled = Mesh::default();
    cube(&mut walled, [15.0, 0.0, 0.0], [17.0, 7.0, 16.0]);
    cube(&mut walled, [15.0, 9.0, 0.0], [17.0, 16.0, 16.0]);
    cube(&mut walled, [15.0, 7.0, 0.0], [17.0, 9.0, 7.0]);
    cube(&mut walled, [15.0, 7.0, 9.0], [17.0, 9.0, 16.0]);
    let open = Mesh::default();
    let cells = |mesh: &Mesh, collapse: f32| {
        let input = Input {
            occluders: Occluders::Soup(mesh),
            targets: &[],
            view_volume: aabb([0.0, 0.0, 0.0], [32.0, 16.0, 16.0]),
            seeds: &[[2.0, 2.0, 2.0], [30.0, 14.0, 14.0]],
            params: Params {
                collapse_size: collapse,
                reachability_threshold: 0.0,
                ..params(
                    16.0,
                    1.0,
                    CellMode::Merged {
                        fill: 0.95,
                        block: 8.0,
                    },
                    SolidMode::Flood,
                )
            },
        };
        let (tome, report) = bake(&input).expect("bake");
        assert_valid(&tome);
        (report.tiles, report.cells)
    };
    let (apart_tiles, apart) = cells(&walled, 16.0);
    let (joined_tiles, joined) = cells(&walled, 32.0);
    assert_eq!((apart_tiles, joined_tiles), (2, 1));
    assert_eq!(
        joined, apart,
        "no cell pair across the walled seam is merged"
    );
    let (_, open_apart) = cells(&open, 16.0);
    let (_, open_joined) = cells(&open, 32.0);
    assert!(open_joined < open_apart, "{open_joined} vs {open_apart}");
}

/// FNV-1a over a tome's bytes, to pin bakes made before a change.
fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The shared multi-tile scene at 8 m tiles and 0.5 m voxels (16 voxels per tile).
fn lever_input<'a>(mesh: &'a Mesh, targets: &'a [Target], p: Params) -> Input<'a> {
    Input {
        occluders: Occluders::Soup(mesh),
        targets,
        view_volume: aabb([-30.0, -30.0, -0.5], [30.0, 30.0, 30.0]),
        seeds: &SEEDS,
        params: Params {
            tile: 8.0,
            voxel: 0.5,
            ..p
        },
    }
}

/// The level-order indices of the stored splits that are not, bit for bit, the runtime's f32
/// midpoint of their node's box, the boxes descending from `root` as `query` derives them (nodes
/// past the stored prefix split at their midpoint).
fn off_midpoint_splits(tree: &tenebrae_tome::KdTree, root: Aabb) -> Vec<usize> {
    let n = tree.codes.len();
    let mut off = Vec::new();
    let mut boxes = vec![root; n];
    let mut inner = 0usize;
    for v in 0..n {
        let code = tree.codes[v];
        if code == 3 {
            continue;
        }
        inner += 1;
        let (a, b) = (usize::from(code), boxes[v]);
        let mid = (b.min[a] + b.max[a]) * 0.5;
        let s = tree.splits.get(v).copied().unwrap_or(mid);
        if s.to_bits() != mid.to_bits() {
            off.push(v);
        }
        let (below, above) = (2 * inner - 1, 2 * inner);
        if above < n {
            boxes[below] = b;
            boxes[below].max[a] = s;
            boxes[above] = b;
            boxes[above].min[a] = s;
        }
    }
    off
}

/// Lever L4's storage: a tile stores either no split (the runtime takes each node's midpoint) or,
/// as the fallback of a collapsed tile whose tree is not a midpoint tree, the shortest level-order
/// prefix of splits that ends at its last off-midpoint split. At least one tile is split-free.
/// Returns (explicit tiles, split-free tiles). The other direction (a split-free tile's tree really
/// is a midpoint tree) is what `assert_open_voxels_match` checks: read with midpoints, any other
/// tree resolves open voxels differently from the voxel-exact L4-off tome.
fn assert_l4_view_splits(tome: &crate::Tome) -> (usize, usize) {
    let (mut explicit, mut free) = (0, 0);
    // Coarse LOD tiles have no view tree.
    for t in tome.tiles.iter().flatten().filter(|t| t.view.is_some()) {
        let tree = &t.view.as_ref().expect("view tree").tree;
        if tree.splits.is_empty() {
            free += 1;
        } else {
            let off = off_midpoint_splits(tree, t.bounds);
            assert_eq!(
                off.last().map(|v| v + 1),
                Some(tree.splits.len()),
                "the stored prefix ends at the last off-midpoint split: {:?}",
                t.bounds
            );
            explicit += 1;
        }
    }
    assert!(free > 0);
    (explicit, free)
}

/// Points of every sampled open voxel, at its centre and near each of its eight corners, find the
/// voxel's own global cell through the runtime-equivalent descent (`query::find_cell`): no leaf
/// assigns an open voxel to solid or to another cell. Points outside the view volume `vv` are
/// skipped: with [`Params::view_mask`] the view trees leave space there "don't care".
fn assert_samples_find_their_cells(tome: &crate::Tome, report: &Report, voxel: f32, vv: &Aabb) {
    let d = 0.45 * voxel;
    let mut checked = 0usize;
    for (g, list) in report.cell_samples.iter().enumerate() {
        for p in list {
            for k in 0..9 {
                let q = if k == 8 {
                    *p
                } else {
                    [0, 1, 2].map(|a| p[a] + if k >> a & 1 == 1 { d } else { -d })
                };
                let b = &tome.header.bounds;
                if (0..3).any(|a| q[a] <= b.min[a] || q[a] >= b.max[a]) {
                    continue;
                }
                if (0..3).any(|a| q[a] < vv.min[a] || q[a] > vv.max[a]) {
                    continue;
                }
                assert_eq!(
                    tenebrae_tome::locate::find_cell(tome, q),
                    Some(g as u32),
                    "point {q:?} of a sample of global cell {g}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
    // Each cell's representative point finds it too.
    assert_eq!(report.cell_points.len(), report.cell_samples.len());
    for (g, pt) in report.cell_points.iter().enumerate() {
        let in_vv = |p: &Vec3| (0..3).all(|a| vv.min[a] <= p[a] && p[a] <= vv.max[a]);
        let Some(pt) = *pt else {
            assert!(
                !report.cell_samples[g].iter().any(in_vv),
                "global cell {g} has a sample in the view volume but no point"
            );
            continue;
        };
        assert_eq!(
            tenebrae_tome::locate::find_cell(tome, pt),
            Some(g as u32),
            "cell point {pt:?} of global cell {g}"
        );
    }
}

/// Exhaustive conservativeness: every open voxel of `vox` that the L4-off tome `off` resolves to
/// a global cell (its leaves lie on voxel planes, so it is exact there) resolves to the same cell
/// in the L4-on tome `on` through `query::find_cell`, at its centre and near each of its eight
/// corners (+-0.45 voxel), partly covered voxels included. Both bakes share their cells. Returns
/// the surface voxels checked (see the second half).
fn assert_open_voxels_match(on: &crate::Tome, off: &crate::Tome, vox: &Voxels) -> usize {
    let (h, d) = (0.5 * vox.voxel, 0.45 * vox.voxel);
    let b = &on.header.bounds;
    let mut checked = 0usize;
    for z in 0..vox.dims[2] {
        for y in 0..vox.dims[1] {
            for x in 0..vox.dims[0] {
                let v = [x, y, z];
                if vox.solid(v) {
                    continue;
                }
                let c = [0, 1, 2].map(|a| vox.origin[a] + v[a] as f32 * vox.voxel + h);
                let Some(g) = tenebrae_tome::locate::find_cell(off, c) else {
                    continue;
                };
                for k in 0..9 {
                    let q = if k == 8 {
                        c
                    } else {
                        [0, 1, 2].map(|a| c[a] + if k >> a & 1 == 1 { d } else { -d })
                    };
                    if (0..3).any(|a| q[a] <= b.min[a] || q[a] >= b.max[a]) {
                        continue;
                    }
                    assert_eq!(
                        tenebrae_tome::locate::find_cell(on, q),
                        Some(g),
                        "point {q:?} of open voxel {v:?}"
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
    // Surface voxels (solid, sharing a face with an open voxel of their tile), at their centre:
    // where the L4-off tome gives one a cell, the L4-on tome does too, the cell of an open voxel
    // among its 26 neighbours (`tile::view_of`'s flexible owners, resolved by `reachability`).
    let centre = |v: [usize; 3]| [0, 1, 2].map(|a| vox.origin[a] + v[a] as f32 * vox.voxel + h);
    let per: [usize; 3] = std::array::from_fn(|a| vox.dims[a] / vox.tiles[a]);
    let mut owned = 0usize;
    for z in 0..vox.dims[2] {
        for y in 0..vox.dims[1] {
            for x in 0..vox.dims[0] {
                let v = [x, y, z];
                if !vox.solid(v) {
                    continue;
                }
                let c = centre(v);
                if (0..3).any(|a| c[a] <= b.min[a] || c[a] >= b.max[a]) {
                    continue;
                }
                // Owned: an open voxel of the same tile shares a face with it.
                let owned_by_tile = (0..6).any(|f| {
                    let (a, up) = (f / 2, f % 2 == 1);
                    let mut w = v;
                    if up {
                        w[a] += 1;
                    } else if w[a] == 0 {
                        return false;
                    } else {
                        w[a] -= 1;
                    }
                    w[a] < vox.dims[a] && w[a] / per[a] == v[a] / per[a] && !vox.solid(w)
                });
                if !owned_by_tile {
                    continue;
                }
                // An owner that reachability removed leaves the voxel solid with L4 off.
                if tenebrae_tome::locate::find_cell(off, c).is_none() {
                    continue;
                }
                let Some(g) = tenebrae_tome::locate::find_cell(on, c) else {
                    panic!("surface voxel {v:?} has an owner with L4 off and none with L4 on");
                };
                let mut near = false;
                for n in 0..27 {
                    let d = [n % 3, n / 3 % 3, n / 9].map(|k| k as i64 - 1);
                    let w = [0, 1, 2].map(|a| v[a] as i64 + d[a]);
                    if (0..3).any(|a| w[a] < 0 || w[a] as usize >= vox.dims[a]) {
                        continue;
                    }
                    let w = w.map(|k| k as usize);
                    if !vox.solid(w) && tenebrae_tome::locate::find_cell(off, centre(w)) == Some(g)
                    {
                        near = true;
                        break;
                    }
                }
                assert!(
                    near,
                    "surface voxel {v:?} resolves to cell {g}, beside none of its voxels"
                );
                owned += 1;
            }
        }
    }
    owned
}

/// Which view-tree splits a pinned bake stores.
#[derive(Clone, Copy, PartialEq)]
enum Stored {
    /// One per node (L4 off).
    All,
    /// None (L4 on, no collapse).
    None,
    /// None, except uneven collapsed tiles (L4 on with collapse).
    Fallback,
}

/// Matrix rows "levers off" and "L4 off": both bakes are byte-identical to the baker before L4
/// (`b279953`), pinned by hash. Levers off is also checked against the dense `0ce18d0`
/// reference by the `sparse_matches_dense_*` tests. The L4-on bakes are pinned too, so a change
/// to the midpoint trees (such as the axis choice) shows here, and L4 must shrink the tome.
#[test]
fn l4_off_bakes_are_unchanged() {
    let (mesh, targets) = scene();
    // (Params, tome length, FNV-1a of the tome bytes) of `tenebrae_tome::write(&tome)` for
    // `lever_input(&mesh, &targets, Params { threads: 4, ..p })` on `scene()`. The two L4-off rows
    // were produced at commit `b279953` (the pre-L4 baker, `Params::default()` without the
    // `midpoint_views` field, and `Params::levers_off()`); the two L4-on rows (`Params::default()`,
    // and the same with collapse size 8 so no tile collapses) by the L4 implementation
    // (spec-m3b-l4-midpoint-view-tree, `midpoint_axis` choosing the axis). To regenerate, bake
    // that input and print `tenebrae_tome::write(&tome).len()` and `fnv(&bytes)`; only the L4-on rows may
    // ever change, and only with an intended change to L4's trees. The default L4-on row was
    // regenerated for the explicit fallback of uneven collapsed tiles (2026-10-12), and again when
    // that fallback became a level-order split prefix (2026-10-12); the
    // collapse-size-8 row (no collapse, so it catches a `midpoint_axis` regression) was not. Every
    // row was regenerated once more for the object box rule, which
    // adds object runs and changes nothing else. The two L4-on rows were regenerated for flexible
    // surface owners (`tile::view_of`, any cell among a surface voxel's 26 neighbours), then for
    // the view-volume mask and export simplify. Every row but
    // `levers_off` was regenerated once more when the defaults became Umbra's cell pipeline (8 m
    // components, lever L2 as Umbra's visibility-cost merge, coalesced portals), and again for
    // shared object-run windows. Every row was regenerated once more when each cell came to list
    // its portals within the tile first (the same records, reordered; the collapse-size-8 bake has
    // no cell whose order changed). Every row but `levers_off` was regenerated once more for
    // lever L5 (inner-node LOD tiles), which `levers_off` leaves off. Every row was regenerated
    // once more for the tile `level` rule (`tile_level`: SH x shortest edge / T, inner tiles the
    // least of their children's), which changes one float per tile and nothing else.
    let pinned = [
        (
            Params {
                midpoint_views: false,
                ..Params::default()
            },
            19_328,
            0x64d5_c955_2be4_7a06u64,
            Stored::All,
        ),
        (
            Params::levers_off(),
            53_056,
            0xb219_4e8d_ea07_d62e,
            Stored::All,
        ),
        (
            Params::default(),
            18_544,
            0x4d92_537a_51bc_2746,
            Stored::Fallback,
        ),
        (
            Params {
                collapse_size: 8.0,
                ..Params::default()
            },
            65_152,
            0x141e_3dc2_b7b4_8640,
            Stored::None,
        ),
    ];
    for (p, len, hash, stored) in pinned {
        let what = format!("{p:?}");
        let input = lever_input(&mesh, &targets, Params { threads: 4, ..p });
        let (tome, _) = bake(&input).expect("bake");
        let bytes = tenebrae_tome::write(&tome);
        assert_eq!((bytes.len(), fnv(&bytes)), (len, hash), "{what}");
        if stored == Stored::Fallback {
            assert_l4_view_splits(&tome);
            continue;
        }
        for t in tome.tiles.iter().flatten().filter(|t| t.view.is_some()) {
            let tree = &t.view.as_ref().expect("view tree").tree;
            let n = if stored == Stored::All {
                tree.codes.len()
            } else {
                0
            };
            assert_eq!(tree.splits.len(), n, "{what}");
        }
    }
    // L4 shrinks the tome, without collapse and with it: this small scene's uneven 24 m unions
    // store only their seam splits (9,904 B on, 10,352 B off; before the object box rule 9,872 B
    // and 10,320 B, and 11,104 B on when they stored every split).
    let size = |midpoint_views: bool, collapse_size: f32| {
        let p = Params {
            threads: 4,
            collapse_size,
            midpoint_views,
            ..Params::default()
        };
        tenebrae_tome::write(&bake(&lever_input(&mesh, &targets, p)).expect("bake").0).len()
    };
    for collapse in [8.0, 32.0] {
        let (on, off) = (size(true, collapse), size(false, collapse));
        assert!(on < off, "collapse {collapse}: L4 on {on} B, off {off} B");
    }
}

/// The other half of the defaults pin: `Report::cell_samples` and the object map of the default
/// bake of `scene()` (4 threads) as `a6c1243`, the M4 baseline, produced them (samples as
/// length-prefixed f32 bits, then the map with `None` as `u32::MAX`), except that target 2, in the
/// sealed room, is kept since objects are also listed in cells whose box overlaps theirs
/// (`plan-bench-gaps.md` item 3). Regenerated when the defaults became Umbra's cell pipeline (8 m
/// components and lever L2 as Umbra's merge), which changes the cells and so their samples.
#[test]
fn default_samples_and_map_are_unchanged() {
    let (mesh, targets) = scene();
    let p = Params {
        threads: 4,
        ..Params::default()
    };
    let (_, report, map) = bake_map(&lever_input(&mesh, &targets, p));
    let mut b = Vec::new();
    for list in &report.cell_samples {
        b.extend((list.len() as u32).to_le_bytes());
        for x in list.iter().flatten() {
            b.extend(x.to_bits().to_le_bytes());
        }
    }
    for m in &map {
        b.extend(m.unwrap_or(u32::MAX).to_le_bytes());
    }
    assert_eq!((b.len(), fnv(&b)), (6_972, 0xaf9e_7760_04c1_7b49));
    assert_eq!(map, [Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]);
}

/// Matrix rows "threads" and "round trip": with L4 on, threads 1 and 4 bake the same bytes, the
/// tome round-trips, stores no view split, every leaf value is a tile cell or the solid value,
/// and the cells are the ones the L4-off bake makes.
#[test]
fn l4_is_thread_independent_and_round_trips() {
    let (mesh, targets) = scene();
    let mut outs = Vec::new();
    for threads in [1, 4] {
        let input = lever_input(
            &mesh,
            &targets,
            Params {
                threads,
                ..Params::default()
            },
        );
        let (tome, report, map) = bake_map(&input);
        assert_valid(&tome);
        assert_l4_view_splits(&tome);
        assert_samples_find_their_cells(&tome, &report, 0.5, &input.view_volume);
        outs.push((tenebrae_tome::write(&tome), report.cell_samples, map));
    }
    assert!(outs[0] == outs[1], "1 vs 4 threads");
    let input = lever_input(
        &mesh,
        &targets,
        Params {
            threads: 4,
            midpoint_views: false,
            ..Params::default()
        },
    );
    let (_, off_report, off_map) = bake_map(&input);
    assert_eq!(off_report.cell_samples, outs[0].1, "L4 changes no cell");
    assert_eq!(off_map, outs[0].2);
}

/// Point queries through the runtime-equivalent descent find each sampled open voxel's own cell,
/// with and without L4, levers on and off.
#[test]
fn point_queries_find_each_open_voxels_cell() {
    let (mesh, targets) = scene();
    for base in [Params::default(), Params::levers_off()] {
        for midpoint_views in [false, true] {
            for view_mask in [false, true] {
                let p = Params {
                    threads: 4,
                    midpoint_views,
                    view_mask,
                    ..base.clone()
                };
                let input = lever_input(&mesh, &targets, p);
                let (tome, report) = bake(&input).expect("bake");
                assert_samples_find_their_cells(&tome, &report, 0.5, &input.view_volume);
            }
        }
    }
}

/// Matrix row "non-dyadic box": 6 m tiles of 0.5 m voxels span 12 voxels, so midpoints fall
/// inside voxels from the third level down. Leaves stay conservative, the tome is valid, and the
/// cells are those of the L4-off bake. With the default collapse size the 6 m tiles also
/// collapse into uneven unions, which keep explicit splits.
#[test]
fn l4_non_dyadic_tiles_stay_conservative() {
    let (mesh, targets) = scene();
    for base in [Params::default(), Params::levers_off()] {
        let input = |midpoint_views: bool| Input {
            params: Params {
                tile: 6.0,
                voxel: 0.5,
                threads: 4,
                midpoint_views,
                ..base.clone()
            },
            ..lever_input(&mesh, &targets, Params::default())
        };
        let (tome, report) = bake(&input(true)).expect("bake");
        assert_valid(&tome);
        assert_l4_view_splits(&tome);
        assert_samples_find_their_cells(&tome, &report, 0.5, &input(true).view_volume);
        let (off, off_report) = bake(&input(false)).expect("bake");
        assert_eq!(off_report.cell_samples, report.cell_samples);
        let vox = voxelize(&input(true)).expect("voxelize");
        assert!(assert_open_voxels_match(&tome, &off, &vox) > 0);
    }
}

/// Collapse under L4: three 16 m tiles in a row collapse into one 48 m tile whose shared plane
/// (x = 16) is not the union's midpoint (x = 24), so it keeps the assembled tree and stores that
/// one split: both 16 m and 32 m halves below are midpoint trees over their own boxes. The tree
/// is the L4-off tree, which stores every split. On the multi-tile scene (8 m
/// tiles collapsing into 16 m and 24 m unions) the uneven unions store splits and every other
/// tile stores none.
#[test]
fn l4_uneven_collapsed_tiles_keep_explicit_splits() {
    let mesh = Mesh::default();
    let input = |midpoint_views: bool| Input {
        occluders: Occluders::Soup(&mesh),
        targets: &[],
        view_volume: aabb([0.0, 0.0, 0.0], [48.0, 16.0, 16.0]),
        seeds: &[[1.0, 1.0, 1.0]],
        params: Params {
            collapse_size: 48.0,
            reachability_threshold: 0.0,
            midpoint_views,
            ..params(
                16.0,
                1.0,
                CellMode::Components { block: 8.0 },
                SolidMode::Flood,
            )
        },
    };
    let (tome, report) = bake(&input(true)).expect("bake");
    assert_eq!(report.tiles, 1);
    assert_eq!(report.cells, 24);
    let tile = tome.tiles[0].as_ref().expect("merged tile");
    assert_eq!(tile.bounds.min, [0.0, 0.0, 0.0]);
    assert_eq!(tile.bounds.max, [48.0, 16.0, 16.0]);
    assert_valid(&tome);
    let tree = &tile.view.as_ref().expect("view").tree;
    assert!(tree.codes.len() > 1);
    assert_eq!(tree.splits, vec![16.0], "only the seam is stored");
    assert_samples_find_their_cells(&tome, &report, 1.0, &input(true).view_volume);
    let (off, off_report) = bake(&input(false)).expect("bake");
    assert_eq!(off_report.cell_samples, report.cell_samples);
    assert_samples_find_their_cells(&off, &off_report, 1.0, &input(true).view_volume);
    // Same tree as with L4 off, read through the prefix: same codes, leaves and leaf boxes.
    let leaves_of = |t: &crate::Tome| {
        let tile = t.tiles[0].as_ref().expect("merged tile");
        let view = tile.view.as_ref().expect("view");
        (
            view.tree.codes.clone(),
            view.leaf_values.clone(),
            tenebrae_tome::locate::leaves(&view.tree, tile.bounds),
        )
    };
    assert!(
        leaves_of(&tome) == leaves_of(&off),
        "the uneven tile's tree is the L4-off tree"
    );
    assert!(tenebrae_tome::write(&tome).len() < tenebrae_tome::write(&off).len());
    // The assembled tree's root is the shared plane x = 16, not the union's midpoint x = 24.
    let off_tree = &off.tiles[0]
        .as_ref()
        .expect("merged tile")
        .view
        .as_ref()
        .expect("view")
        .tree;
    assert_eq!((off_tree.codes[0], off_tree.splits[0]), (0, 16.0));
    let vox = voxelize(&input(true)).expect("voxelize");
    assert_open_voxels_match(&tome, &off, &vox);

    // Mixed tiles: uneven collapsed unions store their (non-midpoint) splits, the rest none, and
    // every open voxel still resolves as with L4 off.
    let (mesh, targets) = scene();
    let input = |midpoint_views: bool| {
        let p = Params {
            threads: 4,
            midpoint_views,
            ..Params::default()
        };
        lever_input(&mesh, &targets, p)
    };
    let (tome, report) = bake(&input(true)).expect("bake");
    assert_valid(&tome);
    assert_samples_find_their_cells(&tome, &report, 0.5, &input(true).view_volume);
    let (explicit, free) = assert_l4_view_splits(&tome);
    assert!(
        explicit > 0 && free > 0,
        "{explicit} explicit, {free} split-free"
    );
    let (off, _) = bake(&input(false)).expect("bake");
    let vox = voxelize(&input(true)).expect("voxelize");
    assert_open_voxels_match(&tome, &off, &vox);
}

/// Everything a bake returns that a cached re-bake must reproduce exactly: the tome bytes and
/// every report field except the cache counters.
type Outcome = (
    Vec<u8>,
    [usize; 11],
    [usize; 3],
    Vec<Vec<Vec3>>,
    Vec<Option<Vec3>>,
    u64,
);

fn outcome(tome: &crate::Tome, r: &Report) -> Outcome {
    (
        tenebrae_tome::write(tome),
        [
            r.tiles,
            r.cells,
            r.portals,
            r.buried_targets,
            r.seeds,
            r.cell_samples.len(),
            r.global_cells,
            r.global_portals,
            r.objects,
            r.dropped_targets,
            r.sealed_targets,
        ],
        r.voxels,
        r.cell_samples.clone(),
        r.cell_points.clone(),
        r.grid_bytes,
    )
}

/// M3c tests: the shared multi-tile scene, levers on, 4 threads.
fn cache_params() -> Params {
    Params {
        threads: 4,
        ..Params::default()
    }
}

/// A full bake's cache.
fn first_cache(input: &Input<'_>) -> TileCache {
    let (_, report, cache) = bake_cached(input, &TileCache::new()).expect("bake");
    assert_eq!(report.cache_hits, [0, 0]);
    assert!(!report.cache_rejected);
    cache
}

/// Re-bakes `input` with the cache parsed from `bytes`, asserts the result is exactly a fresh
/// bake's, and returns the cached bake's report and cache.
fn rebake(input: &Input<'_>, bytes: &[u8]) -> (Report, TileCache) {
    let cache = TileCache::from_bytes(bytes);
    let (tome, report, next) = bake_cached(input, &cache).expect("cached bake");
    let (fresh, fresh_report) = bake(input).expect("fresh bake");
    assert!(
        outcome(&tome, &report) == outcome(&fresh, &fresh_report),
        "a cached re-bake differs from a fresh bake"
    );
    assert_eq!(report.cache_rejected, cache.rejected());
    let (_, map, _, _) = bake_with(input, Some(&cache), &mut Progressor::none()).expect("bake");
    assert_eq!(map, bake_map(input).2, "object maps differ");
    // The returned cache holds this bake's tiles only, and the same ones a fresh bake returns.
    let (_, _, fresh_cache) = bake_cached(input, &TileCache::new()).expect("fresh bake");
    assert!(next.to_bytes() == fresh_cache.to_bytes(), "caches differ");
    (report, next)
}

fn without_triangles(mesh: &Mesh, range: std::ops::Range<usize>) -> Mesh {
    let mut m = mesh.clone();
    m.triangles.drain(range.clone());
    m.owners.drain(range);
    m
}

/// Matrix row "no edit": every tile hits both levels and the cache comes back unchanged.
#[test]
fn cache_without_edit_hits_every_tile() {
    let (mesh, targets) = scene();
    let input = lever_input(&mesh, &targets, cache_params());
    let cache = first_cache(&input);
    let n = cache.entries();
    // 8 x 8 x 5 tiles of 8 m.
    assert_eq!(n, [320, 320]);
    let (report, next) = rebake(&input, &cache.to_bytes());
    assert_eq!(report.cache_hits, n);
    assert!(!report.cache_rejected);
    assert!(next.to_bytes() == cache.to_bytes());
}

/// Matrix row "add mesh": a box inside one tile misses that tile's pass A, and pass B of the tile
/// and its six neighbours (their flood inputs changed); every other tile hits.
#[test]
fn cache_after_adding_a_box_misses_only_tiles_it_reaches() {
    let (mesh, targets) = scene();
    let cache = first_cache(&lever_input(&mesh, &targets, cache_params()));
    let n = cache.entries();
    let mut edited = mesh.clone();
    cube(&mut edited, [19.0, 19.0, 11.0], [21.0, 21.0, 13.0]);
    let (report, _) = rebake(
        &lever_input(&edited, &targets, cache_params()),
        &cache.to_bytes(),
    );
    assert_eq!(report.cache_hits, [n[0] - 1, n[1] - 7]);
}

/// Matrix row "remove mesh": the sealed room's +x wall removed opens the room.
#[test]
fn cache_after_removing_a_wall_matches_a_fresh_bake() {
    let (mesh, targets) = scene();
    let cache = first_cache(&lever_input(&mesh, &targets, cache_params()));
    let n = cache.entries();
    // Cubes push 12 triangles each; the room's +x wall is the eighth cube.
    let edited = without_triangles(&mesh, 84..96);
    let (report, _) = rebake(
        &lever_input(&edited, &targets, cache_params()),
        &cache.to_bytes(),
    );
    let [a, b] = report.cache_hits;
    assert!(a < n[0] && b < n[1] && b > 0, "{:?}", report.cache_hits);
}

/// Matrix row "move mesh": the pillar moved 3 m along x, across the tile plane x = -8.
#[test]
fn cache_after_moving_a_box_across_tiles_matches_a_fresh_bake() {
    let (mesh, targets) = scene();
    let cache = first_cache(&lever_input(&mesh, &targets, cache_params()));
    let n = cache.entries();
    let mut edited = mesh.clone();
    // The pillar is the eleventh cube: vertices 80..88.
    for v in &mut edited.vertices[80..88] {
        v[0] += 3.0;
    }
    let (report, _) = rebake(
        &lever_input(&edited, &targets, cache_params()),
        &cache.to_bytes(),
    );
    let [a, b] = report.cache_hits;
    assert!(a < n[0] && b < n[1] && b > 0, "{:?}", report.cache_hits);
}

/// Matrix row "target only": a target added in front of the list (every other target's index
/// shifts) misses only pass B of the one tile its grown box reaches.
#[test]
fn cache_after_adding_a_target_misses_only_its_tile() {
    let (mesh, targets) = scene();
    let cache = first_cache(&lever_input(&mesh, &targets, cache_params()));
    let n = cache.entries();
    let mut edited = vec![target([19.5, 19.5, 9.5], [20.5, 20.5, 10.5], 99)];
    edited.extend_from_slice(&targets);
    let (report, _) = rebake(
        &lever_input(&mesh, &edited, cache_params()),
        &cache.to_bytes(),
    );
    assert_eq!(report.cache_hits, [n[0], n[1] - 1]);
}

/// Matrix row "grow volume": a larger view volume is another grid, so every tile misses.
#[test]
fn cache_after_growing_the_view_volume_misses_every_tile() {
    let (mesh, targets) = scene();
    let cache = first_cache(&lever_input(&mesh, &targets, cache_params()));
    let mut input = lever_input(&mesh, &targets, cache_params());
    input.view_volume.max[0] = 40.0;
    let (report, next) = rebake(&input, &cache.to_bytes());
    assert_eq!(report.cache_hits, [0, 0]);
    assert!(!report.cache_rejected);
    assert_eq!(next.entries(), [360, 360]);
}

/// Matrix row "parity": parity tiles depend on the whole mesh, so an occluder edit misses every
/// pass B; a target-only edit still reuses them. Parity has no pass A.
#[test]
fn cache_in_parity_mode() {
    let (mesh, targets) = scene();
    let p = || Params {
        solid: SolidMode::Parity,
        ..cache_params()
    };
    let cache = first_cache(&lever_input(&mesh, &targets, p()));
    let n = cache.entries();
    assert_eq!(n, [0, 320]);
    let (report, _) = rebake(&lever_input(&mesh, &targets, p()), &cache.to_bytes());
    assert_eq!(report.cache_hits, [0, n[1]]);
    let mut edited = mesh.clone();
    cube(&mut edited, [19.0, 19.0, 11.0], [21.0, 21.0, 13.0]);
    let (report, _) = rebake(&lever_input(&edited, &targets, p()), &cache.to_bytes());
    assert_eq!(report.cache_hits, [0, 0]);
    let mut more = vec![target([19.5, 19.5, 9.5], [20.5, 20.5, 10.5], 99)];
    more.extend_from_slice(&targets);
    let (report, _) = rebake(&lever_input(&mesh, &more, p()), &cache.to_bytes());
    assert_eq!(report.cache_hits, [0, n[1] - 1]);
}

/// Matrix row "bad cache": truncated, foreign-magic, other-version, corrupted or empty bytes are
/// rejected and the bake is a full one. Payloads that do not decode (behind valid framing and
/// keys) simply miss.
#[test]
fn bad_caches_are_ignored() {
    let (mesh, targets) = scene();
    let input = lever_input(&mesh, &targets, cache_params());
    let cache = first_cache(&input);
    let good = cache.to_bytes();
    let mut bad: Vec<Vec<u8>> = vec![Vec::new(), good[..good.len() - 1].to_vec()];
    for at in [0, 4, 8, 30, good.len() / 2, good.len() - 1] {
        let mut b = good.clone();
        b[at] ^= 1;
        bad.push(b);
    }
    let mut longer = good.clone();
    longer.push(0);
    bad.push(longer);
    for bytes in &bad {
        let parsed = TileCache::from_bytes(bytes);
        assert!(parsed.rejected(), "accepted {} bytes", bytes.len());
        assert_eq!(parsed.entries(), [0, 0]);
        let (report, _) = rebake(&input, bytes);
        assert!(report.cache_rejected);
        assert_eq!(report.cache_hits, [0, 0]);
    }
    for cut in [1usize, 9] {
        let mut broken = cache.clone();
        broken.map_payloads(|p| {
            let len = p.len().saturating_sub(cut);
            p.truncate(len);
        });
        let (report, _) = rebake(&input, &broken.to_bytes());
        assert!(!report.cache_rejected);
        assert_eq!(report.cache_hits, [0, 0]);
    }
}

/// Matrix row "threads": a cached re-bake after an edit gives the same tome, report and cache
/// bytes on 1 and 4 threads.
#[test]
fn cached_rebake_is_thread_independent() {
    let (mesh, targets) = scene();
    let bytes = first_cache(&lever_input(&mesh, &targets, cache_params())).to_bytes();
    let mut edited = mesh.clone();
    cube(&mut edited, [19.0, 19.0, 11.0], [21.0, 21.0, 13.0]);
    let mut outs = Vec::new();
    for threads in [1, 4] {
        let p = Params {
            threads,
            ..cache_params()
        };
        let input = lever_input(&edited, &targets, p);
        let (tome, report, next) =
            bake_cached(&input, &TileCache::from_bytes(&bytes)).expect("bake");
        let mut o = outcome(&tome, &report);
        // Grid storage counts the `threads` largest tiles, so it is the one thread-dependent field.
        o.5 = 0;
        outs.push((o, report.cache_hits, next.to_bytes()));
    }
    assert!(outs[0] == outs[1], "1 vs 4 threads");
}

/// Ties the cache to the code that writes it. A cache entry is only valid while the tile passes
/// and the payload layout give the same bytes, and nothing checks that at run time: an old cache
/// with matching keys would be reused. If this pin changes, the tile passes, the key material or
/// the payload layout changed, so bump `cache::VERSION` (old caches then miss) and update the pin
/// (length and FNV-1a of `to_bytes()` for this input; the version bump itself changes them too).
/// Pinned at cache version 6.
#[test]
fn cache_bytes_are_pinned_to_the_cache_version() {
    let (mesh, targets) = scene();
    let cache = first_cache(&lever_input(&mesh, &targets, cache_params()));
    let bytes = cache.to_bytes();
    assert_eq!(
        (bytes.len(), fnv(&bytes)),
        (466_122, 0x8b18_5d8a_8b10_c310),
        "cache bytes changed: bump cache::VERSION and update this pin"
    );
}

/// Seed edits: clearing every seed switches the flood to the domain boundary, a flag every pass-A
/// key carries, so every tile misses; moving the sealed room's seed into the closed pillar changes
/// the two tiles' pass A and the flood far beyond them.
#[test]
fn cache_after_seed_edits_matches_a_fresh_bake() {
    let (mesh, targets) = scene();
    let cache = first_cache(&lever_input(&mesh, &targets, cache_params()));
    let n = cache.entries();
    let mut input = lever_input(&mesh, &targets, cache_params());
    input.seeds = &[];
    let (report, _) = rebake(&input, &cache.to_bytes());
    assert_eq!(report.cache_hits, [0, 0]);
    // [9, 9, 3] (inside the sealed room) moves into the closed pillar.
    let moved = [SEEDS[0], SEEDS[1], [-10.7, 7.3, 3.0]];
    let mut input = lever_input(&mesh, &targets, cache_params());
    input.seeds = &moved;
    let (report, _) = rebake(&input, &cache.to_bytes());
    let [a, b] = report.cache_hits;
    assert_eq!(a, n[0] - 2);
    assert!(b < a, "{:?}", report.cache_hits);
}

/// Tile-stage settings: every key carries the whole tile-stage `Params` signature (spec key
/// material A), so a change of any of them misses every tile, pass A included, even for the
/// settings pass A does not read (`midpoint_views`, `cell_merge`, `max_cells_per_tile`): the key
/// is deliberately coarser than the dependency.
#[test]
fn cache_after_tile_stage_params_change_misses_every_tile() {
    let (mesh, targets) = scene();
    let bytes = first_cache(&lever_input(&mesh, &targets, cache_params())).to_bytes();
    let base = cache_params();
    let changed = [
        Params {
            midpoint_views: !base.midpoint_views,
            ..cache_params()
        },
        Params {
            surface_occludes: !base.surface_occludes,
            ..cache_params()
        },
        Params {
            cell_merge: base.cell_merge + 2.0,
            ..cache_params()
        },
        Params {
            max_cells_per_tile: base.max_cells_per_tile / 2,
            ..cache_params()
        },
    ];
    for p in changed {
        let what = format!("{p:?}");
        let (report, _) = rebake(&lever_input(&mesh, &targets, p), &bytes);
        assert_eq!(report.cache_hits, [0, 0], "{what}");
    }
}

/// A box moved inside one 8 m tile: only its triangles' corner bits change, so the tile misses
/// pass A and the tile and its six neighbours miss pass B.
#[test]
fn cache_after_moving_a_box_within_a_tile() {
    let (mesh, targets) = scene();
    let mut before = mesh.clone();
    cube(&mut before, [19.0, 19.0, 11.0], [21.0, 21.0, 13.0]);
    let cache = first_cache(&lever_input(&before, &targets, cache_params()));
    let n = cache.entries();
    let mut after = mesh.clone();
    cube(&mut after, [18.0, 18.0, 11.0], [20.0, 20.0, 13.0]);
    let (report, _) = rebake(
        &lever_input(&after, &targets, cache_params()),
        &cache.to_bytes(),
    );
    assert_eq!(report.cache_hits, [n[0] - 1, n[1] - 7]);
}

// M4: the bake contract (spec-m4-bake-api).

/// A unit cube in its own space, as one mesh, and `scene()`'s boxes placed as scaled and
/// translated instances, plus one rotated instance, and an empty mesh placed first, between and
/// last (empty instances share a triangle start); returns the meshes and instances.
fn instanced_scene() -> (Vec<Mesh>, Vec<Instance>) {
    let mut unit = Mesh::default();
    cube(&mut unit, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    // A sheet with no owners: instanced meshes may leave them empty.
    let mut sheet = Mesh::default();
    tri(
        &mut sheet,
        [0.0, 0.0, 0.0],
        [4.0, 0.0, 0.0],
        [0.0, 3.0, 2.5],
    );
    sheet.owners.clear();
    let boxes = [
        ([-20.0, -20.0, -1.0], [20.0, 20.0, 0.0]),
        ([-1.0, -20.0, 0.0], [1.0, -2.0, 9.0]),
        ([-1.0, 2.0, 0.0], [1.0, 20.0, 9.0]),
        ([-1.0, -2.0, 4.0], [1.0, 2.0, 9.0]),
        ([5.0, 5.0, 0.0], [13.0, 13.0, 0.5]),
        ([-12.0, 6.0, 0.0], [-9.5, 8.7, 7.3]),
    ];
    let mut instances: Vec<Instance> = boxes
        .iter()
        .map(|(lo, hi): &(Vec3, Vec3)| Instance {
            mesh: 0,
            transform: [
                [hi[0] - lo[0], 0.0, 0.0, lo[0]],
                [0.0, hi[1] - lo[1], 0.0, lo[1]],
                [0.0, 0.0, hi[2] - lo[2], lo[2]],
            ],
        })
        .collect();
    // A pillar turned 30 degrees about z, and the sheet twice.
    let (c, s) = (0.866_025_4f32, 0.5f32);
    instances.push(Instance {
        mesh: 0,
        transform: [
            [2.0 * c, -2.0 * s, 0.0, 14.0],
            [2.0 * s, 2.0 * c, 0.0, -12.0],
            [0.0, 0.0, 6.5, 0.0],
        ],
    });
    for at in [[-15.0, -15.0, 0.3], [3.1, -14.2, 0.0]] {
        instances.push(Instance {
            mesh: 1,
            transform: [
                [1.0, 0.0, 0.0, at[0]],
                [0.0, 1.0, 0.0, at[1]],
                [0.0, 0.0, 1.0, at[2]],
            ],
        });
    }
    let empty = Instance {
        mesh: 2,
        transform: instances[0].transform,
    };
    for at in [instances.len(), 6, 6, 0] {
        instances.insert(at, empty);
    }
    (vec![unit, sheet, Mesh::default()], instances)
}

/// The world-space soup of an instanced scene, built through [`Instance::apply`].
fn soup_of(meshes: &[Mesh], instances: &[Instance]) -> Mesh {
    let mut out = Mesh::default();
    for i in instances {
        let m = &meshes[i.mesh as usize];
        let base = out.vertices.len() as u32;
        out.vertices.extend(m.vertices.iter().map(|v| i.apply(*v)));
        out.triangles
            .extend(m.triangles.iter().map(|t| t.map(|k| base + k)));
        out.owners.extend(m.triangles.iter().map(|_| 0));
    }
    out
}

/// Everything a bake returns, for identity checks (cache counters excluded).
fn everything(b: &Baked) -> (Outcome, Vec<Option<u32>>) {
    (outcome(&b.tome, &b.report), b.object_map.clone())
}

/// Matrix rows "Instanced": an instanced input bakes to exactly the bytes of its applied soup, at
/// 1 and 4 threads, flood and parity, cached and fresh.
#[test]
fn instanced_occluders_bake_like_their_soup() {
    let (meshes, instances) = instanced_scene();
    let soup = soup_of(&meshes, &instances);
    let (_, targets) = scene();
    for solid in [SolidMode::Flood, SolidMode::Parity] {
        let mut first = None;
        for threads in [1, 4] {
            let p = Params {
                threads,
                solid,
                ..Params::default()
            };
            let soup_input = lever_input(&soup, &targets, p.clone());
            let inst_input = Input {
                occluders: Occluders::Instanced {
                    meshes: &meshes,
                    instances: &instances,
                },
                ..lever_input(&soup, &targets, p)
            };
            let a = super::bake(&soup_input, None, None).expect("soup bake");
            let b = super::bake(&inst_input, None, None).expect("instanced bake");
            assert!(
                everything(&a) == everything(&b),
                "instanced differs from soup ({solid:?}, {threads} threads)"
            );
            // Cached: the instanced cache serves the soup bake, so the keys agree too.
            let c = super::bake(&inst_input, Some(&[]), None).expect("cached bake");
            assert!(everything(&a) == everything(&c));
            let d = super::bake(&soup_input, c.cache.as_deref(), None).expect("rebake");
            assert!(everything(&a) == everything(&d));
            assert_eq!(d.report.cache_misses, [0, 0], "every tile hits ({solid:?})");
            // `grid_bytes` counts the `threads` largest tiles by definition.
            let mut e = everything(&a);
            e.0.5 = 0;
            match &first {
                None => first = Some(e),
                Some(f) => assert!(*f == e, "thread count changed the bake"),
            }
        }
    }
}

/// Matrix row "Instance index", and the per-mesh checks on instanced meshes.
#[test]
fn invalid_instances_are_errors() {
    let (meshes, instances) = instanced_scene();
    let (_, targets) = scene();
    let check = |meshes: &[Mesh], instances: &[Instance]| {
        let soup = Mesh::default();
        let input = Input {
            occluders: Occluders::Instanced { meshes, instances },
            ..lever_input(&soup, &targets, Params::default())
        };
        match super::bake(&input, None, None) {
            Err(Error::Params(what)) => what,
            other => panic!("expected a params error, got {:?}", other.map(|_| ())),
        }
    };
    let mut bad = instances.clone();
    bad[2].mesh = meshes.len() as u32;
    assert_eq!(check(&meshes, &bad), "instance mesh index out of range");
    for x in [f32::NAN, f32::INFINITY] {
        let mut bad = instances.clone();
        bad[0].transform[1][3] = x;
        assert_eq!(check(&meshes, &bad), "non-finite instance transform");
    }
    let mut m = meshes.clone();
    m[1].triangles.push([0, 1, 7]);
    assert_eq!(check(&m, &instances), "triangle vertex index out of range");
    let mut m = meshes.clone();
    m[0].owners.pop();
    assert_eq!(
        check(&m, &instances),
        "owners and triangles differ in length"
    );
}

/// Matrix row "Triangle count": numbering refuses an instanced total over `u32::MAX`, and so does
/// the bake.
#[test]
fn instanced_triangle_totals_are_checked() {
    let mut big = Mesh::default();
    cube(&mut big, [0.0; 3], [1.0; 3]);
    let one = Instance {
        mesh: 0,
        transform: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
    };
    let meshes = [big];
    // 12 triangles per instance: the total fits.
    let ok = vec![one; 1000];
    let tris = Tris::new(Occluders::Instanced {
        meshes: &meshes,
        instances: &ok,
    })
    .expect("fits");
    assert_eq!(tris.len(), 12_000);
    // The last instance's triangles, through the partition: equal to its applied corners.
    let c = tris.corners(11_999);
    let m = &meshes[0];
    let t = m.triangles[11];
    assert_eq!(c, t.map(|i| one.apply(m.vertices[i as usize])));
    // Empty instances share a start and are skipped.
    let empty = [Mesh::default()];
    let none = Tris::new(Occluders::Instanced {
        meshes: &empty,
        instances: &ok,
    })
    .expect("fits");
    assert_eq!(none.len(), 0);
    // 65,536 triangles on three vertices, placed 65,537 times: 2^32 + 65,536 triangles.
    let fan = Mesh {
        vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]; 1 << 16],
        ..Mesh::default()
    };
    let fans = [fan];
    let many = vec![one; (1 << 16) + 1];
    let over = Occluders::Instanced {
        meshes: &fans,
        instances: &many,
    };
    assert!(Tris::new(over).is_none());
    let (_, targets) = scene();
    let soup = Mesh::default();
    let input = Input {
        occluders: over,
        ..lever_input(&soup, &targets, Params::default())
    };
    assert!(matches!(
        super::bake(&input, None, None),
        Err(Error::Params("more than u32::MAX occluder triangles"))
    ));
}

/// Matrix rows "Keep every target" and "Drop (default)".
#[test]
fn keep_every_target_keeps_identity() {
    let (mesh, targets) = sealed_pocket_scene();
    let mut input = sealed_pocket_input(&mesh, &targets);
    input.params.reachability_threshold = 0.1;
    let (dropped, dreport, dmap) = bake_map(&input);
    assert_eq!(dmap, vec![Some(0), None, Some(1), Some(2)]);
    assert_eq!((dreport.dropped_targets, dreport.sealed_targets), (1, 0));
    input.params.keep_every_target = true;
    let (tome, report, map) = bake_map(&input);
    assert_eq!(map, vec![Some(0), Some(1), Some(2), Some(3)]);
    assert_eq!((report.dropped_targets, report.sealed_targets), (0, 1));
    assert_eq!(report.objects, 4);
    assert_eq!(tome.users.ids, vec![1, 2, 3, 4]);
    let boxes: Vec<Aabb> = targets.iter().map(|t| t.bounds).collect();
    assert_eq!(tome.objects, boxes, "object i is target i's box");
    // The kept target is in no cell; the cells are those of the dropping bake.
    let listed = |t: &Tome, o: u32| {
        t.object_runs
            .entries
            .iter()
            .any(|r| (r.index..r.index + r.run).contains(&o))
    };
    assert!(!listed(&tome, 1), "the sealed target is listed in no cell");
    assert!(listed(&tome, 0) && listed(&tome, 3));
    assert_eq!(tome.global_cells, dropped.global_cells);
    assert_eq!(tome.tiles.len(), dropped.tiles.len());
    let bytes = tenebrae_tome::write(&tome);
    assert_eq!(
        tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read")),
        bytes
    );
    assert_samples_find_their_cells(&tome, &report, 0.5, &input.view_volume);
}

/// Matrix row "Users": stock multi-user lists are stored as given, with a prefix; one user per
/// target stores none.
#[test]
fn targets_carry_their_user_lists() {
    let (mesh, targets) = scene();
    let lists: [&[u32]; 6] = [&[7], &[9, 90, 900], &[11], &[13], &[15, 150], &[17]];
    let multi: Vec<Target> = targets
        .iter()
        .zip(lists)
        .map(|(t, users)| Target {
            bounds: t.bounds,
            users,
        })
        .collect();
    let p = Params {
        threads: 4,
        keep_every_target: true,
        ..Params::default()
    };
    let b = super::bake(&lever_input(&mesh, &multi, p.clone()), None, None).expect("bake");
    assert_eq!(b.tome.users.ids, vec![7, 9, 90, 900, 11, 13, 15, 150, 17]);
    assert_eq!(b.tome.users.prefix, Some(vec![0, 1, 4, 5, 6, 8, 9]));
    let bytes = tenebrae_tome::write(&b.tome);
    let back = tenebrae_tome::read(&bytes).expect("read");
    assert_eq!(back.users, b.tome.users);
    // The users change nothing else.
    let single = super::bake(&lever_input(&mesh, &targets, p), None, None).expect("bake");
    assert_eq!(single.tome.users.prefix, None);
    let strip = |mut t: Tome| {
        t.users = single.tome.users.clone();
        tenebrae_tome::write(&t)
    };
    assert!(strip(b.tome) == tenebrae_tome::write(&single.tome));
    // Matrix row "Users, 0 users": refused.
    let mut none = multi.clone();
    none[2].users = &[];
    assert!(matches!(
        super::bake(&lever_input(&mesh, &none, Params::default()), None, None),
        Err(Error::Params("target without a user"))
    ));
}

/// Matrix row "Presets".
#[test]
fn d2_presets_are_the_parity_setting() {
    let a = Params::tome_a();
    assert!(a.keep_every_target);
    let plain = Params {
        keep_every_target: true,
        ..Params::default()
    };
    assert_eq!(format!("{a:?}"), format!("{plain:?}"));
    let b = Params::tome_b();
    assert_eq!(b.voxel, 0.5);
    assert!(!b.lod_tiles);
    assert_eq!(
        format!(
            "{:?}",
            Params {
                voxel: 0.125,
                lod_tiles: true,
                ..b
            }
        ),
        format!("{a:?}")
    );
    // Both bake on the shared scene.
    let (mesh, targets) = scene();
    for p in [a, Params::tome_b()] {
        let p = Params { threads: 4, ..p };
        let b = super::bake(&lever_input(&mesh, &targets, p), None, None).expect("bake");
        assert_eq!(b.object_map, (0..6).map(Some).collect::<Vec<_>>());
    }
}

/// Matrix row "Cell points", and the report's counts.
#[test]
fn cell_points_find_their_cells() {
    let (mesh, targets) = scene();
    for p in [
        Params::default(),
        Params::levers_off(),
        Params {
            collapse_size: 0.0,
            ..Params::default()
        },
        Params {
            solid: SolidMode::Parity,
            ..Params::default()
        },
    ] {
        let p = Params { threads: 4, ..p };
        let b = super::bake(&lever_input(&mesh, &targets, p), None, None).expect("bake");
        let r = &b.report;
        assert_eq!(r.cell_points.len(), b.tome.global_cells.len());
        assert_eq!(r.global_cells, b.tome.global_cells.len());
        assert_eq!(r.global_portals, b.tome.global_portals.len());
        assert_eq!(r.objects, b.tome.objects.len());
        for (g, pt) in r.cell_points.iter().enumerate() {
            let pt = pt.expect("the view volume is the domain: every cell has a point");
            assert!(r.cell_samples[g].contains(&pt));
            assert_eq!(
                tenebrae_tome::locate::find_cell(&b.tome, pt),
                Some(g as u32)
            );
        }
    }
}

/// Records every progress call; stops at `stop_at` (stage, done).
fn recorder(
    log: &mut Vec<Progress>,
    stop_at: Option<(Stage, u64)>,
) -> impl FnMut(Progress) -> Control + '_ {
    move |p| {
        log.push(p);
        if stop_at.is_some_and(|(s, d)| s == p.stage && p.done >= d) {
            Control::Stop
        } else {
            Control::Continue
        }
    }
}

/// Matrix row "Progress": stages in order, fixed totals, monotone counts that reach the total,
/// the same sequence of totals at 1 and 4 threads, and no effect on the output; uncached, and
/// cached (with the keys stage).
#[test]
fn progress_is_monotone_and_complete() {
    let (mesh, targets) = scene();
    for cache in [None, Some(&[][..])] {
        let mut runs = Vec::new();
        for threads in [1, 4] {
            let input = lever_input(
                &mesh,
                &targets,
                Params {
                    threads,
                    ..Params::default()
                },
            );
            let mut log = Vec::new();
            let mut hook = recorder(&mut log, None);
            let b = super::bake(&input, cache, Some(&mut hook)).expect("bake");
            drop(hook);
            let plain = super::bake(&input, None, None).expect("bake");
            assert!(everything(&b) == everything(&plain));
            let order: &[Stage] = match cache {
                Some(_) => &[
                    Stage::Prepare,
                    Stage::Keys,
                    Stage::Flood,
                    Stage::Join,
                    Stage::Cells,
                    Stage::Global,
                ],
                None => &[
                    Stage::Prepare,
                    Stage::Flood,
                    Stage::Join,
                    Stage::Cells,
                    Stage::Global,
                ],
            };
            let mut stages = Vec::new();
            for w in log.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a.stage == b.stage {
                    assert_eq!(a.total, b.total, "{a:?} -> {b:?}");
                    assert!(b.done >= a.done, "{a:?} -> {b:?}");
                } else {
                    assert_eq!(a.done, a.total, "{a:?} ends complete");
                    let pos = |s| order.iter().position(|x| *x == s).expect("known stage");
                    assert!(pos(b.stage) > pos(a.stage), "{a:?} -> {b:?}");
                    assert_eq!(b.done, 0);
                }
                if stages.last() != Some(&a.stage) {
                    stages.push(a.stage);
                }
            }
            let last = log.last().expect("progress");
            assert_eq!((last.stage, last.done, last.total), (Stage::Global, 5, 5));
            stages.push(Stage::Global);
            stages.dedup();
            assert_eq!(stages, order);
            let totals: Vec<(Stage, u64)> = log.iter().map(|p| (p.stage, p.total)).collect();
            let mut totals = totals;
            totals.dedup();
            runs.push(totals);
        }
        assert_eq!(runs[0], runs[1], "thread count changed the stage totals");
    }
}

/// Matrix row "Progress" inside `Prepare`: past `PREPARE_STEP` triangles the bucketing loop
/// reports with the stage's fixed total, and a stop there cancels the bake.
#[test]
fn prepare_reports_while_bucketing() {
    let (mut mesh, targets) = scene();
    for _ in 0..PREPARE_STEP + 10 {
        tri(
            &mut mesh,
            [0.0, 0.0, -0.5],
            [0.01, 0.0, -0.5],
            [0.0, 0.01, -0.5],
        );
    }
    let total = mesh.triangles.len() as u64;
    for threads in [1, 4] {
        let input = lever_input(
            &mesh,
            &targets,
            Params {
                threads,
                ..Params::default()
            },
        );
        let mut log = Vec::new();
        let mut hook = recorder(&mut log, Some((Stage::Prepare, 1)));
        let r = super::bake(&input, None, Some(&mut hook));
        drop(hook);
        assert!(matches!(r, Err(Error::Cancelled)));
        let step = u64::from(PREPARE_STEP);
        let seen: Vec<(Stage, u64, u64)> = log.iter().map(|p| (p.stage, p.done, p.total)).collect();
        assert_eq!(
            seen,
            [
                (Stage::Prepare, 0, total),
                (Stage::Prepare, step - 1, total)
            ]
        );
    }
}

/// Matrix row "Cancel": a stop in any stage returns `Cancelled`, at 1 and 4 threads, without
/// further hook calls.
#[test]
fn a_stop_cancels_the_bake() {
    let (mesh, targets) = scene();
    for threads in [1, 4] {
        for stop in [
            (Stage::Prepare, 0),
            (Stage::Keys, 3),
            (Stage::Flood, 3),
            (Stage::Join, 1),
            (Stage::Cells, 3),
            (Stage::Global, 4),
        ] {
            let input = lever_input(
                &mesh,
                &targets,
                Params {
                    threads,
                    ..Params::default()
                },
            );
            let mut log = Vec::new();
            let mut hook = recorder(&mut log, Some(stop));
            let r = super::bake(&input, Some(&[]), Some(&mut hook));
            drop(hook);
            assert!(
                matches!(r, Err(Error::Cancelled)),
                "{stop:?} at {threads} threads"
            );
            let last = log.last().expect("progress");
            assert_eq!(last.stage, stop.0);
            let first_stop = log
                .iter()
                .position(|p| p.stage == stop.0 && p.done >= stop.1)
                .expect("stopped");
            assert_eq!(first_stop, log.len() - 1, "no call after the stop");
        }
    }
    assert_eq!(Error::Cancelled.to_string(), "bake cancelled");
}

/// Matrix row "Cache": an empty slice starts a cache (no rejection), bad bytes are rejected and
/// ignored, and the counts add up to the grid.
#[test]
fn cache_bytes_in_and_out() {
    let (mesh, targets) = scene();
    let input = lever_input(&mesh, &targets, cache_params());
    let plain = super::bake(&input, None, None).expect("bake");
    assert!(plain.cache.is_none());
    assert_eq!(plain.report.cache_misses, [0, 0]);
    let cold = super::bake(&input, Some(&[]), None).expect("bake");
    assert!(everything(&cold) == everything(&plain));
    assert!(!cold.report.cache_rejected);
    assert_eq!(
        (cold.report.cache_hits, cold.report.cache_misses),
        ([0, 0], [320, 320])
    );
    let bytes = cold.cache.expect("a cache");
    let warm = super::bake(&input, Some(&bytes), None).expect("bake");
    assert_eq!(
        (warm.report.cache_hits, warm.report.cache_misses),
        ([320, 320], [0, 0])
    );
    assert!(warm.cache.as_deref() == Some(bytes.as_slice()));
    let junk = super::bake(&input, Some(b"not a cache"), None).expect("bake");
    assert!(junk.report.cache_rejected);
    assert!(everything(&junk) == everything(&plain));
    // Other tile-stage settings: every tile misses (they are in every key), no rejection.
    let other = lever_input(
        &mesh,
        &targets,
        Params {
            surface_occludes: false,
            ..cache_params()
        },
    );
    let o = super::bake(&other, Some(&bytes), None).expect("bake");
    assert!(!o.report.cache_rejected);
    assert_eq!(
        (o.report.cache_hits, o.report.cache_misses),
        ([0, 0], [320, 320])
    );
}

/// The public types cross threads (contract: every input and output type is `Send`).
#[test]
fn contract_types_are_send() {
    fn send<T: Send>() {}
    send::<Input<'static>>();
    send::<Baked>();
    send::<Error>();
    send::<Progress>();
    send::<Instance>();
}

/// The prepared locator answers exactly as `find_cell`, inside, outside and on tile seams.
#[test]
fn locator_matches_find_cell() {
    let (mesh, targets) = scene();
    for p in [Params::default(), Params::levers_off()] {
        let p = Params { threads: 4, ..p };
        let b = super::bake(&lever_input(&mesh, &targets, p), None, None).expect("bake");
        let loc = tenebrae_tome::locate::Locator::new(&b.tome);
        let mut found = 0;
        for z in -2..62 {
            for y in -66..66 {
                for x in -66..66 {
                    let q = [x as f32 * 0.5 + 0.13, y as f32 * 0.5, z as f32 * 0.5 - 0.29];
                    let want = tenebrae_tome::locate::find_cell(&b.tome, q);
                    assert_eq!(loc.find_cell(q), want, "{q:?}");
                    found += usize::from(want.is_some());
                }
            }
        }
        assert!(found > 1000);
    }
}

/// Matrix row "Transfer": a tome transferred onto itself votes each cell for itself with all its
/// samples; onto a tome of a smaller domain, cells outside it take the nearest voted cell.
#[test]
fn transfer_maps_cells_through_their_samples() {
    let (mesh, targets) = scene();
    let p = Params {
        threads: 4,
        ..Params::default()
    };
    let new = super::bake(&lever_input(&mesh, &targets, p.clone()), None, None).expect("bake");
    let r = &new.report;
    let same = tenebrae_tome::locate::transfer(&new.tome, &r.cell_points, &r.cell_samples);
    assert_eq!(same.len(), r.global_cells);
    for (g, v) in same.iter().enumerate() {
        assert_eq!(v.votes, vec![(g as u32, r.cell_samples[g].len() as u32)]);
        assert_eq!(v.nearest, None);
    }
    // The old tome covers only x < 6: cells beyond it get no vote and a nearest voted cell.
    let mut small = lever_input(&mesh, &targets, p);
    small.view_volume.max[0] = 6.0;
    let old = super::bake(&small, None, None).expect("bake");
    let moved = tenebrae_tome::locate::transfer(&old.tome, &r.cell_points, &r.cell_samples);
    let unvoted: Vec<usize> = (0..moved.len())
        .filter(|&g| moved[g].votes.is_empty())
        .collect();
    assert!(!unvoted.is_empty());
    for (g, v) in moved.iter().enumerate() {
        let n: u32 = v.votes.iter().map(|(_, k)| k).sum();
        assert!(n as usize <= r.cell_samples[g].len());
        assert!(v.votes.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(
            v.votes
                .iter()
                .all(|(c, _)| (*c as usize) < old.tome.global_cells.len())
        );
        match v.nearest {
            Some(m) => {
                assert!(v.votes.is_empty());
                assert!(!moved[m as usize].votes.is_empty());
            }
            None => assert!(!v.votes.is_empty()),
        }
    }
    // Missing points: never a nearest cell, never chosen.
    let none = tenebrae_tome::locate::transfer(&old.tome, &[], &r.cell_samples);
    assert!(none.iter().all(|v| v.nearest.is_none()));
}

/// Overlay geometry: each global cell's box holds its samples; every portal is flat on its axis,
/// links two valid cells and lies within both cells' boxes (a grouped global cell's box is the
/// union of its tile cells, so a portal need not be on its face).
#[test]
fn cell_and_portal_geometry_dequantises() {
    let (mesh, targets) = scene();
    for p in [Params::default(), Params::levers_off()] {
        let p = Params { threads: 4, ..p };
        let b = super::bake(&lever_input(&mesh, &targets, p), None, None).expect("bake");
        let boxes = tenebrae_tome::locate::global_cell_boxes(&b.tome);
        assert_eq!(boxes.len(), b.report.global_cells);
        let d = &b.tome.header.bounds;
        let step = |a: usize| (d.max[a] - d.min[a]) / 65535.0 * 1.01;
        for (g, bx) in boxes.iter().enumerate() {
            for s in &b.report.cell_samples[g] {
                assert!((0..3).all(|a| bx.min[a] <= s[a] && s[a] <= bx.max[a]));
            }
        }
        let portals = tenebrae_tome::locate::global_portals(&b.tome);
        assert_eq!(portals.len(), b.report.global_portals);
        assert!(!portals.is_empty());
        for pr in &portals {
            let a = pr.axis as usize;
            assert_eq!(pr.rect.min[a], pr.rect.max[a]);
            let to = pr
                .to
                .expect("baked global portals are never boundary portals")
                as usize;
            for c in [pr.from as usize, to] {
                let bx = &boxes[c];
                let within = (0..3).all(|k| {
                    bx.min[k] - step(k) <= pr.rect.min[k] && pr.rect.max[k] <= bx.max[k] + step(k)
                });
                assert!(within, "portal {pr:?} outside cell {c} {bx:?}");
            }
        }
    }
}

/// `global::simplify` (Umbra's `tree_simplify_r` with solid as don't-care): identical children
/// collapse to one, a solid half is dropped, and lifting an axis the subtree splits on everywhere
/// is kept when it leaves fewer nodes.
#[test]
fn simplify_collapses_and_rotates() {
    use grid::SOLID;
    use tile::Node::{Leaf, Split};
    // z: [x: 0 | solid] over [x: 0 | solid] becomes x: 0 | solid, then 0.
    let mut t = vec![
        Split {
            axis: 2,
            at: 4.0,
            kids: [1, 2],
        },
        Split {
            axis: 0,
            at: 2.0,
            kids: [3, 4],
        },
        Split {
            axis: 0,
            at: 2.0,
            kids: [5, 6],
        },
        Leaf(0),
        Leaf(SOLID),
        Leaf(0),
        Leaf(SOLID),
    ];
    global::simplify(&mut t);
    assert_eq!(t, vec![Leaf(0)]);
    // z: [x: 1 | 2] over [x: 1 | 3]: lifting x gives x: 1 | [z: 2 | 3], five nodes for seven.
    let mut t = vec![
        Split {
            axis: 2,
            at: 4.0,
            kids: [1, 2],
        },
        Split {
            axis: 0,
            at: 2.0,
            kids: [3, 4],
        },
        Split {
            axis: 0,
            at: 2.0,
            kids: [5, 6],
        },
        Leaf(1),
        Leaf(2),
        Leaf(1),
        Leaf(3),
    ];
    global::simplify(&mut t);
    assert_eq!(
        t,
        vec![
            Split {
                axis: 0,
                at: 2.0,
                kids: [1, 2]
            },
            Leaf(1),
            Split {
                axis: 2,
                at: 4.0,
                kids: [3, 4]
            },
            Leaf(2),
            Leaf(3),
        ]
    );
    // A solid half whose other side splits on the same axis stays: its midpoints would move.
    let mut t = vec![
        Split {
            axis: 0,
            at: 4.0,
            kids: [1, 2],
        },
        Leaf(SOLID),
        Split {
            axis: 0,
            at: 6.0,
            kids: [3, 4],
        },
        Leaf(1),
        Leaf(2),
    ];
    let before = t.clone();
    global::simplify(&mut t);
    assert_eq!(t, before);
}

/// `grid::coalesce` (Umbra's `rect_coalesce`): an L of two rectangles stays two rectangles while
/// the solid corner its bounding rectangle would add costs more than a rectangle, becomes the
/// bounding rectangle once it does not, and without a cost is always the bounding rectangle.
#[test]
fn coalesce_trades_area_for_rectangles() {
    let l = [[0, 10, 0, 2], [0, 2, 2, 10]];
    let mut two = grid::coalesce(&l, Some(10.0));
    two.sort_unstable();
    assert_eq!(two.len(), 2);
    assert_eq!(
        two.iter()
            .map(|r| (r[1] - r[0]) * (r[3] - r[2]))
            .sum::<u32>(),
        36
    );
    assert_eq!(grid::coalesce(&l, Some(100.0)), vec![[0, 10, 0, 10]]);
    assert_eq!(grid::coalesce(&l, None), vec![[0, 10, 0, 10]]);
    // Over 128 pieces are binned first; the cover still covers every unit square of them.
    let pieces: Vec<[u32; 4]> = (0..200).map(|i| [i, i + 1, 0, 1 + i % 3]).collect();
    let cover = grid::coalesce(&pieces, Some(1.0));
    assert!(cover.len() <= 4);
    for p in &pieces {
        for y in p[2]..p[3] {
            assert!(
                cover
                    .iter()
                    .any(|c| c[0] <= p[0] && p[1] <= c[1] && c[2] <= y && y < c[3])
            );
        }
    }
}

/// `tile::level_shift`: the distance rule of `Params::coarsen_outside`, its table at SO 6, SH
/// 0.125 and 32 m tiles, the pad, several volumes and the cap on the tile's voxel count.
#[test]
fn level_shift_grows_with_the_distance_to_the_nearest_volume() {
    use super::tile::level_shift;
    let view = aabb([0.0, 0.0, 0.0], [32.0, 32.0, 32.0]);
    // A 32 m tile `gap` metres past the volume on x, and `side` metres beside it on y.
    let tile = |gap: f32, side: f32| {
        aabb(
            [32.0 + gap, 32.0 + side, 0.0],
            [64.0 + gap, 64.0 + side, 32.0],
        )
    };
    let k = |gap: f32| level_shift(&[view], &tile(gap, -32.0), 6.0, 0.125, 256);
    assert_eq!(k(-8.0), 0, "inside");
    assert_eq!(k(0.0), 0, "touching");
    assert_eq!(k(9.5), 0, "within pad 9.6");
    for (gap, want) in [
        (9.7, 1),
        (71.9, 1),
        (72.0, 2),
        (119.9, 2),
        (120.0, 3),
        (215.9, 3),
        (216.0, 4),
        (407.9, 4),
        (408.0, 5),
        (791.9, 5),
        (792.0, 6),
    ] {
        assert_eq!(k(gap), want, "d = {gap}");
    }
    // Euclidean: 60 m on x and 60 m on y is 84.9 m.
    assert_eq!(level_shift(&[view], &tile(60.0, 60.0), 6.0, 0.125, 256), 2);
    // The nearer of two volumes counts.
    let near = aabb([140.0, 0.0, 0.0], [150.0, 32.0, 32.0]);
    assert_eq!(
        level_shift(&[view, near], &tile(300.0, -32.0), 6.0, 0.125, 256),
        3
    );
    assert_eq!(
        level_shift(&[near, view], &tile(300.0, -32.0), 6.0, 0.125, 256),
        3
    );
    // The pad is at least two voxels.
    assert_eq!(level_shift(&[view], &tile(3.5, -32.0), 1.0, 2.0, 16), 0);
    assert_eq!(level_shift(&[view], &tile(4.5, -32.0), 1.0, 2.0, 16), 1);
    // A whole number of coarse voxels per tile: 6 voxels allow k 1 only, 4 allow 2.
    assert_eq!(level_shift(&[view], &tile(500.0, -32.0), 6.0, 0.125, 6), 1);
    assert_eq!(level_shift(&[view], &tile(500.0, -32.0), 6.0, 0.125, 4), 2);
    assert_eq!(level_shift(&[view], &tile(500.0, -32.0), 6.0, 0.125, 5), 0);
    assert_eq!(
        level_shift(&[view], &tile(500.0, -32.0), 0.0, 0.125, 256),
        0,
        "L2 off"
    );
}

/// `grid::Layer::upsample`: a coarse layer upsampled equals the layer built position by position
/// at the finer resolution.
#[test]
fn layers_upsample_to_the_fine_face() {
    use super::grid::Layer;
    let row = 5usize;
    let value = |x: usize, y: usize| ((x * 7 + y * 3) % 4 / 2 + usize::from(x == y)) as u32;
    let mut coarse = Layer::default();
    for y in 0..row {
        for x in 0..row {
            coarse.push(value(x, y), 1);
        }
    }
    for s in [1usize, 2, 3, 4] {
        let mut fine = Layer::default();
        for y in 0..row * s {
            for x in 0..row * s {
                fine.push(value(x / s, y / s), 1);
            }
        }
        let up = coarse.clone().upsample(row, s);
        assert_eq!((up.runs, up.len), (fine.runs, fine.len), "s = {s}");
    }
    // A uniform layer stays one run.
    let mut one = Layer::default();
    one.push(9, 16);
    let up = one.upsample(4, 2);
    assert_eq!((up.runs, up.len), (vec![(0, 9)], 64));
}

/// The shared scene stretched along +x: a floor, pillars and a sealed room 30 to 120 m past the
/// view volume, and a target at the far end that `domain_margin` takes in, so most tiles lie
/// away from the volume. 8 m tiles at 0.5 m voxels (16 voxels: level shifts up to 4).
fn far_scene() -> (Mesh, Vec<Target<'static>>) {
    let (mut m, mut targets) = scene();
    cube(&mut m, [30.0, -20.0, -1.0], [118.0, 20.0, 0.0]);
    for x in [50.0, 66.0, 81.0, 97.0] {
        cube(&mut m, [x, -3.0, 0.0], [x + 2.5, 3.5, 12.0]);
    }
    let (lo, hi) = ([100.0, 5.0, 0.0], [110.0, 15.0, 6.0]);
    cube(&mut m, lo, [hi[0], hi[1], lo[2] + 0.5]);
    cube(&mut m, [lo[0], lo[1], hi[2] - 0.5], hi);
    cube(&mut m, lo, [lo[0] + 0.5, hi[1], hi[2]]);
    cube(&mut m, [hi[0] - 0.5, lo[1], lo[2]], hi);
    cube(&mut m, lo, [hi[0], lo[1] + 0.5, hi[2]]);
    cube(&mut m, [lo[0], hi[1] - 0.5, lo[2]], hi);
    targets.push(target([116.0, -1.0, 1.0], [118.0, 1.0, 3.0], 21));
    targets.push(target([60.0, 8.0, 1.0], [61.0, 9.0, 2.0], 23));
    (m, targets)
}

fn far_input<'a>(mesh: &'a Mesh, targets: &'a [Target], p: Params) -> Input<'a> {
    lever_input(
        mesh,
        targets,
        Params {
            domain_margin: 100.0,
            keep_every_target: true,
            ..p
        },
    )
}

/// Voxels the tile passes bake (each tile at its own resolution) and tiles above level shift 0.
fn baked_voxels(input: &Input<'_>) -> (usize, usize) {
    let prep = prepare(input, &mut Progressor::none()).expect("prepare");
    let ctx = &prep.ctx;
    let n = ctx.tile_ranges.len();
    let voxels = (0..n).map(|t| ctx.space(t).layout.tile().pow(3)).sum();
    (voxels, (0..n).filter(|&t| ctx.level_shift(t) > 0).count())
}

/// Cells the tile passes build, summed over tiles (before the global stage removes or merges
/// any).
fn tile_stage_cells(input: &Input<'_>) -> usize {
    let prep = prepare(input, &mut Progressor::none()).expect("prepare");
    let ctx = &prep.ctx;
    let (floods, live) = flood(&prep);
    (0..ctx.tile_ranges.len())
        .map(|t| {
            let (surface, solid, peak) = tile::solid_pass(ctx, t, &live, &floods);
            let overlap = tile::overlaps(ctx, t);
            tile::cell_pass(ctx, t, surface.count(), solid, peak, &overlap)
                .cells
                .len()
        })
        .sum()
}

/// `Params::coarsen_outside` on a margin bake: tiles away from the volume are baked coarse (fewer
/// voxels, fewer cells), both tomes round-trip, every in-volume point resolves in both to a cell
/// of the same tile, every seed in the volume finds a cell, and the bake does not depend on the
/// thread count or on a warm cache.
#[test]
fn coarse_tiles_outside_the_view_volume() {
    let (mesh, targets) = far_scene();
    // 2 m seed blocks, so an 8 m tile holds several cells.
    let p = |coarsen_outside: bool, threads: usize| Params {
        coarsen_outside,
        threads,
        cells: CellMode::Components { block: 2.0 },
        ..Params::default()
    };
    let (on_in, off_in) = (
        far_input(&mesh, &targets, p(true, 4)),
        far_input(&mesh, &targets, p(false, 4)),
    );
    let (on_voxels, coarse) = baked_voxels(&on_in);
    let (off_voxels, none) = baked_voxels(&off_in);
    assert_eq!(none, 0);
    assert!(
        coarse > 0 && on_voxels < off_voxels,
        "{coarse} {on_voxels} {off_voxels}"
    );
    let t0 = std::time::Instant::now();
    let (on, on_r) = bake(&on_in).expect("bake");
    let t_on = t0.elapsed();
    let t0 = std::time::Instant::now();
    let (off, off_r) = bake(&off_in).expect("bake");
    let t_off = t0.elapsed();
    eprintln!(
        "coarse: {on_voxels} voxels, {} cells, {t_on:?}; fine: {off_voxels} voxels, {} cells, {t_off:?}",
        on_r.cells, off_r.cells
    );
    let (on_cells, off_cells) = (tile_stage_cells(&on_in), tile_stage_cells(&off_in));
    assert!(on_cells < off_cells, "{on_cells} {off_cells}");
    assert!(on_r.cells <= off_r.cells, "{} {}", on_r.cells, off_r.cells);
    assert_eq!(on.header.bounds, off.header.bounds);
    for tome in [&on, &off] {
        assert_valid(tome);
        let bytes = tenebrae_tome::write(tome);
        assert_eq!(
            tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read")),
            bytes
        );
    }
    // Each leaf stores its own voxel in its level: 0.5 m · 2^k, `k` the least level shift of
    // the grid tiles it covers (0 throughout without coarsening).
    let shifts = |input: &Input<'_>| {
        let prep = prepare(input, &mut Progressor::none()).expect("prepare");
        let ctx = &prep.ctx;
        (0..ctx.tile_ranges.len())
            .map(|t| {
                let f = tile::tile_frame(&ctx.grid, 8.0, &ctx.tile_ranges[t]);
                (f, ctx.level_shift(t))
            })
            .collect::<Vec<_>>()
    };
    let mut coarse_leaves = [0usize; 2];
    for (i, (tome, input)) in [(&on, &on_in), (&off, &off_in)].into_iter().enumerate() {
        let grid = shifts(input);
        for t in tome.tiles.iter().flatten().filter(|t| t.view.is_some()) {
            let k = grid
                .iter()
                .filter(|(f, _)| {
                    (0..3).all(|a| t.bounds.min[a] <= f.min[a] && f.max[a] <= t.bounds.max[a])
                })
                .map(|(_, k)| *k)
                .min()
                .expect("covers a grid tile");
            let want = super::tile_level(0.5 * (1u32 << k) as f32, 8.0, &t.bounds);
            assert_eq!(t.level.to_bits(), want.to_bits(), "{:?}", t.bounds);
            coarse_leaves[i] += usize::from(k > 0);
        }
    }
    assert!(
        coarse_leaves[0] > 0 && coarse_leaves[1] == 0,
        "{coarse_leaves:?}"
    );
    // Every in-volume sample of the fine bake resolves in both tomes, to cells of the same tile.
    let vv = on_in.view_volume;
    let tile_of = |tome: &Tome| -> Vec<Aabb> {
        let mut out = vec![aabb([0.0; 3], [0.0; 3]); tome.global_cells.len()];
        for t in tome.tiles.iter().flatten().filter(|t| t.view.is_some()) {
            for c in &t.cells {
                out[c.0[4] as usize] = t.bounds;
            }
        }
        out
    };
    let (on_tiles, off_tiles) = (tile_of(&on), tile_of(&off));
    let mut checked = 0;
    for p in off_r.cell_samples.iter().flatten() {
        if (0..3).any(|a| p[a] <= vv.min[a] || p[a] >= vv.max[a]) {
            continue;
        }
        let (a, b) = (
            tenebrae_tome::locate::find_cell(&on, *p),
            tenebrae_tome::locate::find_cell(&off, *p),
        );
        let (Some(a), Some(b)) = (a, b) else {
            panic!("in-volume point {p:?}: {a:?} {b:?}");
        };
        assert_eq!(on_tiles[a as usize], off_tiles[b as usize], "{p:?}");
        checked += 1;
    }
    assert!(checked > 0);
    assert_samples_find_their_cells(&on, &on_r, 0.5, &vv);
    for s in &SEEDS {
        assert!(
            tenebrae_tome::locate::find_cell(&on, *s).is_some(),
            "seed {s:?}"
        );
    }
    // Thread count and cache.
    let one = super::bake(&far_input(&mesh, &targets, p(true, 1)), None, None).expect("bake");
    let four = super::bake(&on_in, None, None).expect("bake");
    // Everything but `grid_bytes`, which counts the `threads` largest tiles.
    let key = |b: &Baked| {
        (
            tenebrae_tome::write(&b.tome),
            b.report.cell_samples.clone(),
            b.report.cell_points.clone(),
            b.report.voxels,
            b.object_map.clone(),
        )
    };
    assert!(key(&one) == key(&four), "threads change the coarse bake");
    let cache = first_cache(&on_in);
    let (report, _) = rebake(&on_in, &cache.to_bytes());
    assert_eq!(report.cache_hits, cache.entries());
}

/// With margin 0 and one view volume every tile touches it, so `coarsen_outside` changes nothing
/// (with several volumes, tiles in the gap between them would coarsen).
#[test]
fn coarsening_leaves_margin_zero_bakes_unchanged() {
    let (mesh, targets) = scene();
    let tome = |coarsen_outside: bool| {
        let p = Params {
            threads: 4,
            coarsen_outside,
            ..Params::default()
        };
        let input = lever_input(&mesh, &targets, p);
        assert_eq!(baked_voxels(&input).1, 0);
        tenebrae_tome::write(&bake(&input).expect("bake").0)
    };
    assert!(tome(true) == tome(false));
}

/// A seed in a coarse tile seeds the coarse voxel it lies in.
#[test]
fn a_seed_in_a_coarse_tile_seeds_its_coarse_voxel() {
    let (mesh, targets) = far_scene();
    let seeds = [SEEDS[0], SEEDS[1], SEEDS[2], [105.3, 9.7, 3.2]];
    let mut input = far_input(&mesh, &targets, Params::default());
    input.seeds = &seeds;
    let prep = prepare(&input, &mut Progressor::none()).expect("prepare");
    let ctx = &prep.ctx;
    let g = &ctx.grid;
    let v = [0, 1, 2].map(|a| ((seeds[3][a] - g.origin[a]) / g.voxel).floor() as usize);
    let t = (0..ctx.tile_ranges.len())
        .find(|&t| {
            let lo = ctx.lo(t);
            (0..3).all(|a| lo[a] <= v[a] && v[a] < lo[a] + ctx.layout.tile())
        })
        .expect("tile");
    let sp = ctx.space(t);
    assert!(sp.s > 1, "the room's tile is coarse");
    let size = sp.layout.tile();
    let l = [0, 1, 2].map(|a| (v[a] - ctx.lo(t)[a]) / sp.s);
    assert_eq!(
        tile::starts(ctx, t).0,
        vec![(l[2] * size + l[1]) * size + l[0]]
    );
    // The sealed room's open component is seeded.
    assert!(!tile::flood_pass(ctx, t).seeded.is_empty());
    bake(&input).expect("bake");
}

/// Several view volumes (with a margin, so the domain matches a bake without the second): the
/// domain covers both, the level shift comes from the nearer, the mask
/// and reachability keep both; an empty or invalid extra volume is refused.
#[test]
fn several_view_volumes() {
    let (mesh, targets) = far_scene();
    let second = aabb([96.0, -10.0, 0.0], [112.0, 10.0, 10.0]);
    let p = Params {
        threads: 4,
        extra_view_volumes: vec![second],
        // L1's open region tiles hold one cell over the region whatever the mask, so they would
        // resolve the second volume's points in the bake without it too.
        empty_leaves: false,
        ..Params::default()
    };
    let mut input = far_input(&mesh, &targets, p);
    let seeds = [SEEDS[0], SEEDS[1], SEEDS[2], [92.0, 0.0, 3.0]];
    input.seeds = &seeds;
    let prep = prepare(&input, &mut Progressor::none()).expect("prepare");
    let ctx = &prep.ctx;
    assert!(ctx.domain.max[0] >= 112.0 && ctx.domain.min[0] <= -30.0);
    // A tile between the volumes is coarse; tiles at either volume are not.
    let at = |x: f32| {
        (0..ctx.tile_ranges.len())
            .find(|&t| {
                let f = tile::tile_frame(&ctx.grid, 8.0, &ctx.tile_ranges[t]);
                f.min[0] <= x
                    && x < f.max[0]
                    && f.min[1] <= 1.0
                    && 1.0 < f.max[1]
                    && f.min[2] <= 1.0
                    && 1.0 < f.max[2]
            })
            .expect("tile")
    };
    assert_eq!(ctx.level_shift(at(100.0)), 0);
    assert_eq!(ctx.level_shift(at(0.0)), 0);
    assert!(ctx.level_shift(at(64.0)) > 0);
    let (tome, report) = bake(&input).expect("bake");
    assert_valid(&tome);
    // The same domain (the margin reaches the far target either way) without the second volume.
    let mut alone = far_input(
        &mesh,
        &targets,
        Params {
            empty_leaves: false,
            ..Params::default()
        },
    );
    alone.seeds = &seeds;
    let (single, single_report) = bake(&alone).expect("bake");
    assert_eq!(single.header.bounds, tome.header.bounds);
    // Open points of the second volume resolve only with it (the mask keeps it); the first
    // volume's resolve either way.
    let first = [-10.0, 0.0, 3.0];
    let second_points = [[104.0, -6.0, 3.0], [100.5, 0.5, 3.0]];
    assert!(tenebrae_tome::locate::find_cell(&tome, first).is_some());
    assert!(tenebrae_tome::locate::find_cell(&single, first).is_some());
    for p in second_points {
        assert!(
            tenebrae_tome::locate::find_cell(&tome, p).is_some(),
            "{p:?}"
        );
        assert!(
            tenebrae_tome::locate::find_cell(&single, p).is_none(),
            "{p:?}"
        );
    }
    // Reachability keeps cells inside the second volume only with it.
    let far_cells = |r: &Report| {
        r.cell_samples
            .iter()
            .filter(|s| {
                s.iter()
                    .any(|p| (0..3).all(|a| second.min[a] < p[a] && p[a] < second.max[a]))
            })
            .count()
    };
    assert!(far_cells(&report) > 0);
    assert_eq!(far_cells(&single_report), 0);
    assert!(
        report
            .cell_points
            .iter()
            .any(|p| p.is_some_and(|p| p[0] > 96.0))
    );
    for bad in [
        aabb([0.0, 0.0, 0.0], [0.0, 1.0, 1.0]),
        aabb([0.0, 0.0, f32::NAN], [1.0, 1.0, 1.0]),
        aabb([1.0, 0.0, 0.0], [0.0, 1.0, 1.0]),
    ] {
        let p = Params {
            extra_view_volumes: vec![second, bad],
            ..Params::default()
        };
        assert!(matches!(
            super::bake(&lever_input(&mesh, &targets, p), None, None),
            Err(Error::Params(_))
        ));
    }
}

/// `Params::domain_margin`: the domain grows to take in a target past the view volume (up to the
/// margin), which is then listed in the cells around it instead of in every cell with a boundary
/// portal on that side; a negative margin is refused.
#[test]
fn domain_margin_takes_in_targets() {
    let (mesh, targets) = scene();
    let mut targets = targets.to_vec();
    targets.push(target([0.0, 0.0, 40.0], [1.0, 1.0, 41.0], 99));
    let far = targets.len() as u32 - 1;
    let listed = |t: &Tome| {
        t.tiles
            .iter()
            .flatten()
            .flat_map(|tile| tile.cells.iter())
            .filter(|c| {
                let (start, count) = (c.0[2] as usize, c.0[3]);
                let mut left = count;
                t.object_runs.entries[start..].iter().any(|r| {
                    if left == 0 {
                        return false;
                    }
                    left = left.saturating_sub(r.run);
                    (r.index..r.index + r.run).contains(&far)
                })
            })
            .count()
    };
    let bake_with = |margin: f32| {
        let p = Params {
            threads: 4,
            domain_margin: margin,
            keep_every_target: true,
            ..Params::default()
        };
        bake(&lever_input(&mesh, &targets, p)).expect("bake").0
    };
    let (narrow, wide) = (bake_with(0.0), bake_with(16.0));
    assert!(narrow.header.bounds.max[2] < 40.0);
    assert!(wide.header.bounds.max[2] >= 41.0);
    // Broadcast to boundary cells, or listed in the cells around it: listed either way.
    assert!(listed(&narrow) > 0 && listed(&wide) > 0);
    let p = Params {
        domain_margin: -1.0,
        ..Params::default()
    };
    assert!(matches!(
        super::bake(&lever_input(&mesh, &targets, p), None, None),
        Err(Error::Params(_))
    ));
}

/// `Params::share_lists`: every cell's object list decodes the same with shared windows as
/// without, and the stream is no longer.
#[test]
fn shared_object_lists_decode_unchanged() {
    let (mesh, targets) = scene();
    let decode = |t: &Tome| -> Vec<Vec<u32>> {
        t.tiles
            .iter()
            .flatten()
            .flat_map(|tile| tile.cells.iter())
            .map(|c| {
                let mut left = c.0[3];
                let mut out = Vec::new();
                for r in &t.object_runs.entries[c.0[2] as usize..] {
                    if left == 0 {
                        break;
                    }
                    assert!(r.run <= left, "a window ends inside a run");
                    out.extend(r.index..r.index + r.run);
                    left -= r.run;
                }
                assert_eq!(left, 0);
                out
            })
            .collect()
    };
    let bake_with = |share_lists: bool| {
        let p = Params {
            threads: 4,
            share_lists,
            ..Params::default()
        };
        bake(&lever_input(&mesh, &targets, p)).expect("bake").0
    };
    let (shared, plain) = (bake_with(true), bake_with(false));
    assert_eq!(decode(&shared), decode(&plain));
    assert!(shared.object_runs.entries.len() <= plain.object_runs.entries.len());
    let bytes = tenebrae_tome::write(&shared);
    assert_eq!(
        tenebrae_tome::write(&tenebrae_tome::read(&bytes).expect("read")),
        bytes
    );
}

/// `Voxels` of a coarse tile answer for the coarse voxel covering a grid voxel: the closed pillar
/// at x 50 is solid, open air is not, and grid voxels under one coarse voxel agree.
#[test]
fn voxelize_answers_coarse_voxels() {
    let (mesh, targets) = far_scene();
    let input = far_input(&mesh, &targets, Params::default());
    let vox = voxelize(&input).expect("voxelize");
    let at = |p: Vec3| [0, 1, 2].map(|a| ((p[a] - vox.origin[a]) / vox.voxel).floor() as usize);
    let prep = prepare(&input, &mut Progressor::none()).expect("prepare");
    let v = at([51.2, 0.2, 6.2]);
    let n = vox.tiles;
    let t = prep.ctx.tile_at[((v[2] / 16) * n[1] + v[1] / 16) * n[0] + v[0] / 16];
    let s = prep.ctx.space(t).s;
    assert!(s > 1, "the pillar's tile is coarse");
    assert!(vox.solid(v), "inside the closed pillar");
    let open = at([60.2, 10.2, 20.2]);
    assert!(!vox.solid(open) && !vox.surface(open));
    let base = v.map(|k| k / s * s);
    for d in 0..s * s * s {
        let w = [base[0] + d % s, base[1] + d / s % s, base[2] + d / (s * s)];
        assert_eq!(
            (vox.solid(w), vox.surface(w)),
            (vox.solid(v), vox.surface(v))
        );
    }
}

/// A cached re-bake across `coarsen_outside` (both ways) equals a fresh bake: tiles near the
/// volume hit, coarse tiles miss.
#[test]
fn cache_across_coarsening_matches_a_fresh_bake() {
    let (mesh, targets) = far_scene();
    let input = |coarsen_outside: bool| {
        far_input(
            &mesh,
            &targets,
            Params {
                threads: 4,
                coarsen_outside,
                ..Params::default()
            },
        )
    };
    for (from, to) in [(false, true), (true, false)] {
        let cache = first_cache(&input(from));
        let n = cache.entries();
        let (report, _) = rebake(&input(to), &cache.to_bytes());
        let hits = report.cache_hits;
        assert!(hits[1] > 0 && hits[1] < n[1], "{hits:?} of {n:?}");
    }
}

/// An extra view volume elsewhere in an unchanged domain leaves the keys of tiles it does not
/// reach alone: they still hit, and the re-bake equals a fresh one.
#[test]
fn a_far_extra_volume_keeps_hits_elsewhere() {
    let (mesh, targets) = far_scene();
    let base = far_input(&mesh, &targets, cache_params());
    let cache = first_cache(&base);
    let n = cache.entries();
    let extra = far_input(
        &mesh,
        &targets,
        Params {
            extra_view_volumes: vec![aabb([96.0, -10.0, 0.0], [112.0, 10.0, 10.0])],
            ..cache_params()
        },
    );
    let (report, _) = rebake(&extra, &cache.to_bytes());
    let hits = report.cache_hits;
    assert!(hits[1] > n[1] / 2 && hits[1] < n[1], "{hits:?} of {n:?}");
}

/// Two coarse sibling leaves collapse past `collapse_size` (up to `tile · 2^min(k)`); without
/// coarsening no leaf grows past it.
#[test]
fn coarse_leaves_collapse_past_the_collapse_size() {
    let (mesh, targets) = far_scene();
    let largest = |coarsen_outside: bool| {
        let p = Params {
            threads: 4,
            coarsen_outside,
            collapse_size: 8.0,
            empty_leaves: false,
            ..Params::default()
        };
        let (tome, _) = bake(&far_input(&mesh, &targets, p)).expect("bake");
        tome.tiles
            .iter()
            .flatten()
            .filter(|t| t.view.is_some())
            .map(|t| {
                (0..3)
                    .map(|a| t.bounds.max[a] - t.bounds.min[a])
                    .fold(0.0f32, f32::max)
            })
            .fold(0.0f32, f32::max)
    };
    assert!(largest(true) > 8.0);
    assert_eq!(largest(false), 8.0);
}

/// `tile::layer_reaches_live`, the seam read of `flood_solid`: a coarse face voxel reaches live
/// space when any one of the `s²` grid positions under it on a neighbour's layer is live, not
/// only the first, and a blocked `+` face of the neighbour below closes it.
#[test]
fn a_coarse_face_voxel_reads_every_fine_position() {
    use super::grid::Layer;
    use super::tile::{LABEL, layer_reaches_live};
    let (fine, s) = (8usize, 2usize);
    // Label 0 is live, 1 dead; one position carries 0 (with the blocked bit when `blocked`).
    let layer = |live_at: (usize, usize), blocked: bool| {
        let mut l = Layer::default();
        for y in 0..fine {
            for x in 0..fine {
                let v = if (x, y) == live_at {
                    u32::from(blocked) << 31
                } else {
                    1
                };
                l.push(v, 1);
            }
        }
        l
    };
    let live = [true, false];
    let reaches = |l: &Layer, below: bool| layer_reaches_live(l, &live, [1, 2], s, fine, below);
    // Face voxel (1, 2) covers positions x 2..4, y 4..6.
    for at in [(2, 4), (3, 4), (2, 5), (3, 5)] {
        assert!(reaches(&layer(at, false), false), "{at:?}");
        assert!(reaches(&layer(at, false), true), "{at:?}");
        assert!(!reaches(&layer(at, true), true), "{at:?}");
    }
    for at in [(1, 4), (4, 4), (2, 3), (2, 6)] {
        assert!(!reaches(&layer(at, false), false), "{at:?}");
    }
    let mut surface = Layer::default();
    surface.push(LABEL, fine * fine);
    assert!(!reaches(&surface, false));
}
