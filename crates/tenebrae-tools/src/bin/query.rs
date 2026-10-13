//! `query TOME CAMERAS OUT [radius]` — camera queries on a tome, per camera, with
//! `tenebrae::runtime`.
//!
//! CAMERAS (little endian) is either
//! * a benchmark `cameras.bin`: u32 count, then per camera f32[3] eye and f32[3] look-at point,
//!   the camera built as the benchmark builds it (`tenebrae_bench::truth`); the radius is the
//!   `radius` argument (default 0); or
//! * `"TBQC"`, u32 version 1, u32 count, then per camera f32[16] row-vector world-to-clip
//!   matrix (row by row), f32[3] position and f32 radius (the argument is ignored).
//!
//! OUT: `"TBQV"`, u32 version 1, u32 count, then per camera u32 status, u32 k, k × u32 portal
//! query objects (ascending), u32 m, m × u32 frustum query objects (ascending). The status is
//! 0 (ok), 2 (traversal budget exhausted), 3 (bad camera) or 5 (the camera is in no cell). A
//! frustum query with a non-zero status is an error, not output.
//!
//! Stderr: the camera count and wall time, then the median over cameras of the best of five
//! timed portal queries each, in microseconds.

use std::time::Instant;

use tenebrae::runtime::{Camera, Options, Query, Runtime, Status};
use tenebrae_bench::Camera as BenchCamera;
use tenebrae_bench::truth;

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("truncated at {at}"))
}

fn f32_at(b: &[u8], at: usize) -> Result<f32, String> {
    u32_at(b, at).map(f32::from_bits)
}

/// `n` records of `size` bytes after `head` bytes fit in `b`.
fn fits(b: &[u8], head: usize, n: usize, size: usize) -> Result<(), String> {
    match n.checked_mul(size).and_then(|x| x.checked_add(head)) {
        Some(end) if end <= b.len() => Ok(()),
        _ => Err(format!(
            "{n} cameras need {size} bytes each after {head}, file has {}",
            b.len()
        )),
    }
}

/// Per camera: row-vector world-to-clip matrix, position, radius.
type Cam = ([[f32; 4]; 4], [f32; 3], f32);

fn cameras(b: &[u8], radius: f32) -> Result<Vec<Cam>, String> {
    if b.starts_with(b"TBQC") {
        let version = u32_at(b, 4)?;
        if version != 1 {
            return Err(format!("TBQC version {version}, expected 1"));
        }
        let n = u32_at(b, 8)? as usize;
        fits(b, 12, n, 80)?;
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let at = 12 + 80 * k;
            let mut m = [[0.0f32; 4]; 4];
            for (r, row) in m.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v = f32_at(b, at + 16 * r + 4 * c)?;
                }
            }
            let p = [
                f32_at(b, at + 64)?,
                f32_at(b, at + 68)?,
                f32_at(b, at + 72)?,
            ];
            out.push((m, p, f32_at(b, at + 76)?));
        }
        return Ok(out);
    }
    let n = u32_at(b, 0)? as usize;
    fits(b, 4, n, 24)?;
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let at = 4 + 24 * k;
        let v = |o: usize| f32_at(b, at + o);
        let c = BenchCamera {
            eye: [v(0)?, v(4)?, v(8)?],
            target: [v(12)?, v(16)?, v(20)?],
        };
        out.push((truth::clip_matrix(&c), c.eye, radius));
    }
    Ok(out)
}

/// One camera's answer: status, portal objects, frustum objects.
type Answer = (u32, Vec<u32>, Vec<u32>);

fn answers(tome: &tenebrae::Tome, cams: &[Cam]) -> Result<(Vec<Answer>, Vec<f64>), String> {
    let rt = Runtime::new(tome).map_err(|e| e.to_string())?;
    let mut q = Query::new(&rt);
    let mut answers = Vec::with_capacity(cams.len());
    let mut best = Vec::with_capacity(cams.len());
    let code = |s: Status| match s {
        Status::Budget => 2,
        Status::BadCamera => 3,
        Status::NoCell => 5,
    };
    for &(m, p, r) in cams {
        let c = Camera::from_world_to_clip_rows(m, p);
        let opts = Options {
            radius: r,
            ..Options::default()
        };
        let (status, objects) = match q.visible(&rt, &c, &opts) {
            Ok(list) => (0, list.to_vec()),
            Err(s) => (code(s), Vec::new()),
        };
        let frustum = q
            .frustum(&rt, &c)
            .map(<[u32]>::to_vec)
            .map_err(|s| format!("frustum query status {}", code(s)))?;
        answers.push((status, objects, frustum));
        let mut t = f64::INFINITY;
        for _ in 0..5 {
            let t0 = Instant::now();
            let _ = q.visible(&rt, &c, &opts);
            t = t.min(t0.elapsed().as_secs_f64());
        }
        best.push(t * 1e6);
    }
    Ok((answers, best))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if !(4..=5).contains(&args.len()) {
        return Err("usage: query TOME CAMERAS OUT [radius]".into());
    }
    let radius: f32 = match args.get(4) {
        Some(r) => r.parse().map_err(|e| format!("radius {r}: {e}"))?,
        None => 0.0,
    };
    let bytes = std::fs::read(&args[1]).map_err(|e| format!("{}: {e}", args[1]))?;
    let tome = tenebrae::read(&bytes).map_err(|e| format!("{}: {e}", args[1]))?;
    let cams = std::fs::read(&args[2]).map_err(|e| format!("{}: {e}", args[2]))?;
    let cams = cameras(&cams, radius)?;
    let t0 = Instant::now();
    let (answers, mut best) = answers(&tome, &cams)?;
    let elapsed = t0.elapsed().as_secs_f64();
    let mut out = Vec::new();
    out.extend_from_slice(b"TBQV");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(cams.len() as u32).to_le_bytes());
    for (status, objects, frustum) in &answers {
        out.extend_from_slice(&status.to_le_bytes());
        for list in [objects, frustum] {
            out.extend_from_slice(&(list.len() as u32).to_le_bytes());
            for k in list {
                out.extend_from_slice(&k.to_le_bytes());
            }
        }
    }
    std::fs::write(&args[3], &out).map_err(|e| format!("{}: {e}", args[3]))?;
    best.sort_by(f64::total_cmp);
    let median = best.get(best.len() / 2).copied().unwrap_or(0.0);
    eprintln!(
        "{} cameras in {elapsed:.2} s; portal query median of best-of-5: {median:.1} us",
        cams.len()
    );
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("query: {e}");
        std::process::exit(1);
    }
}
