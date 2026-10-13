//! `truth SCENE QUERIES OUT` — exact segment occlusion against a triangle scene.
//!
//! SCENE (little endian): u32 vertex count, u32 triangle count, f32[3] per vertex,
//! u32[3] per triangle, u32 owner per triangle.
//! QUERIES: u32 count, then per query f32[3] eye, f32[3] point, u32 skip count, u32 skip owners.
//! OUT: one byte per query, 1 = the open segment eye -> point crosses a triangle whose owner is
//! not skipped, 0 = clear.

use std::time::Instant;

use tenebrae::geom::{Bvh, Mesh, Vec3};

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> Result<u32, String> {
        let b = self
            .data
            .get(self.at..self.at + 4)
            .ok_or_else(|| format!("truncated at {}", self.at))?;
        self.at += 4;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        self.u32().map(f32::from_bits)
    }
    fn vec3(&mut self) -> Result<Vec3, String> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: truth SCENE QUERIES OUT".into());
    }
    let scene = std::fs::read(&args[1]).map_err(|e| format!("{}: {e}", args[1]))?;
    let queries = std::fs::read(&args[2]).map_err(|e| format!("{}: {e}", args[2]))?;
    let mut c = Cursor {
        data: &scene,
        at: 0,
    };
    let nv = c.u32()? as usize;
    let nt = c.u32()? as usize;
    let mut mesh = Mesh {
        vertices: Vec::with_capacity(nv),
        triangles: Vec::with_capacity(nt),
        owners: Vec::with_capacity(nt),
    };
    for _ in 0..nv {
        mesh.vertices.push(c.vec3()?);
    }
    for _ in 0..nt {
        mesh.triangles.push([c.u32()?, c.u32()?, c.u32()?]);
    }
    for _ in 0..nt {
        mesh.owners.push(c.u32()?);
    }
    let t0 = Instant::now();
    let bvh = Bvh::build(&mesh);
    eprintln!(
        "bvh: {} triangles in {:.2} s",
        bvh.len(),
        t0.elapsed().as_secs_f32()
    );
    let mut q = Cursor {
        data: &queries,
        at: 0,
    };
    let count = q.u32()? as usize;
    let mut out = Vec::with_capacity(count);
    let t1 = Instant::now();
    let mut skip = Vec::new();
    for _ in 0..count {
        let eye = q.vec3()?;
        let point = q.vec3()?;
        let n = q.u32()? as usize;
        skip.clear();
        for _ in 0..n {
            skip.push(q.u32()?);
        }
        out.push(u8::from(bvh.segment_blocked(eye, point, &skip)));
    }
    eprintln!("queries: {count} in {:.2} s", t1.elapsed().as_secs_f32());
    std::fs::write(&args[3], out).map_err(|e| format!("{}: {e}", args[3]))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("truth: {e}");
        std::process::exit(2);
    }
}
