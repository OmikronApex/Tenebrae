---
id: SPEC-tenebrae
status: final
companions:
  - milestones.md
  - ../../planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md
sources:
  - ../../../README.md
  - ../../../AGENTS.md
  - ../../../CONTRIBUTING.md
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete, preservation-validated contract for what to build, test, and validate. Source documents listed in frontmatter are for traceability — consult them only if you need narrative rationale or prose color this contract intentionally omits.

# Tenebrae — open reader, writer, baker and runtime for Umbra 3.3.6 tomes

## Why

**Pain and vision.** A shipped game engine loads Umbra 3.3.6 visibility tomes (format version 18) every frame to cull occluded objects. Two things are proprietary:
- the only baker, Umbra's optimizer (shipped with Unity 5.1.1);
- the only query code, the engine's own runtime.

A consumer project that adds or changes map content needs new tomes and a way to query them. Tenebrae replaces both in pure Rust: it reads and writes tomes losslessly, bakes them from occluder geometry, and answers the frustum and portal queries natively. The code contains nothing derived from proprietary code and the repository holds no game data, so it can be published.

**Users.** The primary user is the consumer project, which links the facade crate. Secondary users are anyone who handles Umbra 3.3.6 tomes.

## Capabilities

- **CAP-1**
  - **intent:** Any Umbra 3.3.6 v18 tome can be read into a typed model and written back losslessly. The writer recomputes every derived structure.
  - **success:** Byte-identical round trip on 26/26 stock tomes and 4/4 Unity 5.1.1 bakes.
- **CAP-2**
  - **intent:** Exact segment occlusion against a triangle soup serves as the visibility ground truth.
  - **success:** The `truth` tool and the benchmark score visibility with it; 784k segments against 10 M triangles take 2.4 s.
- **CAP-3**
  - **intent:** A tome baked from occluder geometry gets correct answers from the engine's own runtime.
  - **success:** On the parity scene's 586 captured engine views, every query returns status 0 and no truly visible object is culled, for both presets: `tome_a()` (0.125 m, against stock A) and `tome_b()` (0.5 m without LOD tiles, against stock B's 3,422 objects; measured 2026-10-13, 430 kept per view vs stock B 360, 592,640 B vs stock B 881,168 B).
- **CAP-4**
  - **intent:** A bake costs no more than Umbra's in memory, time and tome size.
  - **success:**
    - Parity scene on 16 threads: 65-78 s at +200 MB over the loaded scene. Umbra takes 164-359 s at +1,577 MB.
    - Tome at defaults (LOD tiles on, which CAP-9 trades size for): 1,106,272 B against Umbra's 1,284,240 B. With LOD tiles off: 667,168 B against Umbra's 1,114,672 B with its inner LOD tiles stripped.
    - Benchmark: smaller than Umbra on 35 of 40 cases.
- **CAP-5**
  - **intent:** A re-bake reuses unchanged tiles from a cache.
  - **success:** A cached re-bake is byte-identical to a fresh bake. On the parity scene an unchanged re-bake takes 2.4 s (fresh 66 s) and one moved prop takes 4.9 s.
- **CAP-6**
  - **intent:** The consumer bakes through a versioned API (contract v2).
    - In: instanced or soup occluders, targets with user lists, one view volume.
    - Out: a tome, an object map, representative cell points, a report and cache bytes.
    - Progress reporting with cancel, and presets.
  - **success:**
    - Default bakes are byte-identical to the bakes before the API existed (M3c).
    - An instanced input bakes byte-identically to its applied soup.
    - With `keep_every_target`, object *i* is target *i*.
- **CAP-7**
  - **intent:** A tome can be queried natively for a camera's frustum and portal visibility, with no proprietary runtime.
  - **success:** Measured against the engine's native runtime on 1,029 captured views:
    - statuses and frustum lists are equal;
    - no truly visible object is culled on the benchmark truth;
    - objects kept are within +10 % of the engine's (measured: +3 to +6.5 %);
    - the median query takes at most 1.5× the engine's median (measured: 2.1×, still open).
- **CAP-8**
  - **intent:** A benchmark gates every change without anything from outside the repository.
    - Scenes: 8 generated scenes, Bistro (CC BY 4.0) and Sponza (CC BY 3.0), each at 8/16/32/64 m tiles.
    - Reference: Umbra's own numbers on the same scenes.
  - **success:** `cargo test --release -p tenebrae-bench` passes these gates: round trip, determinism, cache, instanced = soup, size against the baseline, and visibility against ray-cast truth.
- **CAP-9**
  - **intent:** A tome baked by Tenebrae costs no more to query in the engine's own runtime than Umbra's tome does.
  - **success:** Portal queries are 13 % faster than on Umbra's own tomes, and 11 % faster with LOD tiles. Objects kept rise by +0.2 % with no new false cull.

- **CAP-10**
  - **intent:** Tenebrae is published: source on GitHub, library crates on crates.io, built and released by GitHub Actions.
  - **success:**
    - CI runs fmt, clippy `-D warnings` and the tests on every push and pull request.
    - A release tag publishes the crates to crates.io from Actions.
    - A fresh project that depends on the facade crate from crates.io builds.

## Constraints

- **Pure libraries (AD-1).** `tenebrae-tome`, `-geom`, `-bake`, `-runtime` and the facade have no filesystem, network, process, GPU, environment or clock access, no `unsafe`, and no dependencies outside the workspace. File I/O lives in `tenebrae-tools`. Reason: the consumer's core crates ban I/O and threading crates.
- **Fixed dependency directions (AD-2).** `tome` and `geom` depend on nothing. `bake` depends on `tome` and `geom`, and `runtime` on `tome` only. The facade depends on all four, and `tools` and `bench` depend on the facade.
- **Clean-room runtime.** It is written from the format docs and the typed model only.
- **No game data in the repository (AD-4).** Tests read tomes from `TENEBRAE_TOMES` and pass vacuously without it.
- **Deterministic output (AD-5).** The same input and params produce identical bytes across runs, thread counts and cache states.
- **External acceptance (AD-4).** Changes are accepted by equality or parity with the engine's own runtime, through the external harness. They are never accepted by self-validation alone.
- **One synchronous call per view volume (AD-7).** Each call produces one tome. Threads exist only inside the call (`std::thread::scope`), and 1 thread spawns nothing.
- **Units and errors (AD-6, AD-9).** Coordinates are engine world space, metres, `f32`. Invalid input returns a typed `Err` and never panics.
- **Contract versioning (AD-10).** A breaking change to contract items is a new contract version named in the README. New `Params` or `Report` fields are not breaking.
- **Publishing is self-contained.** CI and releases need no game data and no external harness. CI runs only the gates that need nothing outside the repository; tests backed by `TENEBRAE_TOMES` pass vacuously there.
- **Toolchain and gates.** Toolchain pinned at 1.97.1, edition 2024, MSRV 1.95. `cargo fmt` and clippy with `-D warnings` gate every commit. License GPL-3.0-or-later.

## Non-goals

- Umbra versions or tome format versions other than 3.3.6 / v18.
- Encoding the engine's visibility record, its key table or `+0x30`. The consumer owns them (AD-8).
- Baker output with gates (openable portals) or plane BSP leaves. The model reads and writes them, but nothing bakes them.
- Several view volumes in one bake call.
- Any hosted service or GPU path. Publishing the crates (CAP-10) is in scope; running anything is not.
- Knowledge of engine tags or keys. Identity enters only through `Target::user`.

## Success signal

Two things hold, with no proprietary code and no game data in the repository:
- A tome that Tenebrae bakes from a map's occluders loads in the engine and answers the captured views with status 0 and no truly visible object culled, at or below Umbra's size, time and memory.
- Tenebrae's own runtime returns the same statuses and frustum lists as the engine on those views.

## Assumptions

- All sources keep the consumer project anonymous; this spec does the same.
- The ≤ 1.5× speed target stays part of CAP-7's success even though it is unmet. The README marks M6 "done, V1" with the target open.
