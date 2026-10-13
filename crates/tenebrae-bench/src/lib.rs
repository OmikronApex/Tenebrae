//! Domain-agnostic benchmark for Tenebrae: generated scenes that hold no game data.
//!
//! * [`scenes`] builds every scene from a fixed seed, with `f32` arithmetic only (no `sin`, `cos`
//!   or other library functions whose last bit varies by platform), so the bytes, and their
//!   [`Scene::hash`], are the same everywhere.
//! * [`truth`] computes exact visibility for a scene's cameras by ray casting against its
//!   triangles, with the camera model the Umbra reference was measured with.
//! * [`table`] reads and writes the `scene key value` files: the Umbra reference
//!   (`reference/umbra.txt`) and Tenebrae's own baseline (`baseline.txt`).
//!
//! The flow: `bench export DIR` writes each scene's inputs, cameras and truth, which an external
//! harness baked with Umbra 3.3.6 (Unity 5.1.1's optimizer), queried in Umbra's own runtime,
//! scored against the truth and wrote as the reference. The tests in `tests/benchmark.rs` bake
//! every scene with Tenebrae and hold the gates, which need nothing outside the repository.

pub mod run;
pub mod scene;
pub mod scenes;
pub mod table;
pub mod tbsc;
pub mod truth;

pub use scene::{Camera, Scene, SceneTarget};

/// Version of the scene generator. Bump it with any change that alters a scene's bytes; the
/// reference and the baseline record it and go stale together.
pub const GENERATOR: u32 = 2;
