//! `bench list | export DIR [SCENE...] | baseline [FILE] | report [SCENE...] | tbsc FILE`
//!
//! * `list`: every scene with its hash and counts.
//! * `export DIR`: per scene, `DIR/<scene>/` with `scene.bin`, `targets.bin`, `seeds.bin`,
//!   `cameras.bin`, `scene.txt` (formats in `tenebrae_bench::scene`), `truth.bin`
//!   (`tenebrae_bench::truth::bytes`) and per tile size T of `run::TILES` `tenebrae.tT.tome`, the
//!   Tenebrae bake at the benchmark setting (the stock tome A setting at tile T: object `i` is target `i`).
//!   An external Umbra harness reads this directory to produce `reference/umbra.txt`.
//! * `baseline [FILE]`: bakes every case (scene × tile size), scores its visibility with
//!   Tenebrae's runtime against the ray-cast truth (`run::visibility`), and writes
//!   Tenebrae's baseline table (default: the crate's `baseline.txt`).
//! * `report`: bakes and scores the cases, prints each metric against the baseline and the Umbra
//!   reference (`reference/umbra.txt`), then the results table of `README.md` (Tenebrae from
//!   its own query, Umbra from the reference).
//!
//! * `tbsc FILE`: decodes a TBSC asset and prints its counts and bounds.
//!
//! `TENEBRAE_BENCH_THREADS` sets the worker count (default: all cores).

use std::path::{Path, PathBuf};
use std::time::Instant;

use tenebrae_bench::table::Table;
use tenebrae_bench::{GENERATOR, Scene, run, scenes, truth};

fn threads() -> usize {
    std::env::var("TENEBRAE_BENCH_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

fn crate_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn pick(names: &[String]) -> Result<Vec<Scene>, String> {
    if names.is_empty() {
        return Ok(scenes::all());
    }
    names
        .iter()
        .map(|n| scenes::by_name(n).ok_or_else(|| format!("unknown scene {n}")))
        .collect()
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn export(dir: &Path, names: &[String]) -> Result<(), String> {
    for scene in pick(names)? {
        let out = dir.join(scene.name);
        std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        write(&out.join("scene.bin"), &scene.scene_bytes())?;
        write(&out.join("targets.bin"), &scene.targets_bytes())?;
        write(&out.join("seeds.bin"), &scene.seeds_bytes())?;
        write(&out.join("cameras.bin"), &scene.cameras_bytes())?;
        write(&out.join("scene.txt"), scene.manifest().as_bytes())?;
        let t0 = Instant::now();
        let vis = truth::visible(&scene, threads());
        let t_truth = t0.elapsed().as_secs_f32();
        write(&out.join("truth.bin"), &truth::bytes(&vis))?;
        let per_view = |f: fn(&truth::Visible) -> usize| {
            vis.iter().map(f).sum::<usize>() as f32 / vis.len().max(1) as f32
        };
        eprintln!(
            "{:<11} {:016x}  truth {:.1} s ({:.1} visible per view, {:.1} through a hole)",
            scene.name,
            scene.hash(),
            t_truth,
            per_view(|v| v.strict.len()),
            per_view(|v| v.hole.len()),
        );
        for tile in run::TILES {
            let t1 = Instant::now();
            let baked = run::bake_scene(&scene, run::params(threads(), tile), false)
                .map_err(|e| format!("{}: bake failed: {e}", run::case(scene.name, tile)))?;
            let tome = tenebrae::write(&baked.tome);
            write(&out.join(format!("tenebrae.t{tile}.tome")), &tome)?;
            eprintln!(
                "  t{tile:<3} bake {:.1} s, {} B",
                t1.elapsed().as_secs_f32(),
                tome.len()
            );
        }
    }
    Ok(())
}

/// Bakes every scene, scores each bake's visibility, and returns the table of their metrics.
fn measure(scenes: &[Scene]) -> Result<Table, String> {
    let mut t = Table {
        generator: Some(GENERATOR),
        ..Table::default()
    };
    for scene in scenes {
        let t0 = Instant::now();
        let vis = truth::visible(scene, threads());
        eprintln!(
            "{:<15} truth in {:.1} s",
            scene.name,
            t0.elapsed().as_secs_f32()
        );
        for tile in run::TILES {
            let case = run::case(scene.name, tile);
            let t0 = Instant::now();
            let baked = run::bake_scene(scene, run::params(threads(), tile), false)
                .map_err(|e| format!("{case}: bake failed: {e}"))?;
            let t_bake = t0.elapsed().as_secs_f32();
            let t1 = Instant::now();
            t.set(&case, "hash", format!("{:016x}", scene.hash()));
            for (k, v) in run::metrics(scene, &baked) {
                t.set(&case, k, v);
            }
            for (k, v) in run::visibility(scene, &baked, &vis) {
                t.set(&case, k, v);
            }
            eprintln!(
                "{case:<15} baked in {t_bake:.1} s, scored in {:.1} s",
                t1.elapsed().as_secs_f32()
            );
        }
    }
    Ok(t)
}

/// `n` with thousands separators.
fn thousands(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The results table of the crate's `README.md`: Tenebrae from `now`, Umbra from `umbra`.
fn results_table(cases: &[String], now: &Table, umbra: &Table) -> String {
    let mut out = String::from(
        "| Case | Umbra B | Tenebrae B | Umbra kept | Tenebrae kept | Frustum | Visible \
         | Tenebrae query fails | False culls U / T | Through a hole U / T |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    let cell = |t: &Table, case: &str, k: &str| t.int(case, k).map_or("-".to_owned(), thousands);
    for case in cases {
        let fails = now
            .rows
            .iter()
            .filter(|((s, k), _)| s == case && k.starts_with("status_") && k != "status_ok")
            .filter_map(|(_, v)| v.parse::<i64>().ok())
            .sum::<i64>();
        out.push_str(&format!(
            "| {case} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} / {} |\n",
            cell(umbra, case, "bytes"),
            cell(now, case, "bytes"),
            cell(umbra, case, "kept"),
            cell(now, case, "kept"),
            cell(umbra, case, "frustum"),
            cell(umbra, case, "visible"),
            thousands(fails),
            cell(umbra, case, "false_culls"),
            cell(now, case, "false_culls"),
            cell(umbra, case, "false_culls_hole"),
            cell(now, case, "false_culls_hole"),
        ));
    }
    out
}

/// Every case of `scenes`, in table order.
fn cases(scenes: &[Scene]) -> Vec<String> {
    scenes
        .iter()
        .flat_map(|s| run::TILES.map(|t| run::case(s.name, t)))
        .collect()
}

fn read_table(path: &Path) -> Result<Option<Table>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Table::parse(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn report(names: &[String]) -> Result<(), String> {
    let scenes = pick(names)?;
    let now = measure(&scenes)?;
    let base = read_table(&crate_file("baseline.txt"))?.unwrap_or_default();
    let umbra = read_table(&crate_file("reference/umbra.txt"))?.unwrap_or_default();
    println!(
        "{:<15} {:<15} {:>10} {:>10} {:>8} {:>10}",
        "case", "metric", "now", "baseline", "delta", "umbra"
    );
    for case in cases(&scenes) {
        let case = case.as_str();
        let hash = now.get(case, "hash").unwrap_or_default();
        let stale = |t: &Table| t.get(case, "hash").is_some_and(|h| h != hash);
        if stale(&base) {
            println!("{case:<15} baseline is stale (scene hash changed)");
        }
        if stale(&umbra) {
            println!("{case:<15} Umbra reference is stale (scene hash changed)");
        }
        let keys: Vec<String> = now
            .rows
            .keys()
            .filter(|(s, k)| s == case && k != "hash")
            .map(|(_, k)| k.clone())
            .collect();
        for key in keys {
            let v = now.int(case, &key).unwrap_or(0);
            let b = base.int(case, &key);
            let delta = b.map_or(String::new(), |b| {
                if b == 0 {
                    format!("{:+}", v - b)
                } else {
                    format!("{:+.1}%", 100.0 * (v - b) as f64 / b as f64)
                }
            });
            let u = umbra
                .int(case, &key)
                .map_or(String::new(), |u| u.to_string());
            println!(
                "{:<15} {:<15} {:>10} {:>10} {:>8} {:>10}",
                case,
                key,
                v,
                b.map_or(String::new(), |b| b.to_string()),
                delta,
                u
            );
        }
    }
    println!();
    print!("{}", results_table(&cases(&scenes), &now, &umbra));
    Ok(())
}

fn inspect_tbsc(path: &Path) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let t0 = Instant::now();
    let d = tenebrae_bench::tbsc::decode(&data)?;
    let occluding = d.targets.iter().filter(|t| t.owner.is_some()).count();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in &d.mesh.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    println!(
        "{} B, decoded in {:.2} s: {} objects ({occluding} occluding), {} vertices, {} triangles, bounds {lo:?}..{hi:?}",
        data.len(),
        t0.elapsed().as_secs_f32(),
        d.targets.len(),
        d.mesh.vertices.len(),
        d.mesh.triangles.len()
    );
    Ok(())
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let result = match a.first().map(String::as_str) {
        Some("list") => {
            for s in scenes::all() {
                println!(
                    "{:<11} {:016x}  {:>7} tris {:>4} targets {:>3} seeds  {}",
                    s.name,
                    s.hash(),
                    s.mesh.triangles.len(),
                    s.targets.len(),
                    s.seeds.len(),
                    s.about
                );
            }
            Ok(())
        }
        Some("export") if a.len() >= 2 => export(Path::new(&a[1]), &a[2..]),
        Some("baseline") => {
            let path = a
                .get(1)
                .map_or_else(|| crate_file("baseline.txt"), PathBuf::from);
            let all = scenes::all();
            let order = cases(&all);
            let order: Vec<&str> = order.iter().map(String::as_str).collect();
            measure(&all).and_then(|t| {
                let text = t.render(
                    "Tenebrae benchmark baseline: `cargo run --release -p tenebrae-bench -- baseline`.\n\
                     Metrics of the bake at the benchmark setting (Params::tome_a); see crates/tenebrae-bench/README.md.",
                    &order,
                );
                write(&path, text.as_bytes())
            })
        }
        Some("report") => report(&a[1..]),
        Some("tbsc") if a.len() == 2 => inspect_tbsc(Path::new(&a[1])),
        _ => Err(
            "usage: bench list | export DIR [SCENE...] | baseline [FILE] | report [SCENE...] | tbsc FILE"
                .into(),
        ),
    };
    if let Err(e) = result {
        eprintln!("bench: {e}");
        std::process::exit(2);
    }
}
