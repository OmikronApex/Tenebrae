//! Benchmark gates: every generated scene baked at the benchmark setting (the stock tome A setting) at every
//! tile size of `run::TILES` (a case, `scene.tT`).
//!
//! Run in release: `cargo test --release -p tenebrae-bench`. Each scene is one test, so a failure
//! names its scene, and its message names the case. The gates need no game data and no Umbra
//! runtime:
//!
//! * the scene still hashes to what `baseline.txt` and `reference/umbra.txt` were measured on;
//! * the bake succeeds and its tome round-trips byte for byte;
//! * at the stock 32 m: the bake is the same at two thread counts and with a cache (an unchanged
//!   re-bake hits every tile), and the instanced scene bakes to the same bytes as its soup;
//! * object `i` is target `i`, every camera's eye finds a cell, and no tile holds as many cells as
//!   the runtime fails at (measured at 16 and 32 m);
//! * the tome is at most 10 % larger than the baseline;
//! * visibility, scored with Tenebrae's runtime against the ray-cast truth
//!   (`run::visibility`): every camera's portal query returns status 0, the misses of the truth
//!   through a hole (`false_culls_hole`, the errors) do not grow over the baseline, the objects
//!   kept are at most 10 % more than the baseline's, and the frustum query keeps exactly the
//!   baseline's targets (it depends only on target boxes and cameras). A case without baseline
//!   visibility keys skips the last three.
//!
//! A deliberate change is accepted by rewriting the baseline
//! (`cargo run --release -p tenebrae-bench -- baseline`).

use std::path::Path;

use tenebrae::bake::{Input, Occluders, bake};
use tenebrae_bench::table::Table;
use tenebrae_bench::truth::{self, Visible};
use tenebrae_bench::{GENERATOR, Scene, run, scenes};

/// Cells per tile at which the runtime's portal culler fails, where measured
/// (`Params::max_cells_per_tile`): 1,024 on 32 m tiles, 256 on 16 m tiles.
fn tile_cell_limit(tile: u32) -> Option<i64> {
    match tile {
        16 => Some(256),
        32 => Some(1024),
        _ => None,
    }
}

/// Largest tome growth over the baseline before the gate fails; also the largest growth of the
/// objects kept.
const SIZE_SLACK: f64 = 1.10;

fn table(name: &str) -> Table {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Table::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The case is recorded in `t` for this scene's hash and generator, or a failure saying how to
/// refresh the stale table.
fn check_current(
    scene: &Scene,
    case: &str,
    file: &str,
    t: &Table,
    fix: &str,
) -> Result<(), String> {
    let hash = format!("{:016x}", scene.hash());
    match (t.generator, t.get(case, "hash")) {
        (Some(g), Some(h)) if g == GENERATOR && h == hash => Ok(()),
        (g, h) => Err(format!(
            "{file} is stale (generator {g:?}, hash {h:?}; now generator {GENERATOR}, hash {hash}): {fix}"
        )),
    }
}

fn check(name: &str) {
    let scene = scenes::by_name(name).expect("scene");
    let base = table("baseline.txt");
    let umbra = table("reference/umbra.txt");
    let vis = truth::visible(&scene, threads());
    let mut fail = Vec::new();
    for tile in run::TILES {
        let case = run::case(name, tile);
        fail.extend(
            check_case(&scene, tile, &case, &base, &umbra, &vis)
                .into_iter()
                .map(|e| format!("{case}: {e}")),
        );
    }
    assert!(fail.is_empty(), "\n  {}", fail.join("\n  "));
}

/// The gates of one case; the failures. At the stock tile size every gate runs; at the other sizes the
/// thread, cache and instanced comparisons are skipped (no tile-size-specific path, and they
/// would triple the run time).
fn check_case(
    scene: &Scene,
    tile: u32,
    case: &str,
    base: &Table,
    umbra: &Table,
    vis: &[Visible],
) -> Vec<String> {
    let mut fail = Vec::new();
    for (file, t, fix) in [
        (
            "baseline.txt",
            base,
            "cargo run --release -p tenebrae-bench -- baseline",
        ),
        (
            "reference/umbra.txt",
            umbra,
            "re-export and regenerate the Umbra reference (crates/tenebrae-bench/README.md)",
        ),
    ] {
        if let Err(e) = check_current(scene, case, file, t, fix) {
            fail.push(e);
        }
    }

    let n = threads();
    let baked = match run::bake_scene(scene, run::params(n, tile), false) {
        Ok(b) => b,
        Err(e) => {
            fail.push(format!("bake failed: {e}"));
            return fail;
        }
    };
    let bytes = tenebrae::write(&baked.tome);

    // Round trip.
    match tenebrae::read(&bytes) {
        Ok(back) if tenebrae::write(&back) == bytes => {}
        Ok(_) => fail.push("tome does not round-trip byte for byte".into()),
        Err(e) => fail.push(format!("tome does not read back: {e:?}")),
    }

    if tile == run::STOCK_TILE {
        // Thread counts.
        let other = if n == 4 { 3 } else { 4 };
        let again = run::bake_scene(scene, run::params(other, tile), false).expect("bake");
        if tenebrae::write(&again.tome) != bytes || again.object_map != baked.object_map {
            fail.push(format!("bake differs between {n} and {other} threads"));
        }

        // Cache: a cached bake, then an unchanged re-bake from its cache.
        let users = scene.users();
        let targets = scene.bake_targets(&users);
        let input = Input {
            occluders: Occluders::Soup(&scene.mesh),
            targets: &targets,
            view_volume: scene.view_volume,
            seeds: &scene.seeds,
            params: run::params(n, tile),
        };
        let first = bake(&input, Some(&[]), None).expect("cached bake");
        let second = bake(&input, first.cache.as_deref(), None).expect("re-bake");
        if tenebrae::write(&first.tome) != bytes || tenebrae::write(&second.tome) != bytes {
            fail.push("a cached bake or re-bake differs from the plain bake".into());
        }
        if second.report.cache_misses != [0, 0] {
            fail.push(format!(
                "unchanged re-bake missed tiles: {:?}",
                second.report.cache_misses
            ));
        }

        // Instanced input.
        if scene.instanced.is_some() {
            let inst = run::bake_scene(scene, run::params(n, tile), true).expect("instanced bake");
            if tenebrae::write(&inst.tome) != bytes {
                fail.push("instanced input bakes differently from its soup".into());
            }
        }
    }

    // Object numbering, eyes, tile cells.
    let identity: Vec<Option<u32>> = (0..scene.targets.len() as u32).map(Some).collect();
    if baked.object_map != identity {
        fail.push("object map is not the identity under keep_every_target".into());
    }
    let metrics = run::metrics(scene, &baked);
    let m = |k: &str| {
        metrics
            .iter()
            .find(|(key, _)| *key == k)
            .map(|&(_, v)| v)
            .expect("metric")
    };
    if m("eyes_in_cells") != scene.cameras.len() as i64 {
        fail.push(format!(
            "{} of {} camera eyes find no cell",
            scene.cameras.len() as i64 - m("eyes_in_cells"),
            scene.cameras.len()
        ));
    }
    if let Some(limit) = tile_cell_limit(tile)
        && m("max_tile_cells") >= limit
    {
        fail.push(format!(
            "a tile holds {} cells (runtime limit {limit})",
            m("max_tile_cells")
        ));
    }

    // Size against the baseline.
    if let Some(b) = base.int(case, "bytes")
        && m("bytes") as f64 > b as f64 * SIZE_SLACK
    {
        fail.push(format!(
            "tome is {} B, over the baseline {b} B by more than {:.0} %",
            m("bytes"),
            (SIZE_SLACK - 1.0) * 100.0
        ));
    }

    // Visibility.
    let visibility = run::visibility(scene, &baked, vis);
    let v = |k: &str| {
        visibility
            .iter()
            .find(|(key, _)| *key == k)
            .map_or(0, |&(_, v)| v)
    };
    if v("status_ok") != scene.cameras.len() as i64 {
        let failed: Vec<String> = visibility
            .iter()
            .filter(|(k, _)| k.starts_with("status_") && *k != "status_ok")
            .map(|(k, n)| format!("{k} {n}"))
            .collect();
        fail.push(format!(
            "{} of {} portal queries fail ({})",
            scene.cameras.len() as i64 - v("status_ok"),
            scene.cameras.len(),
            failed.join(", ")
        ));
    }
    if let Some(b) = base.int(case, "false_culls_hole")
        && v("false_culls_hole") > b
    {
        fail.push(format!(
            "{} false culls through a hole, over the baseline's {b}",
            v("false_culls_hole")
        ));
    }
    if let Some(b) = base.int(case, "frustum")
        && v("frustum") != b
    {
        // Depends only on the target boxes and the cameras: any change is a query regression.
        fail.push(format!(
            "the frustum query keeps {} targets, the baseline {b}",
            v("frustum")
        ));
    }
    if let Some(b) = base.int(case, "kept")
        && v("kept") as f64 > b as f64 * SIZE_SLACK
    {
        fail.push(format!(
            "keeps {} objects, over the baseline {b} by more than {:.0} %",
            v("kept"),
            (SIZE_SLACK - 1.0) * 100.0
        ));
    }

    // The case's numbers, for the log (`--nocapture`).
    let row = |t: &Table, k: &str| t.int(case, k).map_or("-".to_owned(), |v| v.to_string());
    eprintln!("{case}: {}", scene.about);
    for (k, v) in metrics.iter().chain(&visibility) {
        eprintln!(
            "  {k:<15} {v:>9}  baseline {:>9}  umbra {:>9}",
            row(base, k),
            row(umbra, k)
        );
    }
    fail
}

macro_rules! scene_tests {
    ($($name:ident),* $(,)?) => {
        $(#[test] fn $name() { check(stringify!($name)); })*

        #[test]
        fn every_scene_has_a_test() {
            assert_eq!(&[$(stringify!($name)),*], &scenes::NAMES);
        }
    };
}

scene_tests!(
    rooms, city, thin_walls, terrain, maze, seams, instanced, degenerate, bistro, sponza
);
