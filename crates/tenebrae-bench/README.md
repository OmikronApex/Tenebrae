# Tenebrae benchmark

A domain-agnostic benchmark: eight generated scenes and two real-world scenes (Amazon Lumberyard
Bistro, CC BY 4.0, and Crytek Sponza, CC BY 3.0), none with game data, each baked at four tile sizes
(8, 16, 32 and 64 m; a *case* is `scene.tT`). Every Tenebrae change is checked against it in the
repository:

- **Gates** (`tests/benchmark.rs`), which need nothing outside the repository:
  `cargo test --release -p tenebrae-bench` (a few minutes on 16 threads). Visibility (query status,
  objects kept, false culls) is scored with `tenebrae-runtime`, which returns a status and an
  object list on every view.
- **Umbra's numbers**, baked with Umbra 3.3.6's optimizer and scored in Umbra's own runtime by an
  external harness, are committed as `reference/umbra.txt` (numbers only, no tomes) and printed
  beside Tenebrae's by `report`.

Nothing here is derived from a game. The generated scenes are built from fixed seeds with plain `f32`
arithmetic (no `sin`, `cos` or other platform library calls), so every machine builds the same bytes;
`Scene::hash` covers them, and both tables record the hash and the generator version they were
measured on.

## Scenes

| Scene | What it stresses |
|---|---|
| `rooms` | 3×3 rooms of 32 m behind 6 m walls with doorways, walls off the tile seams: portal correctness |
| `city` | 6×6 blocks of 6-40 m buildings, 10 m streets, lamp posts as target-only props: strong occlusion, culling efficiency |
| `thin_walls` | walls 0 (one two-sided quad), 1/16, 1/8, 1/4 and 1/2 m thick, each whole or with a 0.1, 0.15, 0.2, 0.25 or 0.4 m slit off the voxel lattice: leaks, smallest hole and the band up to two voxels |
| `terrain` | 192 m of hills (closed underneath), rocks, trees with canopies: weak occlusion, large open domain |
| `maze` | sealed 12×12 maze of 4 m corridors under a ceiling: enclosed space, only seeds reach it |
| `seams` | walls on tile seams (one zero-thickness quad exactly on x = 32), boxes on tile corners, walls on and off voxel planes, objects on the domain boundary, a target past the view volume |
| `instanced` | 300 rotated, scaled copies of one mesh, as instances and as their soup (must bake identically) |
| `degenerate` | open boxes, coplanar overlaps, duplicate and zero-area triangles, slivers, flipped winding, non-manifold edges, 1 mm triangles, a huge triangle past the domain |
| `sponza` | Crytek Sponza: an atrium open to the sky, ringed by a ground-floor colonnade and two galleries, hanging fabric; 381 objects, 194,187 occluder triangles over 37 × 23 m; arcades and sight lines across levels |
| `bistro` | Amazon Lumberyard Bistro, exterior and interior in one space: 2,482 objects, 653,286 occluder triangles over 174 × 184 m; real-world density and object sizes, the bistro opening onto the street through its glass |

### The real-world assets

`assets/bistro.tbsc` (5.8 MB) and `assets/sponza.tbsc` (1.5 MB) are the only stored scenes. Each is
built once, outside the repository, and refined in place; nothing in the repository converts
them. Attribution, licence and every change from the original are in `assets/BISTRO.md` and
`assets/SPONZA.md`. The format (`src/tbsc.rs`): per object a target box,
then its occluder triangles with u16 positions over the object's own box, vertices numbered in
order of first use and indices as back-references, all LEB128 varints. `bench tbsc FILE` decodes one
and prints its counts.

Occluders are the opaque triangles; foliage cards, glass, string lights and water stay as
target-only objects, so the street sees into the bistro through its windows. Cameras: three in four
outdoors, 1.5 to 2.5 m above street level (z -0.5 to 1.5) where that is the first surface under the
open sky, which keeps them out of the closed building shells; one in four inside the bistro, 1.4 to
2.0 m above its floor. Sponza's cameras stand 1.4 to 2.0 m above a floor found by casting down
from just under the slab above: half on the ground floor (courtyard and colonnade), a quarter on
each gallery (about 150 / 28 / 21 come out, the galleries being narrow), never inside closed
geometry (odd crossings on all six axis rays). In both, the seeds are the first 40 eyes.

Each generated scene has 200 cameras (FOV 70°, 16:9, row-vector view-projection with D3D depth)
placed inside the view volume, 0.3 m clear of geometry and outside every solid. Targets are the
occluder objects' boxes plus target-only props; target `i` has user id `i`.

**Truth** (`src/truth.rs`): a target is truly visible when a sample point on its box (a 6×6 grid
on each face the eye is outside of) lies in the frustum and the segment from the eye to it crosses
no triangle of another object (Tenebrae's BVH). Umbra answers for object bounds, so the truth
samples the bounds too. Each camera has two truths:

- **strict**: through any gap;
- **through a hole**: the segment stays clear when moved by 0.0625 m either way along both axes
  across it, i.e. a sight line through an opening at least the smallest hole (`SH` 0.125 m) wide.

Both bakers close gaps under the smallest hole by design (the voxel is that size), so the strict
misses beyond the hole truth (`false_culls`) are the tolerance's cost, and a miss of the hole truth
(`false_culls_hole`) is an error, with one known exception: an opening between one and two voxels
wide that holds no whole voxel off the lattice closes in both bakers (`thin_walls`, generator 2), so
only openings of at least 2 SH are guaranteed open. The generated scenes barely differ between the two;
Bistro, full of small objects behind chair backs and railings, does (195 strictly visible objects
per view, 128 through a hole).

## Bake setting

`run::params(threads, T)`: `Params::tome_a()` (the validated parity setting) with `tile = T`
and `collapse_size = max(T, 16)`. `collapse_size = T` mirrors Umbra, whose tile size is its cluster
size (`CS`); the reference bakes Umbra at the parity parameters (`SO 6, BF 90, SH 0.125`, flags 4)
with `CS = T`. The 16 m floor applies at 8 m only. It was Tenebrae's substitute for Umbra's
inner-node LOD tiles, which keep small tiles within the runtime's traversal budget; since Tenebrae
writes them too (lever L5) collapse 8 passes as well, but the floor stays because it is faster and
smaller.

## Gates

Per case: the tables are current for the scene's hash, the bake succeeds, the tome round-trips
byte for byte, object `i` is target `i`, every camera eye finds a cell, no tile reaches the cell
count the runtime fails at (measured: 1,024 at 32 m, 256 at 16 m), and the tome is at most 10 %
larger than the baseline. At 32 m also: the same bytes at two thread counts, a cached bake and an
unchanged re-bake equal the plain bake (with no misses), and instanced equals soup.

Visibility, per case (`run::visibility`, the truth computed once per scene): every camera's portal
query returns status 0, `false_culls_hole` (the errors) is at most the baseline's, `kept` is at
most 10 % above the baseline's (the size gate's tolerance), and `frustum` equals the baseline's (it
depends only on the target boxes and the cameras). A case without baseline visibility keys skips
the last three. The keys are sums over the cameras of `status_ok`, `status_N` (queries with status
N, not scored, as the engine then falls back to frustum culling: 2 budget, 3 bad camera, 5 no
cell), `visible`, `kept`, `false_culls`, `false_views`, their `_hole` counterparts, and `frustum`
(the frustum-only query, every camera). Objects map to targets by their user ids. The cameras are
the harness's (`truth::camera`: the view-projection rounded to `f32`, radius 0, accuracy -1).

`tenebrae-runtime` uses plain `f32` arithmetic and a conservative rasteriser, so the visibility
keys are the same on every machine. Umbra's own runtime rounds with the CPU's `rcpps` and samples
pixel centres, so where the two disagree it is on objects at pixel edges, and the runtime keeps
them (see `docs/implementation-artifacts/spec-m6-native-runtime.md`).

A deliberate change of the numbers is accepted by rewriting the baseline:
`cargo run --release -p tenebrae-bench -- baseline`. `-- report` prints every metric against the
baseline and the Umbra reference, then the results table below (Tenebrae from its own runtime,
Umbra from `reference/umbra.txt`).

## Diagnostic examples

Not gates; tools for studying a bake (`cargo run --release -p tenebrae-bench --example NAME -- ARGS`):

- `sizes SCENE TILE [key=value ...]`: one case's bytes, bake time, cells, portals, view-tree
  nodes, object-run entries and global cells, with the setting overridden by keys (`block`,
  `fill`, `comp`, `merge`, `collapse`, `margin`), `vis=1` to score visibility, `TEN_OUT=FILE` to
  keep the tome.
- `treecmp TOME [--tiles]`: view-tree statistics of any tome, Tenebrae's or Umbra's, optionally
  per leaf tile, to compare two tomes tile by tile.
- `probe TOME [step]`: per height, the share of a lattice over the domain that finds a cell.
- `rtcount TOME CAMS.bin`: mean counters and timing of the runtime's portal query over `TBQC`
  cameras (the format `query` reads).

## Bake time

All 40 cases on 16 threads (2026-10-12): Tenebrae 72 s (best of two rounds of `baseline` with
`TENEBRAE_BENCH_THREADS=16`), Umbra 112 s (per-case optimizer times, the optimizer's default 16
threads, scene hand-over included). Tenebrae is faster on every scene (`thin_walls` 4.5×, `seams`
3.8×, `bistro` 1.2×) except `bistro.t64` (12.5 s against 7.0 s): its time grows with the tile while
Umbra's stays flat, which points at the cell merge's cost per tile (not profiled).

## Refreshing the Umbra reference

Needed only when the generator changes (bump `GENERATOR` with any change to a scene's bytes; the
tests then name both tables as stale). `cargo run --release -p tenebrae-bench -- export DIR` writes
per scene `DIR/<scene>/` with `scene.bin`, `targets.bin`, `seeds.bin`, `cameras.bin`, `scene.txt`
(formats in `src/scene.rs`), `truth.bin` (`truth::bytes`) and Tenebrae's tome per tile size. A
harness with Umbra 3.3.6 (Unity 5.1.1's optimizer and its runtime) bakes each scene at the setting
above, queries every camera, scores the answers against `truth.bin` with the keys above and writes
them as a `scene key value` table (`src/table.rs`) headed by the generator version; the result
replaces `reference/umbra.txt`. The same harness scoring Tenebrae's exported tomes in Umbra's
runtime equalled `bench`'s own visibility keys on all 40 cases, so the two runtimes score alike.

## Results at `GENERATOR` 2 (2026-10-12)

From `bench report`: Tenebrae from its own runtime, Umbra from `reference/umbra.txt`. Tenebrae
bakes with Umbra's cell pipeline (8 m components, lever L2 as Umbra's visibility-cost merge,
coalesced portals), the view-volume mask and export simplify and shared object-run windows. Sums
over each scene's 200 cameras. *Kept* and *visible* (strict truth) count status-0 queries only;
*frustum* is the frustum-only query (the no-occlusion ceiling). *False culls* are misses of the
strict truth, *through a hole* misses of the truth through an opening of at least 0.125 m (the
errors; see Truth above). The table was measured with Tenebrae's earlier bit-exact query; the
runtime keeps a few percent more on some cases (`baseline.txt` holds its current numbers).

| Case | Umbra B | Tenebrae B | Umbra kept | Tenebrae kept | Frustum | Visible | Tenebrae query fails | False culls U / T | Through a hole U / T |
| rooms.t8 | 329,936 | 42,336 | 2,210 | 2,250 | 6,250 | 1,573 | 0 | 0 / 0 | 0 / 0 |
| rooms.t16 | 64,208 | 16,896 | 2,229 | 2,272 | 6,250 | 1,573 | 0 | 0 / 0 | 0 / 0 |
| rooms.t32 | 24,848 | 14,336 | 3,703 | 2,193 | 6,250 | 1,573 | 0 | 0 / 0 | 0 / 0 |
| rooms.t64 | 12,496 | 7,504 | 3,698 | 2,203 | 6,250 | 1,573 | 0 | 0 / 0 | 0 / 0 |
| city.t8 | 1,735,424 | 385,136 | 3,030 | 3,014 | 10,640 | 2,114 | 0 | 0 / 0 | 0 / 0 |
| city.t16 | 597,312 | 226,288 | 2,903 | 2,827 | 10,640 | 2,114 | 0 | 0 / 0 | 0 / 0 |
| city.t32 | 220,112 | 120,688 | 2,832 | 3,297 | 10,640 | 2,114 | 0 | 0 / 0 | 0 / 0 |
| city.t64 | 110,064 | 124,480 | 2,792 | 2,795 | 10,640 | 2,114 | 0 | 0 / 0 | 0 / 0 |
| thin_walls.t8 | 529,088 | 87,200 | 4,994 | 4,657 | 5,392 | 2,548 | 0 | 6 / 0 | 2 / 0 |
| thin_walls.t16 | 103,424 | 42,352 | 5,086 | 4,864 | 5,392 | 2,548 | 0 | 6 / 0 | 2 / 0 |
| thin_walls.t32 | 39,984 | 16,496 | 5,135 | 4,959 | 5,392 | 2,548 | 0 | 6 / 0 | 2 / 0 |
| thin_walls.t64 | 20,688 | 10,208 | 4,856 | 5,185 | 5,392 | 2,548 | 0 | 6 / 0 | 2 / 0 |
| terrain.t8 | 1,678,096 | 306,832 | 8,252 | 7,561 | 11,721 | 5,032 | 0 | 0 / 0 | 0 / 0 |
| terrain.t16 | 417,280 | 149,312 | 8,428 | 7,945 | 11,721 | 5,032 | 0 | 0 / 0 | 0 / 0 |
| terrain.t32 | 121,168 | 40,320 | 8,781 | 8,472 | 11,721 | 5,032 | 0 | 0 / 0 | 0 / 0 |
| terrain.t64 | 91,952 | 56,288 | 8,700 | 8,287 | 11,721 | 5,032 | 0 | 0 / 0 | 0 / 0 |
| maze.t8 | 40,144 | 15,984 | 1,856 | 2,036 | 10,277 | 1,139 | 0 | 0 / 0 | 0 / 0 |
| maze.t16 | 18,352 | 11,968 | 2,855 | 2,198 | 10,277 | 1,139 | 0 | 0 / 0 | 0 / 0 |
| maze.t32 | 9,152 | 9,712 | 3,806 | 2,265 | 10,277 | 1,139 | 0 | 0 / 0 | 0 / 0 |
| maze.t64 | 7,232 | 8,880 | 4,791 | 2,258 | 10,277 | 1,139 | 0 | 0 / 0 | 0 / 0 |
| seams.t8 | 543,200 | 80,688 | 2,829 | 3,203 | 6,042 | 2,213 | 0 | 0 / 0 | 0 / 0 |
| seams.t16 | 129,952 | 27,792 | 3,109 | 3,226 | 6,042 | 2,213 | 0 | 0 / 0 | 0 / 0 |
| seams.t32 | 63,216 | 10,000 | 3,087 | 3,232 | 6,042 | 2,213 | 0 | 0 / 0 | 0 / 0 |
| seams.t64 | 34,128 | 9,872 | 5,115 | 3,250 | 6,042 | 2,213 | 0 | 0 / 0 | 0 / 0 |
| instanced.t8 | 793,504 | 108,192 | 20,769 | 20,756 | 20,772 | 2,060 | 0 | 0 / 0 | 0 / 0 |
| instanced.t16 | 142,672 | 52,000 | 20,771 | 20,771 | 20,772 | 2,060 | 0 | 0 / 0 | 0 / 0 |
| instanced.t32 | 45,504 | 32,592 | 20,771 | 20,771 | 20,772 | 2,060 | 0 | 0 / 0 | 0 / 0 |
| instanced.t64 | 31,632 | 37,648 | 20,770 | 20,771 | 20,772 | 2,060 | 0 | 0 / 0 | 0 / 0 |
| degenerate.t8 | 614,912 | 52,896 | 2,777 | 2,825 | 2,892 | 1,477 | 0 | 0 / 0 | 0 / 0 |
| degenerate.t16 | 189,824 | 18,464 | 2,789 | 2,836 | 2,892 | 1,477 | 0 | 0 / 0 | 0 / 0 |
| degenerate.t32 | 84,752 | 9,968 | 2,751 | 2,836 | 2,892 | 1,477 | 0 | 0 / 0 | 0 / 0 |
| degenerate.t64 | 69,344 | 6,064 | 2,737 | 2,825 | 2,892 | 1,477 | 0 | 0 / 0 | 0 / 0 |
| bistro.t8 | 767,696 | 271,920 | 105,663 | 115,101 | 127,587 | 39,047 | 0 | 323 / 1 | 190 / 0 |
| bistro.t16 | 389,344 | 247,648 | 112,975 | 114,713 | 127,382 | 39,047 | 0 | 150 / 1 | 25 / 0 |
| bistro.t32 | 286,512 | 224,320 | 113,006 | 111,092 | 127,382 | 39,047 | 0 | 150 / 1 | 25 / 0 |
| bistro.t64 | 223,888 | 207,984 | 112,991 | 112,143 | 127,382 | 39,047 | 0 | 150 / 1 | 25 / 0 |
| sponza.t8 | 82,736 | 43,312 | 9,215 | 10,312 | 17,210 | 3,829 | 0 | 77 / 0 | 25 / 0 |
| sponza.t16 | 33,152 | 28,880 | 9,339 | 10,637 | 17,210 | 3,829 | 0 | 63 / 0 | 17 / 0 |
| sponza.t32 | 31,552 | 25,296 | 9,549 | 10,637 | 17,211 | 3,829 | 0 | 45 / 0 | 12 / 0 |
| sponza.t64 | 28,272 | 31,088 | 9,550 | 10,637 | 17,211 | 3,829 | 0 | 44 / 0 | 12 / 0 |

Readings:

- **Every query is status 0 and no case has a through-a-hole miss**; Umbra misses 190 / 25 / 25 / 25
  on Bistro, 12 to 25 on Sponza and 2 on `thin_walls`. Tenebrae's only strict miss left is one on
  Bistro.
- **Size:** Tenebrae is smaller than Umbra on every case but `instanced.t64`, `maze.t32/t64`,
  `sponza.t64` and `city.t64`, and is 227 KB against Umbra's 287 KB on `bistro.t32`.
- **Culling at 32 m** is within a few percent of Umbra's on `bistro`, `seams`, `terrain`,
  `degenerate` and `instanced`, better on `rooms`, `maze` and `thin_walls`, and worse on `city`
  (3,297 against 2,832: targets past the domain are listed in every boundary cell, which
  `Params::domain_margin` removes at +12 % bytes) and `sponza` (10,637 against 9,549: with 6 m
  blocks Tenebrae keeps 9,611; Umbra's 8 m blocks sit on its scene-bounds grid, not on the view
  volume's).
- **8 m tiles bake with a 16 m collapse** (Bake setting above): every query is status 0.
- `instanced` is an open layout that a portal-cell system cannot cull much (both bakers keep
  about all of the frustum); it is there for the input path.
