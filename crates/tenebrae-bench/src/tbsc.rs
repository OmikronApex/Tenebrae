//! TBSC: the benchmark's compact scene format for real-world geometry (`assets/*.tbsc`).
//!
//! Little endian:
//!
//! * header: `TBSC`, u32 version 1, u32 object count, u32 table bytes, u32 position bytes, u32
//!   index bytes;
//! * object table, per object: f32[6] target box (min, max: every triangle of the source object,
//!   occluding or not), f32[6] quantisation box (min, max of its occluder vertices), u32 vertex
//!   count, u32 triangle count;
//! * positions, per object and vertex, per axis: LEB128 varint of the zigzagged difference to the
//!   previous vertex's u16 on that axis (wrapping mod 2^16; the first vertex differs from 0);
//! * indices, per object and triangle corner: LEB128 varint `c`. Vertices are numbered in order of
//!   first use, so `c = 0` is the next new vertex and `c > 0` is vertex `seen - c`.
//!
//! A position decodes as `lo + (hi - lo) * (q / 65535)` in `f32`, evaluated in that order, so it
//! is the same on every platform. An object with no vertices is a target with no geometry.

use tenebrae::geom::{Mesh, Vec3};
use tenebrae::tome::Aabb;

use crate::scene::SceneTarget;

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or_else(|| format!("truncated at {}", self.at))?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        self.u32().map(f32::from_bits)
    }
    fn vec3(&mut self) -> Result<Vec3, String> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    fn varint(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.bytes(1)?[0];
            v |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(format!("varint too long at {}", self.at))
    }
}

/// A decoded TBSC scene: occluder triangles (owner = object index) and one target per object.
#[derive(Clone, Debug)]
pub struct Decoded {
    /// Occluders.
    pub mesh: Mesh,
    /// Per object its target; `owner` is the object index when it has occluder triangles.
    pub targets: Vec<SceneTarget>,
}

/// Decodes a TBSC file.
pub fn decode(data: &[u8]) -> Result<Decoded, String> {
    let mut head = Reader { data, at: 0 };
    if head.bytes(4)? != b"TBSC" {
        return Err("not a TBSC file".into());
    }
    let version = head.u32()?;
    if version != 1 {
        return Err(format!("TBSC version {version}, expected 1"));
    }
    let n = head.u32()? as usize;
    let (tb, pb, ib) = (
        head.u32()? as usize,
        head.u32()? as usize,
        head.u32()? as usize,
    );
    let table = head.bytes(tb)?;
    let positions = head.bytes(pb)?;
    let indices = head.bytes(ib)?;
    if head.at != data.len() {
        return Err("trailing bytes".into());
    }
    let mut t = Reader { data: table, at: 0 };
    let mut p = Reader {
        data: positions,
        at: 0,
    };
    let mut x = Reader {
        data: indices,
        at: 0,
    };
    let mut mesh = Mesh::default();
    let mut targets = Vec::with_capacity(n);
    for obj in 0..n as u32 {
        let (tmin, tmax) = (t.vec3()?, t.vec3()?);
        let (lo, hi) = (t.vec3()?, t.vec3()?);
        let (nv, nt) = (t.u32()? as usize, t.u32()? as usize);
        let base = mesh.vertices.len() as u32;
        let mut q = [0u16; 3];
        for _ in 0..nv {
            let mut v = [0.0f32; 3];
            for k in 0..3 {
                let z = p.varint()?;
                let d = ((z >> 1) as i64) ^ -((z & 1) as i64);
                q[k] = (i64::from(q[k]) + d).rem_euclid(1 << 16) as u16;
                v[k] = lo[k] + (hi[k] - lo[k]) * (f32::from(q[k]) / 65535.0);
            }
            mesh.vertices.push(v);
        }
        let mut seen = 0u32;
        for _ in 0..nt {
            let mut tri = [0u32; 3];
            for c in &mut tri {
                let code = x.varint()?;
                let local = if code == 0 {
                    seen += 1;
                    seen - 1
                } else {
                    seen.checked_sub(code as u32)
                        .ok_or_else(|| format!("object {obj}: bad back-reference"))?
                };
                *c = base + local;
            }
            mesh.triangles.push(tri);
            mesh.owners.push(obj);
        }
        if seen as usize != nv {
            return Err(format!("object {obj}: {seen} vertices used, {nv} stored"));
        }
        targets.push(SceneTarget {
            bounds: Aabb {
                min: tmin,
                max: tmax,
            },
            owner: (nt > 0).then_some(obj),
        });
    }
    if t.at != table.len() || p.at != positions.len() || x.at != indices.len() {
        return Err("section lengths disagree with the object table".into());
    }
    Ok(Decoded { mesh, targets })
}
