//! `probe TOME [step]`: per height, how many points of a lattice over the tome's domain find a cell
//! (`query::Locator::find_cell`). Shows where a tome's view trees resolve cells, e.g. that space
//! outside the view volume is "don't care" with `Params::view_mask`, in Tenebrae's tomes and
//! Umbra's alike.
use tenebrae::query::Locator;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let Some(path) = a.get(1) else {
        eprintln!("usage: probe TOME [step]");
        std::process::exit(2);
    };
    let tome = tenebrae::tome::read(&std::fs::read(path).expect("tome")).expect("valid tome");
    let step: f32 = a.get(2).map_or(1.0, |s| s.parse().expect("step"));
    let b = tome.header.bounds;
    let locator = Locator::new(&tome);
    // Offsets keep the lattice off voxel and tile planes.
    let mut z = b.min[2] + 0.013;
    while z < b.max[2] {
        let (mut hit, mut n) = (0, 0);
        let mut x = b.min[0] + 0.37;
        while x < b.max[0] {
            let mut y = b.min[1] + 0.21;
            while y < b.max[1] {
                n += 1;
                hit += usize::from(locator.find_cell([x, y, z]).is_some());
                y += step;
            }
            x += step;
        }
        println!("z {z:8.2}: {hit}/{n}");
        z += step.max(1.0) * 4.0;
    }
}
