//! `cells TOME OUT [x y z ... | @POINTS]` — per global cell, the boxes of the view-tree leaves that find it.
//!
//! OUT (little endian): u32 cell count, then per cell u32 box count and f32[6] (min, max) per box.
//! Checks every cell's largest box centre with `find_cell` and prints the agreement. Extra points
//! on the command line are looked up and printed.

use tenebrae::query::{Locator, cell_regions};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: cells TOME OUT [x y z ...]");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&a[1]).expect("tome");
    let tome = tenebrae::read(&bytes).unwrap_or_else(|e| {
        eprintln!("{}: {e}", a[1]);
        std::process::exit(1)
    });
    let regions = cell_regions(&tome);
    let locator = Locator::new(&tome);
    let mut out = Vec::new();
    out.extend_from_slice(&(regions.len() as u32).to_le_bytes());
    let (mut empty, mut agree) = (0usize, 0usize);
    for (c, list) in regions.iter().enumerate() {
        out.extend_from_slice(&(list.len() as u32).to_le_bytes());
        for b in list {
            for v in b.min.iter().chain(b.max.iter()) {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        let vol = |b: &tenebrae::tome::Aabb| (0..3).map(|k| b.max[k] - b.min[k]).product::<f32>();
        match list.iter().max_by(|x, y| vol(x).total_cmp(&vol(y))) {
            None => empty += 1,
            Some(b) => {
                let p = [0, 1, 2].map(|k| (b.min[k] + b.max[k]) * 0.5);
                agree += usize::from(locator.find_cell(p) == Some(c as u32));
            }
        }
    }
    std::fs::write(&a[2], out).expect("write");
    eprintln!(
        "cells {}: without a view leaf {empty}, largest-leaf centre finds its own cell {agree}",
        regions.len()
    );
    let mut points: Vec<[f32; 3]> = Vec::new();
    if let Some(file) = a.get(3).and_then(|s| s.strip_prefix('@')) {
        // @FILE: u32 count, then f32[3] per point.
        let b = std::fs::read(file).expect("points");
        let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        points.extend((0..n).map(|k| [f(4 + 12 * k), f(8 + 12 * k), f(12 + 12 * k)]));
    } else {
        for p in a[3..].chunks(3) {
            if let [x, y, z] = p {
                points.push([x, y, z].map(|v| v.parse::<f32>().expect("number")));
            }
        }
    }
    for q in points {
        println!("{q:?} -> {:?}", locator.find_cell(q));
    }
}
