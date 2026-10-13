# Milestones — capability map and evidence

Each capability in `SPEC.md` was delivered by one or more milestones and accepted by the evidence listed below. "—" means no spec was written for that milestone.

| Milestone | Capabilities | Spec | Acceptance evidence |
|---|---|---|---|
| M1 format | CAP-1 | — | Byte-identical round trip: 26/26 stock tomes, 4/4 Unity 5.1.1 bakes |
| M2 geometry core | CAP-2 | — | 784k segments vs 10 M triangles in 2.4 s |
| M3 baker | CAP-3 | — | Parity scene (10 M triangles), 586 engine views: status 0, 0 truly visible culled |
| M3b scale | CAP-4 | `docs/specs/spec-m3b-sparse-voxel-grid/SPEC.md`, `docs/implementation-artifacts/spec-m3b-sparse-voxel-grid.md`, `spec-m3b-l4-midpoint-view-tree.md` | About 1/30 the memory of a dense grid; 65-78 s, +200 MB on 16 threads |
| Tome size at Umbra parity | CAP-4 | — | 667,168 B vs Umbra 1,284,240 B (stock 1,114,672 B); 423 objects kept per view (stock 343); 40 bakes in 72 s vs Umbra 112 s |
| M3c incremental re-bake | CAP-5 | `docs/implementation-artifacts/spec-m3c-incremental-rebake.md` | Cached = fresh bytes; 2.4 s unchanged, 4.9 s one prop moved |
| M4 bake API (contract v2) | CAP-6 | `docs/implementation-artifacts/spec-m4-bake-api.md` | Default = M3c bytes; instanced = soup; `keep_every_target` gives an identity map |
| Query cost in the engine's runtime | CAP-9 | — | Portal queries −11 % with LOD tiles, −13 % vs Umbra's tomes; kept +0.2 % |
| M6 native runtime | CAP-7 | `docs/implementation-artifacts/spec-m6-native-runtime.md` | 1,029 captures: statuses and frustum lists equal; kept +3 to +6.5 %; 272 µs vs 129 µs median (2.1×, ≤ 1.5× open) |
| Workspace crate split | constraints AD-1, AD-2 | `docs/implementation-artifacts/spec-workspace-crate-split.md` | Crate layout and dependency directions enforced by `Cargo.toml` |
| Benchmark | CAP-8 | `docs/implementation-artifacts/plan-bench-gaps.md`, `crates/tenebrae-bench/README.md` | Gates green; `reference/umbra.txt` committed |
| Publication (crates.io, GitHub, Actions) | CAP-10 | — (not started) | — |
