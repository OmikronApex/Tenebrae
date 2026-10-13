---
title: 'Workspace crate split (format, geometry, baker, oracle, facade)'
type: 'refactor'
created: '2026-10-13'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
context:
  - '{project-root}/docs/planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md'
  - '{project-root}/README.md'
  - 'the consumer's Cargo.toml (external) (workspace layout under crates/)'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Tenebrae is one library crate at the repository root (`src/`), plus `tools/` and
`bench/`. Four things live in that one crate with different futures: the tome format (the
contract everything agrees on), the geometry core, the baker, and `query`, which is a port of
the engine's Umbra 3.3.6 runtime over emulated memory. For the consumer one crate is fine. For a public release
as a Rust occlusion-culling toolkit it is not: a runtime user would pull a 10k-line baker, the
baker's `cell_points` reaches into `query`, and the runtime port, built from proprietary code, must stay a private test oracle rather than ship. A real runtime (M6) needs an
empty, well-bounded crate to land in.

**Approach:** Split the library along its existing module seams into workspace crates under
`crates/`, as the consumer lays its workspace out. Keep one facade crate named `tenebrae` whose paths
are exactly today's (`tenebrae::bake::*`, `tenebrae::query::*`, `tenebrae::{read, write,
Tome}`), so contract v2 is unchanged and the consumer, `tools/`, `bench/` and the external harness scripts build
as before. Move the point lookups (`find_cell`, `Locator`, `leaves`, `cell_regions`,
`transfer`, `global_cell_boxes`, `global_portals`) out of the runtime port into the format
crate, which removes the baker's dependency on the port. Byte identity of every output is the
acceptance oracle: the split changes no algorithm.

**Crates after the split** (all `edition 2024`, `rust-version 1.95`, toolchain pin unchanged):

| crate | path | contents | depends on | publishable |
|---|---|---|---|---|
| `tenebrae-tome` | `crates/tenebrae-tome` | model, bit reader, `read`, `write`, `Aabb`; new module `locate` (today's non-port `query` items); `tests/roundtrip.rs` | nothing | yes |
| `tenebrae-geom` | `crates/tenebrae-geom` | `Mesh`, `Bounds`, `Bvh`, segment occlusion, `triangle_box` | nothing | yes |
| `tenebrae-bake` | `crates/tenebrae-bake` | `bake` and its modules (`tile`, `grid`, `global`, `lod`, `group`, `cache`, `reference`, tests) | tome, geom | yes |
| `tenebrae-oracle` | `crates/tenebrae-oracle` | the M5 runtime port: `Camera`, `Prepared`, `visible`, `frustum`, `Status`, `Visibility`, `rcp`; `mem`, `simd`, `traverse`, `walk`, `raster`, `coverage`, `runtime`, `portal` | tome | **never** (`publish = false`) |
| `tenebrae` | `crates/tenebrae` | facade: `pub use tenebrae_tome as tome`, `… as geom`, `… as bake`, `pub use tome::{Tome, read, write}`, and `pub mod query` re-exporting `tome::locate::*` and, under the default feature `oracle`, `tenebrae_oracle::*` | tome, geom, bake, oracle (feature) | yes (without the `oracle` feature once M6 replaces it) |
| `tenebrae-tools` | `crates/tenebrae-tools` | as today | tenebrae, tenebrae-bench | yes |
| `tenebrae-bench` | `crates/tenebrae-bench` | as today (`assets/`, `reference/`, `baseline.txt` move with it) | tenebrae | yes |

The root `Cargo.toml` becomes a virtual workspace (no root package) with
`default-members` = every crate but `tenebrae-bench`, so `cargo test --release` at the root
runs the library and tools tests in seconds and the benchmark stays an explicit
`-p tenebrae-bench`, as now. Lints move to `[workspace.lints]` with `lints.workspace = true`
in every crate (bench keeps `unsafe_code = "deny"` for `tests/rcp.rs`).

## Boundaries & Constraints

**Always:**
- **Byte identity, required.** After the split: every pinned hash in `tenebrae-bake`'s tests
  (`l4_off_bakes_are_unchanged`, `cache_bytes_are_pinned_to_the_cache_version`) passes
  unchanged; `bench/baseline.txt` passes unchanged; the stock-tome round trip passes; and
  the evaluation harness with the item-9 recipe reproduces `fg9_s1f.tome` byte for byte.
- **Contract v2 paths unchanged.** Every item listed under "Contract v2 (M4)" in the README
  resolves at the same `tenebrae::…` path with the same signature. The consumer's snippet compiles as
  written. No contract version bump.
- **Dependency directions (spine AD-2, amended):** `tome` and `geom` depend on nothing;
  `bake` on tome and geom; `oracle` on tome; the facade on all four; tools and bench on the
  facade. Nothing depends on `bake` but the facade. The oracle depends on nothing but tome.
- Library rules hold in tome, geom, bake, oracle and the facade: no dependencies outside the
  workspace, no `unsafe`, no I/O (AD-1).
- `cargo build --release -p tenebrae-tools` still puts the binaries at `target/release/`, where
  the external harness scripts expect them (AD-12). Positionals and blobs unchanged.
- One commit per mechanical step where possible (move, then rename paths, then docs), each
  passing fmt, clippy and tests, so `git log --follow` keeps file history.

**Never:**
- No algorithm, parameter, default or format change. No `pub` surface removed; items that must
  become visible across crates go from `pub(crate)`/`pub(super)` to `pub` with
  `#[doc(hidden)]` where they are internal (`tome::bits`, `geom` helpers, `global::pack_lists`).
- No new runtime code. M6 is a separate spec; this split only makes room for it.
- No `oracle` code outside `crates/tenebrae-oracle` and the facade's feature-gated re-export.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| The consumer build | `tenebrae = { path = "…/crates/tenebrae" }` (or the repo root with a `[workspace]`-aware path) | README snippet compiles; `tenebrae::query::find_cell` etc. resolve | N/A |
| Runtime-only consumer | depends on `tenebrae-tome` only | reads, writes, locates cells; no bake, no oracle compiled | N/A |
| Facade without oracle | `default-features = false` | `tenebrae::query` holds the locate items only; `visible`/`frustum` absent | compile error for callers of the port, by design |
| Bench | `cargo test --release -p tenebrae-bench` | 12 tests pass against the unchanged baseline; `rcp` reached through `tenebrae::query::rcp` | N/A |
| Tools | `cargo build --release -p tenebrae-tools` | seven binaries at `target/release/` | N/A |
| The external research driver | The evaluation harness item-9 recipe | tome byte-identical to `fg9_s1f.tome` | N/A |
| Root test | `cargo test --release` | library + tools tests only (default members) | N/A |

</frozen-after-approval>

## Code Map

- `src/lib.rs` — four `pub mod`s and `pub use tome::{Tome, read, write}`; becomes the facade.
- `src/tome/{mod,bits,read,write}.rs` → `crates/tenebrae-tome/src/{lib,bits,read,write}.rs`.
  `tests/roundtrip.rs` → `crates/tenebrae-tome/tests/roundtrip.rs` (reads `TENEBRAE_TOMES`).
- `src/query/mod.rs` — split: `leaves`, `ranks`, `leaf_at`, `leaf_ranked`, `find_cell`,
  `leaf_cell`, `global_of`, `cell_regions`, `Locator`, `CellVotes`, `transfer`, `dequant`,
  `global_cell_boxes`, `PortalRect`, `global_portals`, `portal_rect` →
  `crates/tenebrae-tome/src/locate.rs`; `Status`, `Visibility`, `Prepared`, `visible`,
  `frustum`, `rcp`, `Camera` and the submodules `camera`, `coverage`, `frustum`, `mem`,
  `portal`, `raster`, `runtime`, `simd`, `traverse`, `walk`, `tests` →
  `crates/tenebrae-oracle/src/`. The port's `tests.rs` builds tomes by hand (`two_cell_tome`)
  and uses `tome::{Gates, Header, PARAMS_SIZE, …}`: all public already.
- `src/geom.rs` → `crates/tenebrae-geom/src/lib.rs`.
- `src/bake.rs` + `src/bake/*` → `crates/tenebrae-bake/src/lib.rs` + `src/*`. `bake.rs:1349`
  `crate::query::Locator::new(&tome)` → `tenebrae_tome::locate::Locator`. `use crate::geom::…`
  / `crate::tome::…` → `tenebrae_geom::…` / `tenebrae_tome::…`.
- Visibility to raise: `tome::bits` (used by `read`/`write` only, stays private),
  `global::pack_lists` (used by `group`, same crate, unchanged). Check `cargo doc` for
  `unreachable_pub`/`missing_docs` after the move.
- `tools/` → `crates/tenebrae-tools/` (`src/bin/*.rs` unchanged; `use tenebrae::…` paths
  unchanged). `bench/` → `crates/tenebrae-bench/` (`assets/`, `reference/`, `baseline.txt`,
  `README.md`, `examples/`, `tests/` move with it; `tests/benchmark.rs` locates assets relative
  to `CARGO_MANIFEST_DIR`, check).
- Docs to update: `README.md` (status row, contract section's dependency path, structural
  seed), `AGENTS.md` (layout, commands), spine (AD-2 crate graph, structural seed, a new AD-13
  "the oracle never ships"), `bench/README.md` paths, `.githooks/pre-commit` (commands already
  `--workspace`).
- Engine evaluation: the external harness scripts reference `target/release/*.exe` only; no change.

## Tasks & Acceptance

**Execution:**
- [x] Root `Cargo.toml`: virtual workspace, `members` = the seven crates, `default-members` =
  all but bench, `[workspace.package]` (edition, rust-version, license) and `[workspace.lints]`.
- [x] `git mv` moves per crate; new `Cargo.toml` per crate; `lib.rs` headers kept.
- [x] `tenebrae-tome::locate` carved out of `query/mod.rs`; oracle gets the rest.
- [x] Facade `crates/tenebrae/src/lib.rs` with the `oracle` default feature.
- [x] `tenebrae-bake` imports repointed; `Locator` from `tenebrae_tome::locate`.
- [x] `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --release`
  (default members), `cargo test --release -p tenebrae-bench`, round trip with `TENEBRAE_TOMES`.
- [x] `cargo build --release -p tenebrae-tools`; the evaluation harness item-9 recipe;
  `cmp` against `fg9_s1f.tome`.
- [x] Docs: README, AGENTS.md, spine (AD-1, AD-2 amended, AD-13 added), bench README
  (no path changes needed: it names files relative to the crate). Memory note on the new layout.

**Acceptance Criteria:**
- Given the split workspace, when every gate above runs, then all pass with no pin, baseline
  or reference regenerated.
- Given the README's contract snippet, when compiled against `crates/tenebrae`, then it builds
  with default features and `tenebrae::query::{find_cell, Locator, transfer, …}` resolve
  without the `oracle` feature.
- Given `crates/tenebrae-oracle/Cargo.toml`, then `publish = false` and its README header
  states why.

## Implementation Notes

- **Done 2026-10-13**, one mechanical commit for the move and one for the docs. Every move is a
  `git mv` (git shows `R` for all 40 files), so `git log --follow` keeps history.
- The only code that changed beyond `use` paths: `query/mod.rs` was cut at `leaves` into
  `tenebrae-tome/src/locate.rs` (the point lookups) and `tenebrae-oracle/src/lib.rs` (the port);
  the oracle's `tests.rs` imports `find_cell` and the model types explicitly, where `use super::*`
  used to carry them; `transfer`'s doc links to `Report` became plain text (the format crate does
  not know the baker). `global::pack_lists` was already `pub(super)` for `group.rs`; nothing else
  needed raising. `cargo clippy --workspace --all-targets -- -D warnings` is clean with
  `unreachable_pub` and `missing_docs` on in every library crate.
- The IDE holding files open made several gortex batch edits fail on rename with a file lock;
  the same edits applied one file at a time.
- **Gates, all unchanged:** 66 bake + 9 oracle + 2 tome + 2 geom tests and the pinned hashes;
  round trip 12/12 installed stock tomes byte-identical and `stock_tomes_answer_queries`;
  benchmark 12/12 against `baseline.txt` plus `rcp_matches_this_cpu`; the evaluation harness
  with the item-9 recipe (`fg9_split.tome`, 882,112 B, 1,561 cells, 0 wrongly culled) byte-identical
  to `fg9_s1f.tome` from before the split (`cmp`).
- the external harness scripts needed no change: they call `target/release/*.exe`, which the workspace still
  builds there.

## Spec Change Log

## Review Triage Log

| # | Source | Finding | Verdict | Route | Evidence |
|---|---|---|---|---|---|

## Verification

**Commands:**
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` -- clean.
- `cargo test --release` and `cargo test --release -p tenebrae-bench` -- all pass, baseline unchanged.
- `set TENEBRAE_TOMES=…` then `cargo test --release -p tenebrae-tome` -- round trip 26/26 + 4/4.
- `cargo build --release -p tenebrae-tools` then the item-9 parity recipe -- byte-identical tome.
