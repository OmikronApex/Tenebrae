//! Mean counters and timing of the native runtime's portal query over `TBQC` cameras:
//! `cargo run --release -p tenebrae-bench --example rtcount -- TOME CAMS.bin`.

use std::time::Instant;

use tenebrae::runtime::{Camera, Options, Query, Runtime};

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("tome");
    let tome = tenebrae::read(&bytes).expect("valid tome");
    let cb = std::fs::read(&a[2]).expect("cameras");
    let n = u32_at(&cb, 8) as usize;
    let t0 = Instant::now();
    let rt = Runtime::new(&tome).expect("decode");
    println!("decode {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    let mut q = Query::new(&rt);
    let mut sums = [0f64; 7];
    let mut stage = [0f64; 4];
    let mut rej = [0f64; 6];
    let mut times = Vec::with_capacity(n);
    let mut reached = 0f64;
    for i in 0..n {
        let at = 12 + 80 * i;
        let mut m = [[0.0f32; 4]; 4];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = f32_at(&cb, at + 16 * r + 4 * c);
            }
        }
        let p = [
            f32_at(&cb, at + 64),
            f32_at(&cb, at + 68),
            f32_at(&cb, at + 72),
        ];
        let opts = Options {
            radius: f32_at(&cb, at + 76),
            ..Options::default()
        };
        let camera = Camera::from_world_to_clip_rows(m, p);
        let mut best = f64::INFINITY;
        for _ in 0..5 {
            let t = Instant::now();
            let _ = q.visible(&rt, &camera, &opts);
            best = best.min(t.elapsed().as_secs_f64());
        }
        times.push(best * 1e6);
        let c = q.counters();
        for (s, v) in sums.iter_mut().zip([
            c.tiles,
            c.cells,
            c.portals,
            c.rasterised,
            c.candidates,
            c.output,
            c.raster_rows,
        ]) {
            *s += f64::from(v);
        }
        for (s, v) in stage.iter_mut().zip(c.stage_ns) {
            *s += f64::from(v);
        }
        for (s, v) in rej.iter_mut().zip(c.rejected) {
            *s += f64::from(v);
        }
        reached += q.reached_cells().len() as f64;
    }
    println!(
        "mean rejected per query: boundary/gate {:.0}, no target {:.0}, off screen {:.0}, not fresh {:.0}, no source pixel {:.0}, target covered {:.0}",
        rej[0] / n as f64,
        rej[1] / n as f64,
        rej[2] / n as f64,
        rej[3] / n as f64,
        rej[4] / n as f64,
        rej[5] / n as f64
    );
    println!(
        "mean stage us (last run of each camera): select {:.1}, start {:.1}, propagate {:.1}, output {:.1}",
        stage[0] / n as f64 / 1e3,
        stage[1] / n as f64 / 1e3,
        stage[2] / n as f64 / 1e3,
        stage[3] / n as f64 / 1e3
    );
    times.sort_by(f64::total_cmp);
    let k = n as f64;
    println!(
        "{n} cameras: median best-of-5 {:.1} us; mean per query: tiles {:.1}, cell visits {:.1}, cells reached {:.1}, portals {:.1}, rasterised {:.1}, candidates {:.1}, output {:.1}, raster rows {:.0}",
        times[n / 2],
        sums[0] / k,
        sums[1] / k,
        reached / k,
        sums[2] / k,
        sums[3] / k,
        sums[4] / k,
        sums[5] / k,
        sums[6] / k
    );
}
