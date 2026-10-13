//! Tome reader: bytes to the typed model.

use super::bits::{self, LEAF};
use super::{
    Aabb, Bsp, Error, Gates, GlobalCell, HEADER_SIZE, Header, KdTree, PARAMS_SIZE, Portal, Run,
    RunList, Tile, TileCell, Tome, Users, VERSION, ViewTree,
};

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn u32(&self, offset: usize, what: &'static str) -> Result<u32, Error> {
        self.0
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or(Error::Truncated { what, offset })
    }
    fn f32(&self, offset: usize, what: &'static str) -> Result<f32, Error> {
        self.u32(offset, what).map(f32::from_bits)
    }
    fn u64(&self, offset: usize, what: &'static str) -> Result<u64, Error> {
        Ok(u64::from(self.u32(offset, what)?) | (u64::from(self.u32(offset + 4, what)?) << 32))
    }
    fn words(&self, offset: usize, count: usize, what: &'static str) -> Result<Vec<u32>, Error> {
        (0..count).map(|i| self.u32(offset + 4 * i, what)).collect()
    }
    fn floats(&self, offset: usize, count: usize, what: &'static str) -> Result<Vec<f32>, Error> {
        (0..count).map(|i| self.f32(offset + 4 * i, what)).collect()
    }
    fn aabb(&self, offset: usize, what: &'static str) -> Result<Aabb, Error> {
        let f = self.floats(offset, 6, what)?;
        Ok(Aabb {
            min: [f[0], f[1], f[2]],
            max: [f[3], f[4], f[5]],
        })
    }
}

fn kd_codes(b: &Bytes<'_>, offset: usize, n: usize, what: &'static str) -> Result<Vec<u8>, Error> {
    let words = b.words(offset, bits::packed_words(n, 2), what)?;
    Ok((0..n).map(|i| bits::field(&words, i, 2) as u8).collect())
}

fn run_list(
    b: &Bytes<'_>,
    offset: usize,
    count: usize,
    index_bits: u32,
    run_bits: u32,
    what: &'static str,
) -> Result<RunList, Error> {
    let width = index_bits + run_bits;
    let words = if count == 0 {
        Vec::new()
    } else {
        b.words(offset, bits::packed_words(count, width), what)?
    };
    let entries = (0..count)
        .map(|i| {
            let v = u64::from(bits::field(&words, i, width.min(32)));
            let index = (v & ((1u64 << index_bits) - 1)) as u32;
            let run = (v >> index_bits) as u32;
            Run { index, run }
        })
        .collect();
    if width > 32 {
        return Err(Error::Unsupported("run-list entry wider than 32 bits"));
    }
    Ok(RunList {
        index_bits,
        run_bits,
        entries,
    })
}

fn portals(
    b: &Bytes<'_>,
    offset: usize,
    count: usize,
    what: &'static str,
) -> Result<Vec<Portal>, Error> {
    (0..count)
        .map(|i| {
            let w = b.words(offset + 16 * i, 4, what)?;
            Ok(Portal([w[0], w[1], w[2], w[3]]))
        })
        .collect()
}

fn tile(b: &Bytes<'_>, base: usize) -> Result<Tile, Error> {
    let w = b.words(base, 20, "tile header")?;
    let kd_nodes = (w[6] >> 5) as usize;
    let leaf_bits = w[6] & 31;
    let cell_count = (w[13] & 0xFFFF) as usize;
    let mut cells = Vec::with_capacity(cell_count);
    for c in 0..cell_count {
        let r = b.words(base + w[14] as usize + 36 * c, 9, "tile cell")?;
        let mut a = [0u32; 9];
        a.copy_from_slice(&r);
        cells.push(TileCell(a));
    }
    let portal_count = cells
        .iter()
        .map(|c| (c.portal_start() + c.portal_count()) as usize)
        .max()
        .unwrap_or(0);
    let tile_portals = if portal_count == 0 {
        Vec::new()
    } else {
        portals(b, base + w[15] as usize, portal_count, "tile portal")?
    };
    let view = if kd_nodes > 0 {
        let codes = kd_codes(b, base + w[7] as usize, kd_nodes, "tile kd")?;
        let leaves = codes.iter().filter(|&&c| c == LEAF).count();
        let words = b.words(
            base + w[8] as usize,
            bits::packed_words(leaves, leaf_bits),
            "tile leaf values",
        )?;
        let leaf_values = (0..leaves)
            .map(|i| bits::field(&words, i, leaf_bits))
            .collect();
        let splits = b.floats(base + w[10] as usize, w[9] as usize, "tile splits")?;
        Some(ViewTree {
            tree: KdTree { codes, splits },
            leaf_bits,
            leaf_values,
        })
    } else {
        if w[7] != 0 || w[8] != 0 || w[9] != 0 {
            return Err(Error::Inconsistent(
                "tile without a view tree carries view-tree offsets",
            ));
        }
        None
    };
    let bsp = if w[16] != 0 || w[18] != 0 {
        let nodes = (0..w[17] as usize)
            .map(|i| b.u64(base + w[16] as usize + 8 * i, "bsp node"))
            .collect::<Result<Vec<_>, _>>()?;
        let planes = (0..w[19] as usize)
            .map(|i| {
                let f = b.floats(base + w[18] as usize + 16 * i, 4, "bsp plane")?;
                Ok([f[0], f[1], f[2], f[3]])
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Some(Bsp { nodes, planes })
    } else {
        None
    };
    Ok(Tile {
        bounds: b.aabb(base, "tile bounds")?,
        flags: (w[11] & 0xFF) as u8,
        level: f32::from_bits(w[12]),
        cell_word_high: (w[13] >> 16) as u16,
        cells,
        portals: tile_portals,
        view,
        bsp,
    })
}

/// Reads a tome. The data is the tome itself (package entry offset 0 = tome base).
pub fn read(data: &[u8]) -> Result<Tome, Error> {
    let b = Bytes(data);
    let first = b.u32(0, "header")?;
    if (first & 0xFFFF) as u16 != VERSION {
        return Err(Error::Version((first & 0xFFFF) as u16));
    }
    if data.len() < HEADER_SIZE {
        return Err(Error::Truncated {
            what: "header",
            offset: data.len(),
        });
    }
    let h = |o: usize| b.u32(o, "header");
    if h(0x48)? != 0 {
        return Err(Error::Unsupported("per-object records at +0x48"));
    }
    if h(0xB4)? != 0 || h(0xB8)? != 0 {
        return Err(Error::Unsupported(
            "alternative cell-to-portal index at +0xB0",
        ));
    }
    let n = h(0x90)? as usize;
    if (h(0x2C)? >> 5) as usize != n || h(0x2C)? & 31 != 0 {
        return Err(Error::Inconsistent(
            "+0x2C does not hold the top node count",
        ));
    }
    let mut params = [0u8; PARAMS_SIZE];
    params.copy_from_slice(&data[0xBC..HEADER_SIZE]);
    let widths = h(0x54)?;
    let header = Header {
        flags: (first >> 16) as u16,
        hash: h(0x04)?,
        base_distance: b.f32(0x0C, "header")?,
        word_10: h(0x10)?,
        bounds: b.aabb(0x14, "header")?,
        word_34: h(0x34)?,
        list_widths_high: widths >> 20,
        words_a4: [h(0xA4)?, h(0xA8)?, h(0xAC)?],
        words_b0: [h(0xB0)?, h(0xB4)?],
        params,
    };
    let top = KdTree {
        codes: kd_codes(&b, h(0x30)? as usize, n, "top kd")?,
        splits: b.floats(h(0x3C)? as usize, h(0x38)? as usize, "top splits")?,
    };
    let node_distances = b.floats(h(0x9C)? as usize, n + 1, "node distances")?;
    let object_count = h(0x40)? as usize;
    let objects = (0..object_count)
        .map(|i| b.aabb(h(0x44)? as usize + 24 * i, "object bounds"))
        .collect::<Result<Vec<_>, _>>()?;
    let users = if h(0x4C)? != 0 {
        let prefix = b.words(h(0x4C)? as usize, object_count + 1, "user prefix")?;
        let total = *prefix.last().unwrap_or(&0) as usize;
        Users {
            ids: b.words(h(0x50)? as usize, total, "user ids")?,
            prefix: Some(prefix),
        }
    } else if h(0x50)? != 0 {
        Users {
            prefix: None,
            ids: b.words(h(0x50)? as usize, object_count, "user ids")?,
        }
    } else {
        Users {
            prefix: None,
            ids: Vec::new(),
        }
    };
    let object_runs = run_list(
        &b,
        h(0x58)? as usize,
        h(0x5C)? as usize,
        widths & 31,
        (widths >> 5) & 31,
        "object runs",
    )?;
    let cell_runs = run_list(
        &b,
        h(0x60)? as usize,
        h(0x64)? as usize,
        (widths >> 10) & 31,
        (widths >> 15) & 31,
        "cell runs",
    )?;
    let cell_count = h(0x7C)? as usize;
    let global_cells = (0..cell_count)
        .map(|i| {
            let w = b.words(h(0x80)? as usize + 20 * i, 5, "global cell")?;
            Ok(GlobalCell {
                portal_start: w[0],
                portal_count: w[1],
                bounds: [w[2], w[3], w[4]],
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let portal_count = global_cells
        .iter()
        .map(|c| (c.portal_start + c.portal_count) as usize)
        .max()
        .unwrap_or(0);
    let global_portals = portals(&b, h(0x84)? as usize, portal_count, "global portal")?;
    let table = h(0xA0)? as usize;
    let tiles = (0..n)
        .map(|i| match b.u32(table + 4 * i, "tile table")? {
            0 => Ok(None),
            o => tile(&b, o as usize).map(Some),
        })
        .collect::<Result<Vec<_>, Error>>()?;
    // The gate index list has no count of its own: its length is the furthest any gate portal reaches.
    let index_len = global_portals
        .iter()
        .chain(tiles.iter().flatten().flat_map(|t| t.portals.iter()))
        .filter(|p| p.is_gate())
        .map(|p| ((p.0[2] >> 12) + (p.0[2] & 0xFFF)) as usize)
        .max()
        .unwrap_or(0);
    let gates = Gates {
        ids: b.words(h(0x6C)? as usize, h(0x68)? as usize, "gate ids")?,
        vertices: (0..h(0x74)? as usize)
            .map(|i| {
                let f = b.floats(h(0x70)? as usize + 12 * i, 3, "gate vertex")?;
                Ok([f[0], f[1], f[2]])
            })
            .collect::<Result<Vec<_>, Error>>()?,
        index_list: if index_len == 0 {
            Vec::new()
        } else {
            b.words(h(0x78)? as usize, index_len, "gate index list")?
        },
    };
    Ok(Tome {
        header,
        top,
        node_distances,
        objects,
        users,
        object_runs,
        cell_runs,
        global_cells,
        global_portals,
        gates,
        tiles,
    })
}
