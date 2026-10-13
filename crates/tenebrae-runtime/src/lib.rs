//! Native visibility queries over an Umbra 3.3.6 tome (M6,
//! `docs/implementation-artifacts/spec-m6-native-runtime.md`).
//!
//! A [`Runtime`] decodes a [`Tome`](tenebrae_tome::Tome) once into native structures and is
//! shared across threads; a [`Query`] holds one thread's scratch and answers the frustum query
//! ([`Query::frustum`]) and the portal query ([`Query::visible`]) for a [`Camera`]. Both return
//! ascending object indices; mapping objects to the caller's ids is the caller's (through the
//! tome's user lists).
//!
//! This crate is written from the tome format and the typed model in `tenebrae-tome`, not from
//! the engine's runtime. Where the two may differ (rounding at pixel edges) this runtime keeps
//! more, never less.
//!
//! Pure: no filesystem, network or GPU; `f32` arithmetic only, so the same tome, camera and
//! options give the same answer everywhere.

mod camera;
mod coverage;
mod decode;
mod query;
mod traverse;

pub use camera::{Camera, Depth};
pub use decode::{Error, Runtime};
pub use query::{Counters, Options, Query, Status, frustum, visible};
