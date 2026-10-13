//! `inspect TOME [--tiles] [--portals]` — decoded summary of a tome, for studying bakes.

use tenebrae::tome::{Aabb, Portal, Tome};

fn deq(q: u32, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * q as f32 / 65535.0
}

fn cell_box(words: [u32; 3], b: &Aabb) -> Aabb {
    let (w6, w7, w8) = (words[0], words[1], words[2]);
    let q_min = [w6 >> 16, w6 & 0xFFFF, w7 >> 16];
    let q_max = [w7 & 0xFFFF, w8 >> 16, w8 & 0xFFFF];
    let mut out = Aabb {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    for a in 0..3 {
        out.min[a] = deq(q_min[a], b.min[a], b.max[a]);
        out.max[a] = deq(q_max[a], b.min[a], b.max[a]);
    }
    out
}

fn portal(p: &Portal, tome_bounds: &Aabb) -> String {
    let d0 = p.0[0];
    let axis = (d0 >> 30) as usize;
    let side = (d0 >> 29) & 1;
    let boundary = (d0 >> 28) & 1;
    let gate = (d0 >> 27) & 1;
    let b26 = (d0 >> 26) & 1;
    let target = d0 & 0x3FF_FFFF;
    let coord = p.0[1] & 0xFFFF;
    let local = p.0[1] >> 16;
    let a1 = (axis + 1) % 3;
    let a2 = (axis + 2) % 3;
    let b = tome_bounds;
    format!(
        "axis {axis} side {side} bnd {boundary} gate {gate} b26 {b26} target {target} local {local} plane {:.2} rect a{a1} [{:.2},{:.2}] a{a2} [{:.2},{:.2}]",
        deq(coord, b.min[axis], b.max[axis]),
        deq(p.0[2] >> 16, b.min[a1], b.max[a1]),
        deq(p.0[2] & 0xFFFF, b.min[a1], b.max[a1]),
        deq(p.0[3] >> 16, b.min[a2], b.max[a2]),
        deq(p.0[3] & 0xFFFF, b.min[a2], b.max[a2]),
    )
}

fn fmt_box(b: &Aabb) -> String {
    format!(
        "({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2})",
        b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: inspect TOME [--tiles] [--portals]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(2)
    });
    let t: Tome = tenebrae::read(&bytes).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1)
    });
    let tiles_flag = args.iter().any(|a| a == "--tiles");
    let portals_flag = args.iter().any(|a| a == "--portals");
    let params = String::from_utf8_lossy(&t.header.params);
    println!("params {}", params.trim_end_matches('\0'));
    println!(
        "bounds {} base_distance {} top nodes {} objects {} users {} global cells {} global portals {}",
        fmt_box(&t.header.bounds),
        t.header.base_distance,
        t.top.codes.len(),
        t.objects.len(),
        t.users.ids.len(),
        t.global_cells.len(),
        t.global_portals.len()
    );
    println!(
        "object runs {} ({}+{} bits), cell runs {} ({}+{} bits), top codes {:?}, splits {:?}",
        t.object_runs.entries.len(),
        t.object_runs.index_bits,
        t.object_runs.run_bits,
        t.cell_runs.entries.len(),
        t.cell_runs.index_bits,
        t.cell_runs.run_bits,
        &t.top.codes[..t.top.codes.len().min(40)],
        &t.top.splits[..t.top.splits.len().min(12)]
    );
    // Rough byte estimate per structure (tile sections), for studying bake size.
    let (mut nodes, mut leaves, mut cells, mut portals, mut tiles) = (0usize, 0usize, 0, 0, 0);
    for tile in t.tiles.iter().flatten() {
        tiles += 1;
        cells += tile.cells.len();
        portals += tile.portals.len();
        if let Some(v) = &tile.view {
            nodes += v.tree.codes.len();
            leaves += v.leaf_values.len() * v.leaf_bits as usize / 8;
        }
    }
    println!(
        "tiles {tiles}: view nodes {nodes} (~{} KB keys+splits) leaf values ~{} KB, cells {cells} (~{} KB), portals {portals} (~{} KB), global portals ~{} KB, object runs ~{} KB",
        nodes * 5 / 1024,
        leaves / 1024,
        cells * 36 / 1024,
        portals * 16 / 1024,
        t.global_portals.len() * 16 / 1024,
        t.object_runs.entries.len() * (t.object_runs.index_bits + t.object_runs.run_bits) as usize
            / 8
            / 1024
    );
    for (i, c) in t
        .global_cells
        .iter()
        .enumerate()
        .take(if tiles_flag { 64 } else { 0 })
    {
        println!(
            "gcell {i} portals {}+{} box {}",
            c.portal_start,
            c.portal_count,
            fmt_box(&cell_box(c.bounds, &t.header.bounds))
        );
    }
    if portals_flag {
        for (i, p) in t.global_portals.iter().enumerate().take(64) {
            println!("gportal {i} {}", portal(p, &t.header.bounds));
        }
    }
    if !tiles_flag {
        return;
    }
    for (node, tile) in t.tiles.iter().enumerate() {
        let Some(tile) = tile else { continue };
        println!(
            "tile node {node} code {} dist {} box {} flags {} level {} hi52 {} cells {} portals {} view {} bsp {}",
            t.top.codes[node],
            t.node_distances[node],
            fmt_box(&tile.bounds),
            tile.flags,
            tile.level,
            tile.cell_word_high,
            tile.cells.len(),
            tile.portals.len(),
            tile.view.as_ref().map_or("-".into(), |v| format!(
                "{} nodes, {} leaves x{} bits {:?}",
                v.tree.codes.len(),
                v.leaf_values.len(),
                v.leaf_bits,
                &v.leaf_values[..v.leaf_values.len().min(16)]
            )),
            tile.bsp.is_some()
        );
        for (ci, c) in tile.cells.iter().enumerate() {
            let w = c.0;
            println!(
                "   cell {ci} portals {}+{} objects@{}x{} {} {} box {}",
                w[0],
                w[1],
                w[2],
                w[3],
                if w[5] == 0 { "global" } else { "runs" },
                if w[5] == 0 {
                    w[4].to_string()
                } else {
                    format!("{}x{}", w[4], w[5])
                },
                fmt_box(&cell_box([w[6], w[7], w[8]], &tile.bounds))
            );
        }
        if portals_flag {
            for (pi, p) in tile.portals.iter().enumerate() {
                println!("   portal {pi} {}", portal(p, &t.header.bounds));
            }
        }
    }
}
