//! Screen coverage: which pixels of a 64x64 screen a cell is seen through. One coverage is
//! 128 `u32` words, row-major, two words per row; bit `x & 31` of word `2 y + (x >> 5)` is
//! pixel `(x, y)`. A query keeps its coverages in one pool, one per cell slot, each with the
//! bounding box of its covered pixels so tests touch only the rows that can matter.
//!
//! Rasterisation is conservative: a pixel is marked when the convex polygon, grown by a
//! fraction of a pixel, touches its square, so a portal or object near a pixel edge is seen
//! rather than missed.

/// Screen width and height in pixels.
pub(crate) const SIZE: i32 = 64;
/// Words per coverage.
pub(crate) const WORDS: usize = 128;
/// Pixels a rasterised polygon is grown by on every side.
const DILATE: f32 = 0.125;

/// A pixel rectangle `[x0, x1) x [y0, y1)`, clamped to the screen; empty when `x0 >= x1` or
/// `y0 >= y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x0: i32,
    pub(crate) y0: i32,
    pub(crate) x1: i32,
    pub(crate) y1: i32,
}

impl Rect {
    pub(crate) const FULL: Rect = Rect {
        x0: 0,
        y0: 0,
        x1: SIZE,
        y1: SIZE,
    };
    pub(crate) const EMPTY: Rect = Rect {
        x0: SIZE,
        y0: SIZE,
        x1: 0,
        y1: 0,
    };

    pub(crate) fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub(crate) fn intersect(&self, o: &Rect) -> Rect {
        Rect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }

    /// The smallest rectangle holding both (an empty one contributes nothing).
    pub(crate) fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    /// The pixels the normalised-device rectangle `[nx0, nx1] x [ny0, ny1]` (each in
    /// `-1..1`) touches, conservatively.
    pub(crate) fn from_ndc(nx0: f32, ny0: f32, nx1: f32, ny1: f32) -> Rect {
        let px = |v: f32| (v + 1.0) * (0.5 * SIZE as f32);
        let lo = |v: f32| (px(v).floor().clamp(0.0, SIZE as f32)) as i32;
        let hi = |v: f32| (px(v).ceil().clamp(0.0, SIZE as f32)) as i32;
        Rect {
            x0: lo(nx0),
            y0: lo(ny0),
            x1: hi(nx1),
            y1: hi(ny1),
        }
    }
}

/// Row `y`'s two-word mask of pixels `[x0, x1)`.
fn row_mask(x0: i32, x1: i32) -> [u32; 2] {
    let word = |w: i32| -> u32 {
        let (a, b) = ((x0 - 32 * w).clamp(0, 32), (x1 - 32 * w).clamp(0, 32));
        if a >= b {
            0
        } else {
            let hi = if b == 32 { u32::MAX } else { (1u32 << b) - 1 };
            hi & !((1u32 << a) - 1)
        }
    };
    [word(0), word(1)]
}

/// The pixel span of each row of the convex polygon `poly` (pixel coordinates, any winding,
/// at most 8 vertices) within the rows of `r`, into `out[y] = [x0, x1)` (rows outside `r` or
/// the polygon get an empty span). Conservative: a pixel counts when the polygon, grown by
/// [`DILATE`] pixels, touches its square. A convex polygon covers one contiguous interval per
/// row, found from the edges clipped to the row's band.
pub(crate) fn spans_of(poly: &[[f32; 2]], r: Rect, out: &mut [[u8; 2]; SIZE as usize]) {
    out.fill([SIZE as u8, 0]);
    let n = poly.len();
    if r.is_empty() || n < 3 {
        return;
    }
    for y in r.y0..r.y1 {
        // The band of pixel row `y`, grown by the dilation.
        let (band_lo, band_hi) = (y as f32 - DILATE, (y + 1) as f32 + DILATE);
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let (ya, yb) = (a[1].min(b[1]), a[1].max(b[1]));
            if yb < band_lo || ya > band_hi {
                continue;
            }
            // The edge's x range over the band.
            let x_at = |yy: f32| {
                if a[1] == b[1] {
                    a[0]
                } else {
                    a[0] + (b[0] - a[0]) * (yy - a[1]) / (b[1] - a[1])
                }
            };
            let (y0c, y1c) = (ya.max(band_lo), yb.min(band_hi));
            let (x0, x1) = (x_at(y0c), x_at(y1c));
            lo = lo.min(x0.min(x1));
            hi = hi.max(x0.max(x1));
            if a[1] == b[1] {
                lo = lo.min(a[0].min(b[0]));
                hi = hi.max(a[0].max(b[0]));
            }
        }
        if lo > hi {
            continue;
        }
        let x0 = ((lo - DILATE).floor().max(0.0) as i32).max(r.x0);
        let x1 = ((hi + DILATE).ceil().min(SIZE as f32) as i32).min(r.x1);
        if x0 < x1 {
            out[y as usize] = [x0 as u8, x1 as u8];
        }
    }
}

/// The pool of coverages, one per slot.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pool {
    words: Vec<u32>,
    /// Per slot, the bounding box of its covered pixels.
    bbox: Vec<Rect>,
}

impl Pool {
    pub(crate) fn clear(&mut self) {
        self.words.clear();
        self.bbox.clear();
    }

    /// Adds a cleared coverage; returns its slot.
    pub(crate) fn push_empty(&mut self) -> u32 {
        let slot = self.bbox.len();
        self.words.resize(self.words.len() + WORDS, 0);
        self.bbox.push(Rect::EMPTY);
        slot as u32
    }

    /// Adds a full-screen coverage; returns its slot.
    pub(crate) fn push_full(&mut self) -> u32 {
        let slot = self.bbox.len();
        self.words.resize(self.words.len() + WORDS, u32::MAX);
        self.bbox.push(Rect::FULL);
        slot as u32
    }

    /// Marks every pixel of `slot` covered.
    pub(crate) fn fill_full(&mut self, slot: u32) {
        let s = slot as usize * WORDS;
        self.words[s..s + WORDS].fill(u32::MAX);
        self.bbox[slot as usize] = Rect::FULL;
    }

    /// The words of `slot`.
    pub(crate) fn words_of(&self, slot: u32) -> &[u32] {
        let s = slot as usize * WORDS;
        &self.words[s..s + WORDS]
    }

    /// Some pixel of `r` is covered in `slot`.
    pub(crate) fn any_in(&self, slot: u32, r: Rect) -> bool {
        let r = r.intersect(&self.bbox[slot as usize]);
        if r.is_empty() {
            return false;
        }
        let s = slot as usize * WORDS;
        let m = row_mask(r.x0, r.x1);
        (r.y0..r.y1).any(|y| {
            let i = s + 2 * y as usize;
            self.words[i] & m[0] != 0 || self.words[i + 1] & m[1] != 0
        })
    }

    /// Every pixel of `r` covered in `src` is already covered in `dst`.
    pub(crate) fn covers(&self, dst: u32, src: u32, r: Rect) -> bool {
        let r = r.intersect(&self.bbox[src as usize]);
        if r.is_empty() {
            return true;
        }
        let (ds, ss) = (dst as usize * WORDS, src as usize * WORDS);
        let m = row_mask(r.x0, r.x1);
        (r.y0..r.y1).all(|y| {
            let i = 2 * y as usize;
            (m[0] & self.words[ss + i] & !self.words[ds + i]) == 0
                && (m[1] & self.words[ss + i + 1] & !self.words[ds + i + 1]) == 0
        })
    }

    /// ORs into `dst` every pixel of the convex polygon `poly` (pixel coordinates, any
    /// winding, at most 8 vertices) that is also covered in `src`, within `r`: the polygon's
    /// [`spans_of`] through [`Pool::rasterise_spans`], the spans found over the rows of `r`
    /// and the source's covered box only.
    pub(crate) fn rasterise(&mut self, dst: u32, src: u32, poly: &[[f32; 2]], r: Rect) -> Rect {
        let r = r.intersect(&self.bbox[src as usize]);
        let mut spans = [[SIZE as u8, 0]; SIZE as usize];
        spans_of(poly, r, &mut spans);
        self.rasterise_spans(dst, src, &spans, r)
    }

    /// ORs into `dst` every pixel of the row spans `spans` (per row `y`, pixels
    /// `[spans[y][0], spans[y][1])`) that is also covered in `src`, within `r`. The rectangle
    /// of the pixels added to `dst` (empty when nothing changed).
    pub(crate) fn rasterise_spans(
        &mut self,
        dst: u32,
        src: u32,
        spans: &[[u8; 2]; SIZE as usize],
        r: Rect,
    ) -> Rect {
        let r = r.intersect(&self.bbox[src as usize]);
        if r.is_empty() {
            return Rect::EMPTY;
        }
        let mut changed = false;
        let (ds, ss) = (dst as usize * WORDS, src as usize * WORDS);
        let mut added = Rect::EMPTY;
        for y in r.y0..r.y1 {
            let s = spans[y as usize];
            let x0 = i32::from(s[0]).max(r.x0);
            let x1 = i32::from(s[1]).min(r.x1);
            if x0 >= x1 {
                continue;
            }
            let m = row_mask(x0, x1);
            let i = 2 * y as usize;
            let mut row_changed = false;
            for (k, mk) in m.iter().enumerate() {
                let add = mk & self.words[ss + i + k] & !self.words[ds + i + k];
                if add != 0 {
                    self.words[ds + i + k] |= add;
                    row_changed = true;
                }
            }
            if row_changed {
                changed = true;
                added.x0 = added.x0.min(x0);
                added.x1 = added.x1.max(x1);
                added.y0 = added.y0.min(y);
                added.y1 = added.y1.max(y + 1);
            }
        }
        if changed {
            let b = &mut self.bbox[dst as usize];
            *b = b.union(&added);
        }
        added
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_masks_cover_the_span() {
        assert_eq!(row_mask(0, 64), [u32::MAX, u32::MAX]);
        assert_eq!(row_mask(3, 5), [0b11000, 0]);
        assert_eq!(row_mask(30, 34), [0xC000_0000, 0b11]);
        assert_eq!(row_mask(40, 40), [0, 0]);
    }

    #[test]
    fn rasterisation_is_conservative_and_masked() {
        let mut pool = Pool::default();
        let src = pool.push_full();
        let dst = pool.push_empty();
        // A triangle inside pixels (10, 10)..(12, 12): those pixels are set, and with the
        // dilation (under a pixel) at most the ring around them; nothing further out.
        let poly = [[10.5, 10.5], [11.5, 10.5], [11.0, 11.5]];
        assert!(!pool.rasterise(dst, src, &poly, Rect::FULL).is_empty());
        assert!(pool.any_in(
            dst,
            Rect {
                x0: 10,
                y0: 10,
                x1: 12,
                y1: 12
            }
        ));
        assert!(!pool.any_in(
            dst,
            Rect {
                x0: 0,
                y0: 0,
                x1: 9,
                y1: 64
            }
        ));
        assert!(!pool.any_in(
            dst,
            Rect {
                x0: 13,
                y0: 0,
                x1: 64,
                y1: 64
            }
        ));
        assert!(!pool.any_in(
            dst,
            Rect {
                x0: 0,
                y0: 13,
                x1: 64,
                y1: 64
            }
        ));
        // The bounding box follows the pixels: a second rasterisation elsewhere grows it.
        let far = [[40.5, 40.5], [41.5, 40.5], [41.0, 41.5]];
        assert!(!pool.rasterise(dst, src, &far, Rect::FULL).is_empty());
        assert!(pool.any_in(
            dst,
            Rect {
                x0: 40,
                y0: 40,
                x1: 42,
                y1: 42
            }
        ));
        assert!(pool.covers(dst, dst, Rect::FULL));
        // Masked by an empty source: nothing is added.
        let empty = pool.push_empty();
        let dst2 = pool.push_empty();
        assert!(pool.rasterise(dst2, empty, &poly, Rect::FULL).is_empty());
        assert!(!pool.any_in(dst2, Rect::FULL));
    }
}
