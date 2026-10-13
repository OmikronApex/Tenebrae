//! `edit IN OUT OPERATION` — experimental tome edits through the typed model.
//!
//! Operations:
//!   strip-inner   drop every inner-node tile (coarse LOD tiles); their node distance becomes -1.
//!   strip-global-portals  empty the global portal graph (global cells keep their boxes).
//!   graft-users DONOR  take DONOR's object user lists (same objects, same order, checked).
//!   identity      read and write unchanged (round-trip check on one file).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: edit IN OUT strip-inner|strip-global-portals|graft-users DONOR|identity");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[1]).unwrap_or_else(|e| {
        eprintln!("{}: {e}", args[1]);
        std::process::exit(2)
    });
    let mut tome = tenebrae::read(&bytes).unwrap_or_else(|e| {
        eprintln!("{}: {e}", args[1]);
        std::process::exit(1)
    });
    match args[3].as_str() {
        "strip-inner" => {
            let mut stripped = 0;
            for (node, code) in tome.top.codes.iter().enumerate() {
                if *code != 3 && tome.tiles[node].take().is_some() {
                    tome.node_distances[node] = -1.0;
                    stripped += 1;
                }
            }
            eprintln!("stripped {stripped} inner tiles");
        }
        "strip-global-portals" => {
            tome.global_portals.clear();
            for c in &mut tome.global_cells {
                c.portal_start = 0;
                c.portal_count = 0;
            }
        }
        "graft-users" => {
            // Same objects in the same order as DONOR (a bake whose targets are DONOR's objects):
            // take DONOR's user lists, which name the visibility record rows of each object.
            let donor =
                std::fs::read(args.get(4).expect("graft-users needs DONOR")).expect("donor");
            let donor = tenebrae::read(&donor).expect("donor tome");
            assert_eq!(
                donor.objects.len(),
                tome.objects.len(),
                "object counts differ"
            );
            let moved = donor
                .objects
                .iter()
                .zip(&tome.objects)
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(moved, 0, "object boxes differ: not the same objects");
            tome.users = donor.users;
        }
        "identity" => {}
        other => {
            eprintln!("unknown operation {other}");
            std::process::exit(2);
        }
    }
    std::fs::write(&args[2], tenebrae::write(&tome)).unwrap_or_else(|e| {
        eprintln!("{}: {e}", args[2]);
        std::process::exit(2)
    });
}
