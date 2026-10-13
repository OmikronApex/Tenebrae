---
title: 'Release to crates.io on a version tag'
type: 'chore'
created: '2026-10-13'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: 'f8e0ab31ca8ac2dee24cd223b1b184ef6c033f10'
context:
  - '{project-root}/docs/planning-artifacts/architecture/architecture-Tenebrae-2026-10-11/ARCHITECTURE-SPINE.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Nothing publishes Tenebrae: every crate has `publish = false`, the manifests lack the metadata crates.io requires, and there is no release workflow. AD-14 fixes how a release must work.

**Approach:** Pushing a tag `vX.Y.Z` on a commit of `main` runs a GitHub Actions workflow that reruns the CI gates on that commit, then publishes the five library crates in dependency order through crates.io trusted publishing. The manifests become publishable under AD-14 (lockstep version, exact pins, metadata).

## Boundaries & Constraints

**Always:**
- One workspace version; each `[workspace.dependencies]` entry carries `path` and `version = "=X.Y.Z"` (AD-14).
- Publish only `tenebrae-tome`, `tenebrae-geom`, `tenebrae-bake`, `tenebrae-runtime`, `tenebrae`; `tenebrae-tools` and `tenebrae-bench` keep `publish = false`.
- Every published crate has `license`, `description`, `repository`, `readme`, `keywords`, `categories`.
- The release refuses a tag whose commit is not on `main` or whose version differs from the workspace version, and publishes only after the gates pass on the tagged commit.
- Authenticate with `rust-lang/crates-io-auth-action` (job permission `id-token: write`); no long-lived crates.io token in repository secrets. Actions in the new workflow are pinned by full commit SHA with the version in a comment.
- The first publish of each crate is manual (trusted publishing needs the crate to exist); document it.
- Decision (user, 2026-10-13): the publish job runs in a GitHub environment `release` with the owner as required reviewer, so every release waits for approval on the run page; the crates.io trusted publishers are bound to that environment.
- Decision (user, 2026-10-13, renegotiated after review): drop `Counters::stage_ns` and the clock that fills it from `tenebrae-runtime` in this change, so the first publish satisfies AD-1 (AD-10 allows the removal only before it).
- Decision (user, 2026-10-13): after publishing, the workflow creates a GitHub Release for the tag whose notes are the annotated tag's message; release tags must therefore be annotated.

**Never:**
- Upgrading CI to AD-15's matrix, MSRV job, semver checks, or re-pinning `ci.yml`'s existing actions (deferred).
- Pushing a tag or publishing anything as part of this work.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Release | tag `v0.1.0` on a `main` commit, workspace version 0.1.0 | gates pass, five crates published in order | N/A |
| Off-main tag | tag on a commit not reachable from `main` | no gates, no publish | job fails naming the reason |
| Version mismatch | tag `v0.2.0`, workspace 0.1.0 | no publish | job fails naming both versions |
| Gate failure | any gate fails on the tagged commit | no publish | release run fails |
| Lightweight tag | `git tag v0.1.0` without `-a` | no gates, no publish | job fails asking for an annotated tag |
| Approval declined | reviewer rejects the `release` deployment | nothing published, no GitHub Release | run ends rejected |
| Non-version tag | tag `test` or `v1` | workflow does not start | N/A |

</frozen-after-approval>

## Code Map

- `Cargo.toml` -- `[workspace.package]` (version 0.1.0, license) and `[workspace.dependencies]` (path only): add shared metadata and `=0.1.0` pins here.
- `crates/{tenebrae-tome,tenebrae-geom,tenebrae-bake,tenebrae-runtime,tenebrae}/Cargo.toml` -- `publish = false` to remove; inherit the new metadata with `.workspace = true`; keep each crate's own `description`.
- `crates/tenebrae-tools/Cargo.toml`, `crates/tenebrae-bench/Cargo.toml` -- keep `publish = false`; no change.
- `.github/workflows/ci.yml` -- jobs `fmt`, `clippy`, `test`, `bench` (the required checks on `main`); add a `workflow_call` trigger so the release can call it. Job names must not change.
- `.github/workflows/release.yml` -- new.
- `CONTRIBUTING.md` -- add a "Releasing" section (bump PR, annotated tag, approving the run, first manual publish, trusted-publisher setup per crate with environment `release`, AD-14's pre-tag maintainer checks).
- Facts: library crates hold no `include_bytes!` and no dev-dependencies, so packaging needs only `src/` (and `tome/tests/`). Toolchain 1.97.1 has `cargo publish --workspace`, which orders by dependencies. `rust-lang/crates-io-auth-action` v1.0.5 = `c6f97d42243bad5fab37ca0427f495c86d5b1a18`.

## Tasks & Acceptance

**Execution:**
- [x] `Cargo.toml` -- add `repository = "https://github.com/OmikronApex/Tenebrae"`, `readme = "README.md"`, `keywords`, `categories` (valid crates.io slugs) to `[workspace.package]`; add `version = "=0.1.0"` to each library entry in `[workspace.dependencies]` -- AD-14.
- [x] five library `Cargo.toml` -- drop `publish = false`; inherit `repository`, `readme`, `keywords`, `categories` -- publishable.
- [x] `.github/workflows/ci.yml` -- add `workflow_call:` -- reuse the gates.
- [x] `.github/workflows/release.yml` -- trigger on tags `v[0-9]+.[0-9]+.[0-9]+`; `verify` job (ancestor of `origin/main`, tag == workspace version); `gates` job calling `ci.yml`; `publish` job (needs both, `id-token: write`) authenticating and running `cargo publish --workspace` -- AD-14.
- [x] `.github/workflows/release.yml` (cont.) -- `publish` runs in environment `release`; a final step creates the GitHub Release from the tag message (`contents: write`) -- user decisions.
- [x] GitHub repository settings -- create environment `release` with the owner as required reviewer (via `gh api`, with the user's go-ahead) -- the approval gate.
- [x] `CONTRIBUTING.md` -- "Releasing" section -- the manual steps.
- [x] `crates/tenebrae-runtime/src/query.rs`, `traverse.rs`, `crates/tenebrae-bench/examples/rtcount.rs` -- drop `Counters::stage_ns` and the clock in `Traversal::run`; `rtcount` stops printing stage times -- renegotiated decision (AD-1).

**Acceptance Criteria:**
- Given the changed manifests, when `cargo publish --workspace --dry-run` runs locally, then it packages and verifies exactly the five library crates, in dependency order.
- Given the branch, when CI runs on its PR, then `fmt`, `clippy`, `test`, `bench` pass under unchanged names.
- Given `release.yml`, when it is linted (`actionlint` if available, else a careful read against the matrix), then every matrix row is handled.

## Implementation Notes

- Metadata: `keywords = ["umbra", "visibility", "occlusion-culling", "culling", "tome"]`, `categories = ["game-development", "rendering", "parser-implementations"]`, `readme = "README.md"` (the workspace README, packaged into each crate). `tenebrae-bench` keeps a path-only workspace entry: it is not published.
- `release.yml` jobs: `verify` (ubuntu) fetches the tag object and `main` explicitly, then checks annotated (`git cat-file -t` = `tag`), ancestor of `origin/main`, and `cargo metadata` versions == tag version; it reports every failed check with `::error::` before failing, and passes the tag message (subject + body, no signature) to `publish` as an output. `gates` (`needs: verify`) calls `ci.yml`. `publish` (`needs: [verify, gates]`, environment `release`, `contents: write`, `id-token: write`) authenticates, runs `cargo publish --workspace` and `gh release create --verify-tag` with the notes. Approval rejected: the job never runs, so no publish and no Release.
- Actions pinned: `actions/checkout@11d5960a326750d5838078e36cf38b85af677262` (v4.4.0, same major as `ci.yml`), `rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18` (v1.0.5). No other actions; the Release uses the runner's `gh`.
- Not done: the `release` environment was not created in GitHub (needs the user's go-ahead; the `gh api` command is in `CONTRIBUTING.md`).
- First release: the manual first publish already uploads `0.1.0`, so the `v0.1.0` tag run is expected to have its deployment rejected and the Release created by hand (`gh release create --notes-from-tag`); documented in `CONTRIBUTING.md`.

- Review pass 1 patches (2026-10-13): `verify` compares the tag's commit with `GITHUB_SHA` and runs `cargo publish --workspace --dry-run` before gates and approval; each library crate carries a copy of the root `LICENSE` (cargo packages it; `license` stays the SPDX id, no `license-file`); CONTRIBUTING names the `stage_ns` prerequisite for the first publish and the Release-step-only recovery; AD-14 carries a dated amendment for the interim gate set.
- The committed dry run packages and verifies all five crates before uploading any, so a build failure cannot leave a partial publish; only an upload-time failure can.

- Renegotiated scope (user, 2026-10-13): `Counters::stage_ns` and the runtime's `Instant` removed; no library crate reads a clock now. fmt, clippy, 74 tests, 11 bench gates and the publish dry run pass. `rtcount` keeps its own wall-clock timing (bench crate, allowed by AD-1).

## Spec Change Log

## Review Triage Log

Pass 1 (2026-10-13): blind hunter (BH), edge-case hunter (ECH), verification gap (VG).

| # | Finding | Verdict | Route | Evidence |
|---|---------|---------|-------|----------|
| BH1, ECH1, ECH8 | `verify` checks the re-fetched tag's commit; `gates`/`publish` build `github.sha`, so a moved tag is checked on one commit and published from another | medium | patch | `release.yml` fetches `+refs/tags/${TAG}` with `--force` and never compares it with `GITHUB_SHA`; checkout and `ci.yml` use `github.sha`. |
| VG2, BH7 | Packaging and the Linux build are first exercised in the approved `publish` job | medium | patch | `ci.yml` builds on Windows only and never packages; `publish` runs on `ubuntu-latest`. Fix: dry run in `verify`. |
| BH2 | Published crates carry no licence text | medium | patch | `cargo package -p tenebrae-geom --list` lists no `LICENSE`; with `license-file` it does, without a warning. |
| BH3, ECH6, VG-o1 | CONTRIBUTING's first-publish steps omit the `stage_ns` prerequisite | medium | patch | `traverse.rs:142` still reads a clock; AD-10 allows dropping the field only before the first publish. |
| ECH4, ECH5, BH8 | Rerunning after a failed Release step fails on already-published versions; recovery text covers only partial publish | low | patch | `cargo publish` refuses existing versions; a one-line recovery note is a direct correction. |
| BH4 | AD-14 says the release reruns AD-15's full gate set; the workflow reruns today's `ci.yml` | low | patch | Spine AD-14 rule line unamended; dated amendment is a direct correction. |
| VG1 | `verify` script logic is tested by hand only | low | reject | Release runs rarely, every publish waits for owner approval, and the fix is new test infrastructure. |
| BH5 | `ci.yml` actions in the release path are tag-pinned | low | reject | Frozen intent: "Never: re-pinning `ci.yml`'s existing actions (deferred)". |
| BH6 | `rust-cache` saves caches on tag runs | low | reject | A few cache entries per release; fixing needs extra configuration. |
| BH9 | Matrix lacks partial-publish/rerun rows; per-tag concurrency lets two releases interleave | low | reject | Matrix edit is a spec edit; concurrent releases each wait for approval, so they cannot interleave unattended. |
| BH10 | `verify` does not compare the `=X.Y.Z` pins with the tag | low | reject | A stale pin fails resolution loudly (`failed to select a version for the requirement`) in `verify`'s `cargo metadata`/dry run. |
| BH11 | Workspace README packaged as every crate's README | false | reject | README has no relative links; using it is the planned choice, rendered as-is on crates.io. |
| BH12 | Docs fix an order the dry run did not follow; tag subject repeated in notes; signed-tag body | false | reject | CONTRIBUTING/AD-14 list `tome, geom` as one tier ("then" separates tiers); `%(contents:body)` excludes the signature. |
| BH13 | Verification used `--allow-dirty`, no shellcheck, some rows reasoned only | low | reject | The tree was uncommitted by design; the committed dry run and PR CI run in step 4/5 verification. |
| ECH2, ECH3, ECH7 | A failing `rev-parse`/`cargo metadata` aborts under `set -e` without `::error::` | low | reject | Needs a tag on a non-commit object or a broken manifest; the job still fails loudly with the command's own error. |
| BH2 (follow-up) | Patch as sent (`license-file` beside `license`) prints `only one of license or license-file is necessary` on every cargo command | low | patch | `cargo check --workspace` printed it 5×; replaced by a `LICENSE` copy in each library crate directory, which cargo packages without the field; the earlier "without a warning" read was from a filtered run. |
| VG-o2 | Off-main check is only as fresh as `verify` | low | reject | Reviewer accepts it; `main` only moves forward, so ancestry cannot be lost. |

## Verification

Results 2026-10-13: `cargo publish --workspace --dry-run --allow-dirty` packaged and verified tenebrae-geom, tenebrae-tome, tenebrae-bake, tenebrae-runtime, tenebrae in that order (tools and bench skipped). fmt, clippy (`--workspace --all-targets -D warnings`) and `cargo test --release` pass. `actionlint` 1.7.7 (shellcheck not installed): 0 errors. The `verify` script was run against a local bare clone: annotated tag on main passes and emits the notes; lightweight, off-main and version-mismatch tags each fail with their message.

**Commands:**
- `cargo publish --workspace --dry-run` -- expected: five crates packaged and verified, tools and bench skipped.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --release` -- expected: pass.

**Manual checks (if no CLI):**
- `release.yml`: verify-job logic for each matrix row; SHA pins with version comments.
