//! [`Query`]: one thread's scratch for the camera queries.

use crate::camera::Camera;
use crate::decode::Runtime;
use crate::traverse::Traversal;

/// Why a query gave no object list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// The camera is in no cell: outside the domain or in solid space. Callers fall back to
    /// the frustum query.
    NoCell,
    /// The camera's matrix cannot project (a multiple of the identity, or not finite).
    BadCamera,
    /// The portal traversal hit its work bound. Callers fall back to the frustum query.
    Budget,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Status::NoCell => "camera in no cell",
            Status::BadCamera => "degenerate camera",
            Status::Budget => "traversal budget exhausted",
        })
    }
}

impl std::error::Error for Status {}

/// Options of the portal query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options<'g> {
    /// The camera's collision radius in metres: the query starts in every cell the near
    /// plane's quad, grown by it, touches. 0 starts in the cells holding the position and the
    /// near plane's corners.
    pub radius: f32,
    /// Level-of-detail distance scale: a coarse tile stands in for its subtree once the camera
    /// is farther from the node's box than the node's distance entry times
    /// `max(accuracy, base distance)`. `-1` (the default) uses the tome's base distance.
    pub accuracy: f32,
    /// Per gate (in the tome's gate order), whether it is open; a gate past the end of the
    /// slice is open. `None`: every gate open.
    pub gates: Option<&'g [bool]>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options {
            radius: 0.0,
            accuracy: -1.0,
            gates: None,
        }
    }
}

/// What a query did, for cost models and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Tiles entered.
    pub tiles: u32,
    /// Cells finished (visits; a cell whose coverage grows again is visited again).
    pub cells: u32,
    /// Portals considered.
    pub portals: u32,
    /// Portals rasterised.
    pub rasterised: u32,
    /// Objects tested against a cell's coverage.
    pub candidates: u32,
    /// Objects output.
    pub output: u32,
    /// Pixel rows handed to the rasteriser, summed over rasterisations.
    pub raster_rows: u32,
    /// Portals rejected, by reason: boundary or closed gate, no target or self, off screen,
    /// outside the source's fresh pixels, no source pixel in the rectangle, target already
    /// covered.
    pub rejected: [u32; 6],
}

/// One thread's query scratch. Reuse it across cameras: after the first query of a given size
/// it allocates nothing.
#[derive(Clone, Debug, Default)]
pub struct Query {
    out: Vec<u32>,
    counters: Counters,
    traversal: Traversal,
}

impl Query {
    /// A query for `rt`.
    pub fn new(rt: &Runtime) -> Query {
        Query {
            out: Vec::with_capacity(rt.objects().min(1 << 16)),
            counters: Counters::default(),
            traversal: Traversal::default(),
        }
    }

    /// The frustum query: every object whose box is not outside the camera's frustum,
    /// ascending.
    pub fn frustum(&mut self, rt: &Runtime, camera: &Camera) -> Result<&[u32], Status> {
        self.out.clear();
        self.counters = Counters::default();
        if camera.degenerate {
            return Err(Status::BadCamera);
        }
        for (i, b) in rt.objects.iter().enumerate() {
            self.counters.candidates += 1;
            if camera.box_in_frustum(b.min, b.max) {
                self.out.push(i as u32);
            }
        }
        self.counters.output = self.out.len() as u32;
        Ok(&self.out)
    }

    /// The portal query: every object some sight line from the camera reaches through the
    /// tome's cells and portals, ascending. On an `Err` the caller falls back to
    /// [`Query::frustum`].
    pub fn visible(
        &mut self,
        rt: &Runtime,
        camera: &Camera,
        opts: &Options,
    ) -> Result<&[u32], Status> {
        self.out.clear();
        self.counters = Counters::default();
        self.traversal
            .run(rt, camera, opts, &mut self.out, &mut self.counters)?;
        Ok(&self.out)
    }

    /// The counters of the last query.
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// Diagnostics: after a portal query, the cells reached, as `(top node, local cell)` and
    /// the 128 words of their 64x64 screen coverage (row-major, two words per row, bit
    /// `x & 31` of word `2 y + (x >> 5)` is pixel `(x, y)`).
    pub fn reached_cells(&self) -> Vec<((u32, u32), Vec<u32>)> {
        self.traversal
            .reached()
            .map(|(c, w)| (c, w.to_vec()))
            .collect()
    }

    /// Diagnostics: the pixel rectangle `[x0, x1) x [y0, y1)` of the 64x64 screen that the
    /// portal query tests a box against (the whole screen when a corner lies behind the near
    /// plane).
    pub fn screen_rect(&self, camera: &Camera, b: &tenebrae_tome::Aabb) -> [i32; 4] {
        let r = self.traversal.rect_of(camera, b);
        [r.x0, r.y0, r.x1, r.y1]
    }
}

/// [`Query::frustum`] with a fresh [`Query`].
pub fn frustum(rt: &Runtime, camera: &Camera) -> Result<Vec<u32>, Status> {
    Query::new(rt).frustum(rt, camera).map(<[u32]>::to_vec)
}

/// [`Query::visible`] with a fresh [`Query`].
pub fn visible(rt: &Runtime, camera: &Camera, opts: &Options) -> Result<Vec<u32>, Status> {
    Query::new(rt)
        .visible(rt, camera, opts)
        .map(<[u32]>::to_vec)
}
