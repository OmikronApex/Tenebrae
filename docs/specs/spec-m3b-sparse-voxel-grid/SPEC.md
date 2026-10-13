---
id: SPEC-m3b-sparse-voxel-grid
status: final
companions:
  - ../../planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md
  - the m3b-umbra-global-stage research answers (external)
  - the format research notes (external)
sources:
  - ../../../README.md
  - the m3b-umbra-global-stage research request (external)
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete, preservation-validated contract for what to build, test, and validate. Source documents listed in frontmatter are for traceability — consult them only if you need narrative rationale or prose color this contract intentionally omits.

# M3b — Sparse grid, parallel tiles, and Umbra parity

## Why

**Pain.** The baker's dense voxel grid needs about 4 GB at 0.25 m on the parity scene's *reduced* domain (the format research notes §6g). That blocks two things:
- Umbra's own resolution: 0.125 m holes, which halve sliver errors again.
- Full-size domains, which every real the consumer map needs.

**What Umbra does** (research handover, the answer companion):
- It never allocates a dense grid. Each tile gets a pointer-free octree, and only nodes that triangles pass through reach hole level (0.125 m).
- It bakes tiles on 16 threads.
- Its small tome comes from per-tile cell merging plus reachability. Leaf-tile collapse is a no-op at its 32 m tiles.

**Goal.** Parity with Umbra 3.3.6 on the parity scene in quality, peak memory, tome size, and tile-stage parallelism.

## Capabilities

- **CAP-1**
  - **intent:** Bake memory scales with the blocks that geometry touches, not with domain volume. This covers every per-voxel structure (surface, solid, cell ownership), and a uniform block costs one value.
  - **success:** A parity-scene bake with explicit 0.25 m `Params` (terrain scene, the evaluation harness) produces tome and samples bytes identical to the baseline baker at the M3 dense baker, at a lower peak memory.
- **CAP-2**
  - **intent:** The parity scene bakes at 0.125 m voxels on Umbra's baseline setup (*Parity setup*, below) within Umbra's memory.
  - **success:**
    - Peak memory over the loaded scene is at or below Umbra's, measured by the handover's Q4 method: working set and private bytes, peak minus the value after scene load.
    - At 1 thread: ≤ +322 MB working set and ≤ +334 MB private.
    - At 16 threads: ≤ +1,577 MB working set and ≤ +1,723 MB private.
    - the external 586-view evaluation reports 0 wrongly culled objects and status 0 for every query.
- **CAP-3**
  - **intent:** A grid that would exceed the memory limit is refused before anything is allocated, and every bake states its grid storage.
  - **success:**
    - A domain/voxel/thread-count combination over the limit returns a typed `bake::Error` without allocating it. The limit accounts for tiles in flight.
    - `Report` carries the grid's storage bytes.
- **CAP-4**
  - **intent:** Invalid input returns an error instead of panicking or producing a silently wrong bake (spine AD-6, AD-9).
  - **success:** Unit tests confirm each of the following returns `Err` and never panics:
    - out-of-range triangle indices;
    - `owners.len() != triangles.len()`;
    - non-finite vertices;
    - boxes with `min > max` or non-finite values;
    - grid-size overflow.

    Also, `bake::Error` implements `Display` and `std::error::Error`, and `tome::Error::Version` has a description.
- **CAP-5**
  - **intent:** `Params::default()` bakes at 0.125 m voxels, for quality parity with Umbra.
  - **success:**
    - After CAP-2 holds, a default bake equals an explicit 0.125 m bake.
    - Block and tile extents keep their metre sizes (6 m blocks, 16 m tiles).
    - the external research reports 0 wrongly culled objects and status 0 for every query.
- **CAP-6**
  - **intent:** Unreachable cells are removed before cells are numbered, using Umbra's global-cluster rule (handover Q2, pass 2):
    - only cells that overlap the view volume, or that reach it through portals, take part;
    - components run over in-tile portals and cross-tile links;
    - a component is kept when it has at least `threshold` × the largest component's cell count (`Params`, default 0.1);
    - a tile left with no cells stays as a 0-cell leaf;
    - a target whose cells were all removed is dropped from the tome (spine AD-8).
  - **success:**
    - Every output cell belongs to a kept component.
    - The object map gives `None` for exactly the dropped targets.
    - the external research reports 0 wrongly culled objects and status 0 for every query.
- **CAP-7**
  - **intent:** Leaf tiles collapse by Umbra's rule (handover Q1), applied to Tenebrae's 16 m tiles after CAP-6:
    - post-order over the top tree, two sibling leaves merge when their union's largest extent is ≤ CS (`Params`, 32 m) and their combined cell count is < 96;
    - the merged tile runs a seam merge of cell pairs across the shared plane, at threshold 0.5 × block size;
    - merges cascade upward.
  - **success:**
    - On the parity setup at 0.125 m, the parity scene tome is ≤ 1,114,672 B (Umbra's tome with `strip-inner`).
    - the external research reports 0 wrongly culled objects and status 0 for every query.
- **CAP-8**
  - **intent:** The per-tile stage runs on a worker pool. The thread count is a `Params` field, default 16, and 1 thread spawns nothing (spine AD-7). The global stage (cross-tile links, reachability, collapse, numbering) stays serial.
  - **success:**
    - Tome, samples, and object map are byte-identical at 1 and 16 threads, on the CAP-1 and CAP-2 bakes.
    - Peak memory is reported at both thread counts against CAP-2's two rows.

## Parity setup

Umbra's baseline bake (handover Q4), which CAP-2 and CAP-7 reproduce:
- Occluders: the non-terrain statics scene (the parity scene).
- Domain and view volume: (−192, −160, −672)..(160, 192, 608).
- Targets: the 3,385 stock object boxes.

## Constraints

- Spine AD-1: no dependencies and no `unsafe`. The sparse structure and the worker pool are hand-written on std.
- Spine AD-5: output stays deterministic across runs and thread counts. Each tile writes its own slot, and results join in tile-index order. Nothing may depend on hash iteration, allocation, or completion order.
- Spine AD-7 (amended): one sync call. Threads exist only inside the call, and 1 thread runs entirely on the caller's thread.
- Spine AD-8 (amended): surviving targets keep input order. The object map is `Option<object>` per target and per bake.
- Spine AD-11: every index a bake returns, including `Report::cell_samples`, refers to the tome after CAP-6 and CAP-7's compaction.
- `Report::cell_samples` keeps its meaning: per global cell, centres of open voxels. The consumer's probe and bitset tables vote from them.
- Spine AD-12: the `bake` and `diagnose` CLI arguments and blob layouts stay append-only. New `Params` fields (threads, reachability threshold, CS) get CLI arguments whose defaults equal `Params::default()`.
- Spine AD-4: acceptance goes through the engine's native runtime and truth tools. CAP-6 and CAP-7 change bytes, so their only oracle is the external evaluation.
- Order of work: CAP-1 lands first, byte-identical, before anything that changes output. CAP-8 is checked against the 1-thread bake.

## Non-goals

- Inner-node LOD tiles. They are optional per §6f.
- Progress hooks, cancellation, and the rest of the M4 consumer-facing API.
- Changing the existing cell, merge, flood, or view-tree algorithms. That includes replacing flood fill with Umbra's backface ray tests. The seam merge in CAP-7 is the only new merge step.
- Stripping global portals. The engine's audio reads them (§6h).
- M3c, the incremental re-bake with a tile cache. It is a separate spec (spine Deferred).
- Umbra 3.3.16 changes and flag-16 object depth maps (handover: no gain for the 3.3.6 runtime).

## Success signal

- **0.25 m regression check:** before and after CAP-1, a 0.25 m the parity scene bake produces byte-identical output at a lower peak memory.
- **Thread check:** 1-thread and 16-thread bakes are byte-identical.
- **0.125 m parity:** on the parity setup with default `Params`, the following all hold:
  - peak memory at or below Umbra's at both thread counts;
  - tome ≤ 1,114,672 B;
  - the external 586-view evaluation reports 0 wrongly culled objects and status 0 for every query.

## Assumptions

- The sparse grid only changes storage. Flood, components, best-first merge, view tree, and solid-voxel ownership are unchanged, so byte identity at 0.25 m is a valid regression oracle for CAP-1.
- The seam merge uses Tenebrae's existing merge cost, restricted to cell pairs across the shared plane, with threshold (0.5 × block)². Umbra's metric is only inferred. The size target and the external research decide.
- If CAP-6 plus CAP-7 miss 1,114,672 B, the next lever is Umbra's per-tile cell merging (modes 1 and 2, threshold SO). Adding it is a spec update.
- Buried targets, which overlap no cell, are kept. Only targets whose cells CAP-6 removed are dropped.
- Tenebrae's flood fill makes interiors solid, while Umbra keeps them as cells and filters them in reachability. The parity comparison accepts that difference.

## Open Questions

None block M3b. Deferred to M4 (spine): a per-bake object map, against a `+0x30` visibility record that tome A and tome B may share.
