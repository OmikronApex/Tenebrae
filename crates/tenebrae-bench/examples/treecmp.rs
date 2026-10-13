//! `treecmp TOME [--tiles]`: view-tree statistics of a tome (Tenebrae's or Umbra's): leaf-tile
//! cells, view-tree nodes and leaves, and leaves at most one 0.125 m voxel thin (open, and
//! solid). `--tiles` prints one line per leaf tile, by its minimum corner, for comparing two
//! tomes tile by tile.
use tenebrae::tome::{Aabb, Tome};

/// The smallest extent a leaf counts as one voxel thin at (0.125 m voxels).
const THIN: f32 = 0.13;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let Some(path) = a.get(1) else {
        eprintln!("usage: treecmp TOME [--tiles]");
        std::process::exit(2);
    };
    let per_tile = a.iter().any(|x| x == "--tiles");
    let tome: Tome = tenebrae::tome::read(&std::fs::read(path).expect("tome")).expect("valid tome");
    // cells, nodes, leaves, thin open leaves, thin solid leaves
    let mut total = [0usize; 5];
    for t in tome.tiles.iter().flatten() {
        let Some(v) = &t.view else { continue };
        let codes = &v.tree.codes;
        // Level order: children are appended in order, so the next free slot is a counter.
        let mut boxes = vec![t.bounds; codes.len()];
        let (mut next, mut leaf, mut split) = (1usize, 0usize, 0usize);
        let mut row = [t.cells.len(), codes.len(), 0, 0, 0];
        for (i, &code) in codes.iter().enumerate() {
            let b = boxes[i];
            if code == 3 {
                let solid = v.leaf_values[leaf] == (1u32 << v.leaf_bits) - 1;
                leaf += 1;
                row[2] += 1;
                let extent = (0..3).map(|k| b.max[k] - b.min[k]).fold(f32::MAX, f32::min);
                if extent <= THIN {
                    row[if solid { 4 } else { 3 }] += 1;
                }
                continue;
            }
            let axis = code as usize;
            let at = v
                .tree
                .splits
                .get(split)
                .copied()
                .unwrap_or((b.min[axis] + b.max[axis]) * 0.5);
            split += 1;
            let (mut lo, mut hi): (Aabb, Aabb) = (b, b);
            lo.max[axis] = at;
            hi.min[axis] = at;
            boxes[next] = lo;
            boxes[next + 1] = hi;
            next += 2;
        }
        if per_tile {
            println!(
                "{:?} cells {} nodes {} leaves {} thin-open {} thin-solid {}",
                t.bounds.min, row[0], row[1], row[2], row[3], row[4]
            );
        }
        for (s, r) in total.iter_mut().zip(row) {
            *s += r;
        }
    }
    println!(
        "TOTAL cells {} nodes {} leaves {} thin-open {} thin-solid {}",
        total[0], total[1], total[2], total[3], total[4]
    );
}
