//! `sizes SCENE TILE [key=value ...]`: bakes one benchmark case at the benchmark setting and
//! prints its tome bytes, bake time, cells, portals, view-tree nodes, object-run entries and
//! global cells. Keys override the setting: `block` and `fill` (`CellMode::Merged`), `comp`
//! (`CellMode::Components` with that block), `merge` (`cell_merge`), `collapse`
//! (`collapse_size`), `margin` (`domain_margin`); `vis=1` also scores visibility against the
//! ray-cast truth. `TEN_OUT=FILE` writes the tome.
use tenebrae::bake::CellMode;
use tenebrae_bench::{run, scenes, truth};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let scene = scenes::by_name(&a[1]).unwrap();
    let tile: u32 = a[2].parse().unwrap();
    let mut p = run::params(16, tile);
    let mut vis = false;
    for kv in &a[3..] {
        let (k, v) = kv.split_once('=').unwrap();
        let x: f32 = v.parse().unwrap();
        match (k, &mut p.cells) {
            ("block", CellMode::Merged { block, .. }) => *block = x,
            ("fill", CellMode::Merged { fill, .. }) => *fill = x,
            ("merge", _) => p.cell_merge = x,
            ("comp", _) => p.cells = CellMode::Components { block: x },
            ("collapse", _) => p.collapse_size = x,
            ("margin", _) => p.domain_margin = x,
            ("vis", _) => vis = x != 0.0,
            _ => panic!("{kv}"),
        }
    }
    let t0 = std::time::Instant::now();
    let baked = run::bake_scene(&scene, p, false).unwrap();
    let secs = t0.elapsed().as_secs_f32();
    let tome = &baked.tome;
    let bytes = tenebrae::write(tome).len();
    if let Ok(out) = std::env::var("TEN_OUT") {
        std::fs::write(out, tenebrae::write(tome)).unwrap();
    }
    let (mut nodes, mut cells, mut portals) = (0, 0, 0);
    for t in tome.tiles.iter().flatten() {
        cells += t.cells.len();
        portals += t.portals.len();
        if let Some(v) = &t.view {
            nodes += v.tree.codes.len();
        }
    }
    print!(
        "{}.t{tile} {bytes} B ({secs:.1} s): cells {cells}, portals {portals}, view nodes {nodes}, object runs {}, global cells {}",
        a[1],
        tome.object_runs.entries.len(),
        tome.global_cells.len(),
    );
    if vis {
        let t = truth::visible(&scene, 16);
        let v = run::visibility(&scene, &baked, &t);
        let get = |k: &str| v.iter().find(|(n, _)| *n == k).map_or(-1, |x| x.1);
        print!(
            " | kept {} false_culls {} hole {} status_ok {}",
            get("kept"),
            get("false_culls"),
            get("false_culls_hole"),
            get("status_ok")
        );
    }
    println!();
}
