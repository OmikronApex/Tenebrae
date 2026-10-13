//! Baking a scene with Tenebrae and measuring the result.

use std::collections::BTreeSet;

use tenebrae::bake::{Baked, Error, Input, Occluders, Params, bake};
use tenebrae::query::Locator;
use tenebrae::runtime::{Options, Query, Runtime, Status};
use tenebrae::tome::Tome;

use crate::Scene;
use crate::truth::{self, Visible};

/// Bakes `scene` with `params` (soup occluders, or the instanced form with `instanced`).
pub fn bake_scene(scene: &Scene, params: Params, instanced: bool) -> Result<Baked, Error> {
    let users = scene.users();
    let targets = scene.bake_targets(&users);
    let occluders = match (&scene.instanced, instanced) {
        (Some((meshes, instances)), true) => Occluders::Instanced { meshes, instances },
        _ => Occluders::Soup(&scene.mesh),
    };
    bake(
        &Input {
            occluders,
            targets: &targets,
            view_volume: scene.view_volume,
            seeds: &scene.seeds,
            params,
        },
        None,
        None,
    )
}

/// Tile sizes the benchmark bakes every scene at, in metres: the culling resolution. 32 m is the stock tomes'
/// (the validated parity setting); the others measure how both bakers scale with it.
pub const TILES: [u32; 4] = [8, 16, 32, 64];

/// The stock tomes' tile size.
pub const STOCK_TILE: u32 = 32;

/// A benchmark case: `scene.tT`, the row name in the tables.
pub fn case(scene: &str, tile: u32) -> String {
    format!("{scene}.t{tile}")
}

/// The benchmark's bake setting at `tile` metres: the stock tome A setting (the validated parity setting at
/// 32 m) with the tile set to `tile` and the leaf-tile collapse size to `tile`, at least 16 m.
/// `collapse = tile` is the stock relation (Umbra's `T` is its cluster size `CS`, 32 = 32), which
/// the Umbra reference mirrors by baking with `CS = tile`. The 16 m floor first stood in for Umbra's
/// inner-node LOD tiles; with Tenebrae's own (lever L5) it stays because it is faster and smaller
/// (see [`Params::collapse_size`]).
pub fn params(threads: usize, tile: u32) -> Params {
    Params {
        threads,
        tile: tile as f32,
        collapse_size: tile.max(16) as f32,
        ..Params::tome_a()
    }
}

/// Deterministic numbers of one bake, as `(key, value)` in table order.
pub fn metrics(scene: &Scene, baked: &Baked) -> Vec<(&'static str, i64)> {
    let bytes = tenebrae::write(&baked.tome).len();
    let r = &baked.report;
    let max_tile_cells = baked
        .tome
        .tiles
        .iter()
        .flatten()
        .map(|t| t.cells.len())
        .max()
        .unwrap_or(0);
    let locator = Locator::new(&baked.tome);
    let eyes_in_cells = scene
        .cameras
        .iter()
        .filter(|c| locator.find_cell(c.eye).is_some())
        .count();
    vec![
        ("bytes", bytes as i64),
        ("tiles", r.tiles as i64),
        ("cells", r.cells as i64),
        ("global_cells", r.global_cells as i64),
        ("portals", r.portals as i64),
        ("global_portals", r.global_portals as i64),
        ("objects", r.objects as i64),
        ("buried_targets", r.buried_targets as i64),
        ("sealed_targets", r.sealed_targets as i64),
        ("max_tile_cells", max_tile_cells as i64),
        ("eyes_in_cells", eyes_in_cells as i64),
    ]
}

/// The visibility keys every case has, in the reference's order; a `status_N` key follows for
/// each other status that occurs.
pub const VISIBILITY_KEYS: [&str; 9] = [
    "status_ok",
    "visible",
    "kept",
    "false_culls",
    "false_views",
    "frustum",
    "visible_hole",
    "false_culls_hole",
    "false_views_hole",
];

/// The table key counting queries that returned `status`, numbered as Umbra's runtime numbers
/// them (2 budget, 3 bad camera, 5 no cell).
fn status_key(status: Status) -> &'static str {
    match status {
        Status::Budget => "status_2",
        Status::BadCamera => "status_3",
        Status::NoCell => "status_5",
    }
}

/// Per tome object, its user ids (`+0x4C` / `+0x50`): the benchmark's target indices.
fn object_users(tome: &Tome) -> Vec<Vec<u32>> {
    let ids = &tome.users.ids;
    match &tome.users.prefix {
        None => ids.iter().map(|&u| vec![u]).collect(),
        Some(prefix) => prefix
            .windows(2)
            .map(|w| {
                ids.get(w[0] as usize..w[1] as usize)
                    .unwrap_or(&[])
                    .to_vec()
            })
            .collect(),
    }
}

/// Visibility of the bake from the scene's cameras, scored against the ray-cast truth `truth`
/// (one per camera) exactly as the Umbra reference was scored in Umbra's runtime, with
/// Tenebrae's runtime (`tenebrae::runtime`): portal query at radius 0, objects mapped to targets
/// by their user ids.
///
/// Keys (sums over the cameras): `status_ok` (queries with status 0; the others are not scored,
/// as the engine falls back to frustum culling) and `status_N` per other status, `visible` /
/// `kept` / `false_culls` / `false_views` (strict truth: truly visible, returned, truly visible
/// but not returned, cameras with a false cull), the same against the truth through a hole
/// (`*_hole`), and `frustum` (targets the frustum-only query returns, every camera).
pub fn visibility(scene: &Scene, baked: &Baked, truth: &[Visible]) -> Vec<(&'static str, i64)> {
    let users = object_users(&baked.tome);
    let targets = |objs: &[u32]| -> BTreeSet<u32> {
        objs.iter()
            .flat_map(|&o| users.get(o as usize).into_iter().flatten().copied())
            .collect()
    };
    let rt = Runtime::new(&baked.tome).expect("a baked tome decodes");
    let mut q = Query::new(&rt);
    let opts = Options::default();
    let mut sums: Vec<(&'static str, i64)> = VISIBILITY_KEYS.iter().map(|&k| (k, 0)).collect();
    let add = |sums: &mut Vec<(&'static str, i64)>, key: &'static str, v: i64| match sums
        .iter_mut()
        .find(|(k, _)| *k == key)
    {
        Some(e) => e.1 += v,
        None => sums.push((key, v)),
    };
    for (cam, t) in scene.cameras.iter().zip(truth) {
        let c = truth::camera(cam);
        let frustum = match q.frustum(&rt, &c) {
            Ok(list) => targets(list).len() as i64,
            Err(_) => 0,
        };
        add(&mut sums, "frustum", frustum);
        let got = match q.visible(&rt, &c, &opts) {
            Ok(list) => targets(list),
            Err(status) => {
                add(&mut sums, status_key(status), 1);
                continue;
            }
        };
        add(&mut sums, "status_ok", 1);
        for (list, visible, culls, views) in [
            (&t.strict, "visible", "false_culls", "false_views"),
            (
                &t.hole,
                "visible_hole",
                "false_culls_hole",
                "false_views_hole",
            ),
        ] {
            let miss = list.iter().filter(|u| !got.contains(u)).count() as i64;
            add(&mut sums, visible, list.len() as i64);
            add(&mut sums, culls, miss);
            add(&mut sums, views, i64::from(miss > 0));
        }
        add(&mut sums, "kept", got.len() as i64);
    }
    sums
}
