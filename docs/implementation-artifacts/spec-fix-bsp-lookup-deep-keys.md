---
title: 'Plane-BSP point lookup and wide top-tree node keys'
type: 'bugfix'
created: '2026-10-13'
status: 'done'
baseline_commit: 'f1a04ecb1413736f2a7c1634f904bd9b2d36127e'
route: 'dispatch'
review_loop_iteration: 0
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Two defects block a consumer from replacing its own tome reader with Tenebrae. (1) `find_cell` answers `None` for any point in a view-tree leaf refined by a plane BSP, so 26,991 of 366,141 light-probe lookups on the installed game come back empty where the engine finds a cell. (2) `write` panics (`write.rs:96`, shift overflow in `node_keys`) on 4 of 723 installed tomes whose top tree is 33 levels deep, so they cannot round-trip.

**Approach:** Follow the plane BSP in the point lookup, with one walk in `tenebrae_tome::locate` used by `find_cell`, `Locator` and the runtime's camera-cell lookup. Write top-tree node keys of any width: the stock deep tomes store 34-bit keys (`+0x94` = 34) with the same path encoding, so the encoding is unchanged and only the u32 limit goes.

## Boundaries & Constraints

**Always:**
- BSP walk, from the model's own fields: a leaf value with its top bit set (and not all ones) names BSP node `value & (all_ones >> 1)`. Node `n` (u64): `lo = n as u32`, plane `lo & 0x3FFF_FFFF`; `d = p·plane.xyz + plane.w` in f32. When `d >= 0`, go to `(n >> 48) & 0xFFFF`, which is a local cell if `lo` bit 31 is set. Otherwise go to `(n >> 32) & 0xFFFF`, which is a local cell if bit 30 is set. A local cell `0xFFFF` is no cell. The resulting local cell resolves exactly like a direct leaf value.
- The lookup never panics on a model `read` accepts: missing node or plane → `None`; a walk longer than 65,536 steps (a looping BSP) → `None`; `leaf_bits == 0` → `None`.
- Keep `find_cell` ≡ `Locator::find_cell` ≡ `Runtime::find_cell` on every point.
- Every stock tome round-trips byte for byte, including the four deep ones; every existing bake stays byte-identical (pinned hashes and bench baseline unchanged).
- Contract v2 signatures unchanged; document the BSP fields on `Bsp` / `ViewTree::leaf_values`.

**Never:**
- No change to `read`, the tome layout, or the baker (it emits no BSP).
- No tome or other game data committed; stock checks run through `TENEBRAE_TOMES`.
- `cell_regions` keeps skipping BSP leaves (its boxes are exact only for non-BSP leaves); document it, don't change it.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Direct leaf | leaf value < top bit | tile cell word 4 when word 5 = 0, as today | `None` past the cell table |
| BSP front/back | leaf value = top bit \| r | walk to a local cell, then as a direct leaf | — |
| BSP names 0xFFFF | walk ends on `0xFFFF` | `None` | — |
| Broken BSP | node or plane index out of range, or the walk loops | `None` | no panic |
| `leaf_bits` 0 | view tree with 0-bit values | `None` | no underflow |
| Deep top tree | top depth 33 (stock) or ≥ 64 (synthetic) | keys written at width depth + 1; stock bytes identical | no panic at any depth |

</frozen-after-approval>

## Code Map

- `crates/tenebrae-tome/src/locate.rs` -- `find_cell` (line ~90), `leaf_cell`, `Locator::find_cell`: route the leaf value through a new `pub fn local_cell(view: &ViewTree, bsp: Option<&Bsp>, ordinal, p) -> Option<u32>` (leaf decode + BSP walk); `leaf_cell` becomes local → `global_of`. `cell_regions` keeps its own skip; update its doc.
- `crates/tenebrae-tome/src/lib.rs:131-149` -- `ViewTree::leaf_values` and `Bsp` docs: state the node/plane encoding above.
- `crates/tenebrae-runtime/src/decode.rs:84-108` -- `ViewData` gains `bsp: Option<Bsp>` (cloned from `Tile::bsp` at ~453); `ViewData::local_cell` calls `locate::local_cell`. Docs at 95 and 493 drop "refined by a plane BSP". `Runtime::local_cell` keeps its `< cells.len()` check. Runtime may use `tenebrae_tome::locate` (it already imports `leaf_ranked`, `ranks`).
- `crates/tenebrae-tome/src/write.rs:92-112` -- `node_keys`: keep `depths`; drop u32 keys. Emit each key's bits straight into the packed words: at depth `d` the marker 1 sits at bit `W-1-d`, and bit `W-1-d+j` (j = 1..d) is step j from the root (1 = below the split). `W = max depth + 1` stays in `+0x94`. Today's `bits::pack(keys, key_bits, …)` call at ~line 20 is replaced.
- `crates/tenebrae-tome/src/bits.rs` -- reuse `packed_words`; add a single-bit setter only if it reads cleaner than inline.
- `crates/tenebrae-tome/tests/roundtrip.rs` -- already checks Locator ≡ find_cell on stock tomes; no structural change needed.
- `deferred-work.md:93`, `ARCHITECTURE-SPINE.md:260`, `SPEC.md:97`, `spec-m6-native-runtime.md` -- wording says BSP leaves resolve "as `locate` does" / are unexercised: amend to say lookups follow them (baker still never emits them).

## Tasks & Acceptance

**Execution:**
- [x] `crates/tenebrae-tome/src/locate.rs` -- add `local_cell` with the BSP walk; use it in `find_cell`, `Locator`; unit tests for every matrix row (front, back, nested, 0xFFFF, out-of-range node/plane, loop, `leaf_bits` 0).
- [x] `crates/tenebrae-tome/src/lib.rs` -- document the BSP encoding.
- [x] `crates/tenebrae-runtime/src/decode.rs` -- carry the BSP, delegate, update docs; test that `Runtime::find_cell` equals `locate::find_cell` on a BSP tile.
- [x] `crates/tenebrae-tome/src/write.rs` -- arbitrary-width node keys; tests: a 33-deep and a 70-deep chain top tree write without panic, read back equal, and keys match the old u32 path for depth < 32 (byte identity of a shallow tome).
- [x] Docs listed in the Code Map -- amend the BSP wording.

**Acceptance Criteria:**
- Given all 723 installed tomes in `TENEBRAE_TOMES`, when `cargo test --release -p tenebrae-tome` runs, then every tome round-trips byte for byte (none panic).
- Given the same tomes, when a scratch harness (never committed) compares `find_cell` against the engine-faithful reference lookup on a 24³ grid per tome domain, then every point agrees (previously 65,587 of 95,800,320 differed).
- Given the workspace, when the gates in Verification run, then all pass with pinned hashes and the bench baseline unchanged.

## Implementation Notes

- `node_keys` builds each key bit by bit in the packed words: a child copies its parent's field shifted down one bit and sets the top bit when below the split. That is the old u32 recurrence verbatim, so any tree under 33 levels writes the same bytes (unit test against the old function on chains of depth 0-31 and ragged trees).
- The runtime's `ViewData` now holds clones of the tile's `ViewTree` and `Bsp` and delegates to `locate::local_cell`; `Runtime::local_cell` keeps its `< cells.len()` check.
- Test fixtures (`crates/tenebrae-tome/src/fixtures.rs`, `#[cfg(test)]`): a one-tile tome with a two-node BSP, and chain top trees. The runtime test carries its own copy of the BSP tome (test code cannot cross crates).
- `cell_regions` gained a `leaf_bits == 0` skip (it would underflow); its BSP skip is unchanged and now documented.
- 2026-10-13, 723 installed tomes: round trip 723/723 byte for byte, the four 33-deep ones included. Scratch reference harness (`gridcmp`, not committed), 24³ grid per domain: 9,994,752 points, 0 differ between the engine-faithful reference and `find_cell`, `Locator` and `Runtime::find_cell`.
- `stock_tomes_answer_queries` fails on `80c3d892.tome` ("no lattice point in a cell") at the baseline too: its open cells are thin slabs the test's 24³ lattice misses, and no BSP is involved. Fixed afterwards by the orchestrator: the per-tome `found > 0` assertion became `found + centres > 0`, where every `cell_regions` box centre must find its cell through `Locator` (a stronger check of the lookup than a lattice hit, and true for thin-slab domains). The lattice equality `Locator ≡ find_cell` is kept.
- Baseline figures on the same 24³ grid (723 tomes, 9,994,752 points): 29,910 points differed at `f1a04ec`, all `None` where the reference finds a cell, over 406 tomes; 0 differ after the change. The Intent's 26,991 / 366,141 is the light-probe population, and the AC's 65,587 / 95,800,320 a grid over each tome once per package that pairs it; neither was re-measured here.

- After review patches (2026-10-13): fmt and clippy clean; `cargo test --release` pass; `cargo test --release -p tenebrae-bench` 11 passed, baseline unchanged; `TENEBRAE_TOMES` (723 tomes) `cargo test --release -p tenebrae-tome` pass in 124 s; reference grid 9,994,752 / 9,994,752.

## Spec Change Log

## Review Triage Log

| # | Finding | Verdict | Route | Evidence |
|---|---------|---------|-------|----------|
| 1 | Leaf value wider than `leaf_bits` is read as a BSP root (edge) | low | reject | `read` masks every field to `leaf_bits`; only a hand-built model reaches it, and the fix adds a guard. |
| 2 | Fixed 65,536-step walk bound: a loop costs 65,536 plane tests, and an acyclic walk from a root > 65535 can need 65,537 steps (edge, blind) | low | patch | Real by construction; bounding by `nodes.len()` is exact (an acyclic walk visits each node once) and still returns `None` on a loop. |
| 3 | Spec notes say `roundtrip.rs` unchanged while the diff changes it (edge, blind, verification) | low | reject | Fix edits this spec; Implementation Notes updated anyway. The loosening claim is false: lattice equality is kept and the region-centre check is stronger. |
| 4 | Acceptance figures don't reconcile (blind) | low | reject | Fix edits this spec; the populations are now explained in Implementation Notes. |
| 5 | Leaf decoding duplicated; `cell_regions` shift overflows for `leaf_bits > 32` (blind) | low | reject | Pre-existing in `cell_regions`; `read` yields `leaf_bits ≤ 31` (`w[6] & 31`); only hand-built models reach it. |
| 6 | Untested boundaries: `leaf_bits` 32 / 1, a non-cell step to `0xFFFF`, NaN plane (blind) | low | reject | 32 is unreachable from `read`; a step to `0xFFFF` hits a missing node → `None`; NaN takes the back branch as the reference does (`d >= 0` false). |
| 7 | Deep-key tests check one node; `read` ignores `+0x98` (blind) | medium | patch | Verified: `read` never reads keys, so a wrong layout above 32 levels would round-trip; byte proof rests on 4 local tomes only. |
| 8 | Runtime test duplicates the BSP fixture (blind) | low | reject | Drift possible but harmless to users; sharing needs a new feature or public module. |
| 9 | New public `local_cell` and the BSP behaviour change are not in Contract v2 (blind) | low | patch | README lists `query::` items and omits `local_cell`; an addition is not breaking. |
| 10 | Cells reached only through BSP leaves get no region boxes (blind) | low | reject | Intent keeps `cell_regions` skipping BSP leaves (Never). |
| 11 | `deferred-work.md` M6 summary still lists BSP leaves as open (blind) | low | patch | Summary contradicts its own amendment line. |
| 12 | No runtime test for a BSP result past the cell table (blind, verification) | medium | patch | Only guard is `< tile.cells.len()` in `Runtime::local_cell`; `find_cell` indexes unchecked behind it; no test produces an out-of-range cell. |
| 13 | No committed stock-tome test can see the BSP walk regress (verification) | medium (unverified impact) | defer | Both lattice sides share `local_cell`, region centres skip BSP leaves; engine agreement needs the external harness by design. |

## Verification

**Commands:**
- `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` -- expected: clean.
- `cargo test --release` -- expected: pass.
- `TENEBRAE_TOMES=<723-tome dir> cargo test --release -p tenebrae-tome` -- expected: pass, round trip 723/723.
- `cargo test --release -p tenebrae-bench` -- expected: pass, baseline unchanged.
