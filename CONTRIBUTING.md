# Contributing to Tenebrae

## How this project is developed

Tenebrae is developed with AI coding agents (Claude Code and others) driven through the
[BMAD Method](https://github.com/bmad-code-org/BMAD-METHOD) framework, with a human owner making
every decision that matters: scope, acceptance criteria, trade-offs, and what gets merged. Most of
the code, tests and documents in this repository were written by an agent under that direction and
reviewed by the owner and by independent agent reviewers. We disclose this plainly, and we keep the
process artifacts in the repository so the reasoning behind the code stays inspectable.

What that means in practice:

- **Specs before code.** Milestone work from M3b on is specified in `docs/implementation-artifacts/`
  (BMAD's `bmad-build` format): a frozen intent block the human owns, boundaries, an edge-case
  matrix, tasks with acceptance criteria, implementation notes with the measured numbers, a change
  log and a review triage log. The earlier milestones and a few smaller levers predate that habit;
  their scope and evidence are recorded in the README's status table and in
  `docs/specs/spec-tenebrae/milestones.md`. Product-level specs (`bmad-spec`: what to build) live
  under `docs/specs/`: the project spec `spec-tenebrae/` and the larger M3b work. The architecture
  lives in `docs/planning-artifacts/architecture/` as an architecture spine (`AD-n` decisions).
- **Decisions are recorded.** `.memlog.md` files beside the artifacts are the agents' working
  logs (assumptions, decisions, open questions). `deferred-work.md` is the ledger of what was
  consciously left out and why. Nothing is silently dropped.
- **Verification is in the repository.** `cargo test --release` and the benchmark gates
  (`cargo test --release -p tenebrae-bench`) are the acceptance; clippy and rustfmt run in the
  pre-commit hook. Every spec names the commands that prove it.

## If you contribute

You are welcome to work with or without agents. Either way, please keep to the same shape:

1. **Read first:** `AGENTS.md` (layout, policy, how to run things), the README's status table, and
   the spec of the milestone you touch.
2. **Specs for non-trivial work.** A feature, a format change, a new lever or a contract change
   gets a spec in `docs/implementation-artifacts/` before the code. The quickest way is the BMAD
   skills (`bmad-spec` to write one, `bmad-build` to implement it, `bmad-code-review` and
   `bmad-review` to review), which need the local setup below. A hand-written spec in the same
   structure is equally fine.
3. **Keep the invariants** in `AGENTS.md`: pure library crates, no `unsafe`, no dependencies
   outside the workspace, byte-identical bakes across thread counts and cache states, no game
   data in the repository, contract v2 unchanged unless a new version is named.
4. **Keep the gates green:** fmt, clippy with `-D warnings`, all tests, and the benchmark baseline.
   A deliberate change to the benchmark's numbers regenerates `baseline.txt` and says so in the
   commit message; a pinned hash changes only with an intended output change.
5. **Record what you decided and what you left out:** in the spec's change log and
   `deferred-work.md`. Reviewers should be able to see the alternatives you rejected.
6. **Disclose agent use** in the commit or pull request, as this project does
   (`Co-Authored-By:` trailers for agents are fine). Review what an agent wrote before you submit
   it; you are the author of record.

## Local BMAD setup

The BMAD install is **not** part of the repository: `.agents/`, `.claude/` and `_bmad/` are
gitignored, because the install is per developer and per agent client. To follow the workflow
with agents, install the BMAD Method into your clone with its installer (follow the current
instructions in the [BMAD Method repository](https://github.com/bmad-code-org/BMAD-METHOD); this
project was built with BMAD 6.12.x plus the `bmad-loop` module). Choose the agent client(s) you use (Claude Code, Codex, Cursor, …); the installer writes the
skills into `.agents/skills/` / `.claude/skills/` and its configuration into `_bmad/`. Keep
`_bmad/config.toml`'s output folders as the repository already uses them: planning artifacts in
`docs/planning-artifacts/`, implementation artifacts in `docs/implementation-artifacts/`. Nothing
the installer writes should be committed. Without the install you can still read every artifact
and write specs in the same structure by hand.

## Releasing

Releases follow AD-14 in the architecture spine: one version for every crate, and the five library
crates (`tenebrae-tome`, `tenebrae-geom`, `tenebrae-bake`, `tenebrae-runtime`, `tenebrae`) publish
together to crates.io. `tenebrae-tools` and `tenebrae-bench` never publish. The workflow is
`.github/workflows/release.yml`; only the owner releases.

### Each release

1. **Bump PR.** Change `[workspace.package] version` in the root `Cargo.toml` and the five
   `version = "=X.Y.Z"` pins in `[workspace.dependencies]` to the new version, refresh `Cargo.lock`
   (`cargo check`), and merge the PR to `main` through the usual checks.
2. **Pre-tag checks (AD-14).** On that `main` commit, run the stock-tome round trip
   (`cargo test --release -p tenebrae-tome` with `TENEBRAE_TOMES` set) and, for any change since
   the last release that AD-4 sends to the external harness, the external harness. Note the
   outcome; it goes into the tag message.
3. **Annotated tag.** `git tag -a vX.Y.Z -m "Tenebrae X.Y.Z" -m "<notes, including the pre-tag
   check results>"` on the `main` commit, then `git push origin vX.Y.Z`. The tag message becomes
   the GitHub Release notes. Only tags shaped `vX.Y.Z` start the workflow.
4. **The workflow** refuses a lightweight tag, a commit that is not on `main`, and a tag version
   that differs from the workspace version, naming the reason. Otherwise it reruns the CI gates
   (`fmt`, `clippy`, `test`, `bench`) on the tagged commit.
5. **Approve the run.** The `publish` job waits for the `release` environment; approve it on the
   run page (rejecting it publishes nothing). It authenticates through crates.io trusted
   publishing, runs `cargo publish --workspace` (dependency order: `tenebrae-tome`,
   `tenebrae-geom`, then `tenebrae-bake`, `tenebrae-runtime`, then `tenebrae`), and creates the
   GitHub Release for the tag.

If a publish fails part-way, the crates already uploaded cannot be replaced: publish the rest by
hand in dependency order (`cargo publish -p <crate>` with a short-lived API token from the tagged
commit) and create the GitHub Release with `gh release create vX.Y.Z --verify-tag
--notes-from-tag`.
If `cargo publish` succeeded and only the GitHub Release step failed, do not rerun the job (it
fails on the versions already published); create the Release with
`gh release create vX.Y.Z --verify-tag --notes-from-tag`.

### One-time setup

- **GitHub environment.** Repository settings, Environments: create `release` with the owner as
  required reviewer. Or with the CLI:
  `gh api -X PUT repos/OmikronApex/Tenebrae/environments/release -F "reviewers[][type]=User" -F "reviewers[][id]=$(gh api users/OmikronApex --jq .id)"`.
- **First publish (manual).** Prerequisite: drop `Counters::stage_ns` from `tenebrae-runtime` and
  the clock that fills it (logged in `deferred-work.md`; AD-1, AD-10) before the first
  `cargo publish`, or 0.1.0 locks the field in. Trusted publishing can only be configured for a
  crate that exists, so the first version of each crate is published by hand: on the release
  commit of `main`, with a crates.io API token (`cargo login`), run
  `cargo publish --workspace --dry-run`, then `cargo publish --workspace`, then revoke the token.
  Push the annotated tag afterwards. That run may stop at `verify` (its dry run finds the versions
  already uploaded) or reach the approval, which you then reject; either way, create the GitHub
  Release by hand with `gh release create vX.Y.Z --verify-tag --notes-from-tag`.
- **Trusted publishers.** On crates.io, for each of the five crates, Settings, Trusted Publishing,
  add a GitHub publisher: owner `OmikronApex`, repository `Tenebrae`, workflow `release.yml`,
  environment `release`. No crates.io token is stored in the repository's secrets.

## What we do not accept

- Game data, extracted assets, or anything derived from a proprietary binary. Tests read tomes
  from the directories named in `TENEBRAE_TOMES` and pass vacuously without them.
- Code that reproduces a proprietary runtime's internals. `tenebrae-runtime` is written from the
  format and the typed model; keep it that way.
- Dependencies in the library crates. Tooling may add what it needs.

## License

Contributions are accepted under the project's license, GPL-3.0-or-later (`LICENSE`).
