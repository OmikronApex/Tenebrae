<!-- bmad:context -->
<!-- Verified 2026-10-13 against 3d4a72f. Managed by bmad-project-context; edits inside this block are replaced on refresh. Keep anything you want preserved outside the markers. -->

## Tenebrae

Rust reader, writer, baker and runtime for Umbra 3.3.6 visibility tomes (format version 18). A Cargo workspace under `crates/`; each crate's role is commented on its member line in the root `Cargo.toml`. Milestones and status are in `README.md`, product-level specs (`bmad-spec`: what to build) in `docs/specs/`, milestone implementation specs (`bmad-build`: how) in `docs/implementation-artifacts/`, and the architecture spine (`AD-n` decisions) in `docs/planning-artifacts/architecture/`.

## Policy

- Never commit to `main`; branch off it and land work through a pull request.
- Keep the library crates pure (`tenebrae-tome`, `-geom`, `-bake`, `-runtime`, `tenebrae`): no filesystem, network, process or GPU access, no dependencies outside the workspace, no `unsafe`. File I/O goes in `tenebrae-tools`.
- Dependency directions: `tome` and `geom` depend on nothing; `bake` on tome and geom; `runtime` on tome only; the facade on all four; `bench` on the facade; `tools` on the facade and `bench`. Nothing else depends on `bake` or `runtime`.
- Write `tenebrae-runtime` from the format docs and the typed model only; no code derived from a proprietary runtime.
- Never commit tomes or other game-derived data; tests read them from the directories named in `TENEBRAE_TOMES` (`;`-separated).
- Contract v2 (`README.md`) names the public items; a breaking change to them is a new contract version.
- Never edit the frozen intent block of an implementation spec; the human owns it.
- Disclose agent authorship in commits with `Co-Authored-By:` trailers.

## Where things are

- Larger work starts with a product-level `bmad-spec` (`docs/specs/<id>/SPEC.md`: capabilities, constraints, non-goals). Non-trivial work starts from an implementation spec in `docs/implementation-artifacts/` (`bmad-build` format: frozen intent, boundaries, edge-case matrix, tasks and acceptance criteria, implementation notes, change log, review triage log).
- Architecture decisions go into the spine as `AD-n` entries with binds / prevents / rule; date amendments.
- Record left-out work with evidence in `docs/implementation-artifacts/deferred-work.md`; agents' working logs are `.memlog.md` files beside the artifact they belong to.
- Triage reviews (`bmad-code-review`, `bmad-review`) into the spec's review log: finding, verdict, route, evidence.
- The BMAD install (`.agents/`, `.claude/`, `_bmad/`) is per developer and gitignored; `CONTRIBUTING.md` says how to set it up.

## Running and verifying

- Lint with `cargo clippy --workspace --all-targets -- -D warnings`; without `--workspace` only the default members are checked and `tenebrae-bench` is skipped.
- Format-check with `cargo fmt --all -- --check`. Both gates run in `.githooks/pre-commit`; enable it once per clone with `git config core.hooksPath .githooks`.
- `cargo test --release` at the root runs every crate but the benchmark (default members); run `cargo test --release -p tenebrae-bench` (a few minutes) as well before committing any change to baking or querying.
- Run the stock-tome round trip with `cargo test --release -p tenebrae-tome` and `TENEBRAE_TOMES` set; without it the tome tests pass vacuously.
- Build or run tool binaries with `-p tenebrae-tools` (e.g. `cargo build --release -p tenebrae-tools`).
- Acceptance against the engine's own runtime (captured views, native timing) needs an external harness that is not part of this repository; the specs record its results as measured.

## Conventions that differ from defaults

- Keep bakes byte-identical across thread counts and with or without a tile cache; the benchmark checks this only at 32 m, so a change can break it elsewhere unnoticed.

## Known pitfalls

- Do not skip the benchmark or quietly move its numbers: a deliberate change needs `cargo run --release -p tenebrae-bench -- baseline` and a commit message that says so; a change to a generated scene needs `GENERATOR` bumped and the Umbra reference regenerated (`crates/tenebrae-bench/README.md`).
- Change the pinned hashes (`l4_off_bakes_are_unchanged`, `cache_bytes_are_pinned_to_the_cache_version`) only with an intended tome or cache change; regenerate them from the test's own `left:` value and say so in the commit.

<!-- /bmad:context -->
