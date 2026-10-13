//! Byte-identical round trip of real tomes: read, write, compare.
//!
//! Tomes are game data and never live in this repository. Point `TENEBRAE_TOMES` at one or more
//! directories (separated by `;`); every file whose first word is `0xD6000012` is tested. Without
//! the variable the test reports that it found nothing and passes.

use std::path::PathBuf;

fn tomes() -> Vec<PathBuf> {
    let Ok(dirs) = std::env::var("TENEBRAE_TOMES") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in dirs.split(';').filter(|d| !d.is_empty()) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(bytes) = std::fs::read(&path)
                && bytes.len() >= 4
                && bytes[..4] == [0x12, 0x00, 0x00, 0xD6]
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_tome_round_trips_byte_for_byte() {
    let paths = tomes();
    if paths.is_empty() {
        eprintln!("TENEBRAE_TOMES unset or no tomes found: nothing to check");
        return;
    }
    let mut failures = Vec::new();
    for path in &paths {
        let bytes = std::fs::read(path).expect("read tome");
        match tenebrae_tome::read(&bytes) {
            Err(e) => failures.push(format!("{}: read failed: {e}", path.display())),
            Ok(tome) => {
                let again = tenebrae_tome::write(&tome);
                if again != bytes {
                    let first = again
                        .iter()
                        .zip(&bytes)
                        .position(|(a, b)| a != b)
                        .unwrap_or(again.len().min(bytes.len()));
                    failures.push(format!(
                        "{}: differs at 0x{first:X} (written {} bytes, original {} bytes)",
                        path.display(),
                        again.len(),
                        bytes.len()
                    ));
                }
            }
        }
    }
    eprintln!("{} tomes checked, {} failed", paths.len(), failures.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The query surface on stock tomes: the prepared locator answers as `find_cell` on a lattice of
/// points through each domain, and every non-gate global portal turns into geometry inside the
/// domain that links valid cells. Prints each tome's settings string and counts.
#[test]
fn stock_tomes_answer_queries() {
    use tenebrae_tome::locate::{Locator, find_cell, global_cell_boxes, global_portals};
    let paths = tomes();
    if paths.is_empty() {
        eprintln!("TENEBRAE_TOMES unset or no tomes found: nothing to check");
        return;
    }
    for path in &paths {
        let tome = tenebrae_tome::read(&std::fs::read(path).expect("read tome")).expect("tome");
        let params = String::from_utf8_lossy(&tome.header.params);
        let (none, several) = tome.users.prefix.as_ref().map_or((0, 0), |p| {
            let n = |f: fn(u32) -> bool| p.windows(2).filter(|w| f(w[1] - w[0])).count();
            (n(|k| k == 0), n(|k| k > 1))
        });
        eprintln!(
            "{}: \"{}\" objects {} (no user {none}, several {several}), global cells {}, global portals {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            params.trim_end_matches('\0'),
            tome.objects.len(),
            tome.global_cells.len(),
            tome.global_portals.len()
        );
        let d = tome.header.bounds;
        let loc = Locator::new(&tome);
        let n = 24;
        let mut found = 0usize;
        for k in 0..n * n * n {
            let f = |i: usize, a: usize| {
                d.min[a] + (d.max[a] - d.min[a]) * (i as f32 + 0.37) / n as f32
            };
            let q = [f(k % n, 0), f(k / n % n, 1), f(k / (n * n), 2)];
            let want = find_cell(&tome, q);
            assert_eq!(loc.find_cell(q), want, "{} at {q:?}", path.display());
            found += usize::from(want.is_some());
        }
        assert!(found > 0, "{}: no lattice point in a cell", path.display());
        let boxes = global_cell_boxes(&tome);
        assert_eq!(boxes.len(), tome.global_cells.len());
        let step = |a: usize| (d.max[a] - d.min[a]) / 65535.0 * 1.01;
        for b in &boxes {
            assert!(
                (0..3).all(|a| d.min[a] - step(a) <= b.min[a] && b.max[a] <= d.max[a] + step(a))
            );
        }
        for p in global_portals(&tome) {
            assert!((p.from as usize) < boxes.len());
            assert!(p.to.is_none_or(|t| (t as usize) < boxes.len()), "{p:?}");
            assert_eq!(p.rect.min[p.axis as usize], p.rect.max[p.axis as usize]);
        }
    }
}
