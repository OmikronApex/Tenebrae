//! Tenebrae: a reader, writer, baker and runtime for the Umbra 3.3.6 visibility tome (format
//! version 18). This crate is the facade over the workspace's pure libraries (contract v2):
//!
//! * [`tome`] (`tenebrae-tome`) reads a tome into a lossless, typed model and writes the model
//!   back. Everything the format derives (rank directories, node keys, the per-node cell prefix,
//!   offsets, sizes, tile sizes) is recomputed by the writer rather than copied, so a
//!   byte-identical round trip of a baked tome is evidence that the layout is understood, not
//!   that bytes were carried along.
//! * [`geom`] (`tenebrae-geom`): triangle soup, BVH, exact segment occlusion.
//! * [`bake`] (`tenebrae-bake`): occluders, targets and a view volume in; a tome out.
//! * [`runtime`] (`tenebrae-runtime`): frustum and portal queries over a decoded tome.
//! * [`query`]: point lookups over a tome (`tenebrae-tome`'s `locate`) and the runtime's camera
//!   queries, re-exported together.
//!
//! The crate is pure: it never touches the filesystem, network or GPU. Callers hand it bytes.
//! The layout it implements is documented in `tenebrae-tome` (the model's doc comments and
//! `write.rs`, which records the section order and padding).

pub use tenebrae_bake as bake;
pub use tenebrae_geom as geom;
pub use tenebrae_runtime as runtime;
pub use tenebrae_tome as tome;

pub use tome::{Tome, read, write};

/// Point, cell and camera queries over a [`Tome`].
///
/// The point lookups (`find_cell`, `Locator`, `leaves`, `cell_regions`, `transfer`,
/// `global_cell_boxes`, `global_portals`) are `tenebrae_tome::locate`. The camera queries
/// (`Runtime`, `Query`, `Camera`, `Options`, `Status`, `visible`, `frustum`) are
/// `tenebrae_runtime`.
pub mod query {
    pub use tenebrae_runtime::*;
    pub use tenebrae_tome::locate::*;
}
