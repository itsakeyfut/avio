# v0.16.0 — Engine / Library Split: Independent Publishing

**Goal**: Present `avio` as an editing **engine** and the `ff-*` family as standalone, independently-versioned **primitive libraries**. The engine/primitive boundary is already clean (the model relocation and purification of #1326/#1327 landed, and preview == export via the C4 epic). This milestone makes that separation visible to downstream users: each `ff-*` crate is publishable and consumable on its own, versions diverge to reflect each crate's own change cadence, and the project moves to a GitHub organization.

**Prerequisite**: v0.15.x complete — the editing model lives only in `avio`, the `ff-*` crates are model-agnostic primitives, and the single `derive(model, t) -> Scene` feeds both preview and export.

**Crates in scope**: workspace-wide (every crate's manifest; CI; release automation; docs). No source refactoring of the boundary is required — it is already clean.

**Also included in this cut** (already merged since v0.15.1, additive; not the headline theme):
- Preview == export visual parity — the C4 epic (single `derive`, `composite_op` / `lavfi_overlay` / audio / xfade-kind parity in preview).
- **音MAD support** (Pitch Shift ±24 semitones & BPM Detection) — a minor feature; see `oto-mad.md`.

---

## Requirements

### Independent per-crate versioning

- Each crate carries its own `version` in `[package]`, decoupled from a single shared workspace version. Versions may diverge so that each number reflects only that crate's own changes (the tokio / `http` model, not the bevy / wgpu lockstep model).
- Foundational primitives (`ff-format`, `ff-common`) can reach and hold a stable version independent of higher-churn crates (`ff-filter`, `ff-preview`, `ff-render`): a change in one crate never forces a version bump in an unchanged crate.
- Internal dependencies are expressed as both a `path` and a `version` requirement, so every crate is publishable to crates.io and a consumer of a single `ff-*` crate receives an honest SemVer contract for that crate alone.
- Versions never regress below what is already published on crates.io (the 0.15.1 baseline is monotonic); independent version lines begin at or above that baseline (not reset to 0.1.0).

**Rationale (why independent, not lockstep)**: the purpose of the split is that the `ff-*` crates are pure, independently-usable FFmpeg primitives with genuinely different stability guarantees and audiences. Lockstep would pin the stable foundations to the churn of the volatile engine crates — `ff-format` could never reach and hold 1.0 while `ff-filter` iterates at 0.x (the trap that keeps bevy and futures family-wide at 0.x). Independent versioning is the honest expression of "these are separate libraries."

### Automated, change-driven releases

- Releases are driven by `release-plz`: it derives per-crate version bumps and changelog entries from conventional commits, creates per-crate tags (e.g. `ff-filter-v0.3.0`), and updates dependent crates' internal version requirements automatically.
- The publish flow tolerates unchanged crates: a release publishes only the crates whose version advanced, and does not abort when an unchanged crate's version already exists on the registry (the current all-crates loop under `set -euo pipefail` would abort here).
- Publishing continues to use crates.io Trusted Publishing / OIDC (no long-lived tokens); local `cargo publish` remains blocked by design.
- Each crate has its own changelog, replacing the single-version `CHANGELOG.md` assumption.

### Per-crate publishing readiness

- Every library crate publishes with complete metadata (`description`, `license`, `repository`, `readme`, `keywords`, `categories`). This is already satisfied for all 12 crates; the milestone verifies it as a release gate rather than adding it.
- Non-library members (`avio-examples`, `tools/gen-test-assets`, the `fuzz` crates) remain excluded from publishing (`publish = false`).
- Each primitive crate's public surface and docs read as a standalone library: no dangling references to the editing model from a primitive crate. The stale `scene_adapter` / `avio::TimelinePlayer` doc references in `ff-preview` are reconciled, and any model-flavoured vocabulary on the primitive public surface is reviewed.

### GitHub organization migration

- The repository moves to a GitHub organization. The `repository` slug in crate metadata, `CODEOWNERS`, and `.github/*` references point to the org.
- Each crate's crates.io Trusted Publisher is re-pointed to the org (this configuration lives on crates.io, performed by the maintainer).
- Historical changelog links and cross-repo references rely on GitHub's transfer redirects rather than being rewritten.

### Documentation — the "engine + primitives" story

- `avio` is documented as the editing engine; the `ff-*` family is documented as independently-usable FFmpeg primitive libraries, with guidance on consuming a single primitive crate without the engine.

---

## Design Decisions

| Topic | Decision |
|---|---|
| Versioning model | Independent per-crate (tokio / `http`), not lockstep — `ff-*` are standalone products with divergent cadences; a stable `ff-format` must be able to reach and hold 1.0 while `ff-filter` iterates at 0.x |
| Release tooling | `release-plz` — per-crate tags, per-crate changelogs, dependent-crate version bumps, all derived from conventional commits |
| Initial versions | Start from the 0.15.1 baseline (crates.io versions are monotonic); do not reset to 0.1.0 |
| pitch/BPM (Oto-MAD) | Included as a **minor feature** of this release (already implemented; see `oto-mad.md`); additive, not the headline theme |
| Release numbering | One 0.16.0 cut carries the accumulated code (engine-separation, C4 preview==export, Oto-MAD) and introduces independent versioning + org migration; no separate 0.15.2 is cut |
| Boundary purification | None required — #1326 / #1327 already landed; only cosmetic doc reconciliation remains |
| Org migration | Repo transfer + metadata/CI slug updates + crates.io Trusted Publisher re-pointing; changelog history and cross-repo links left to GitHub redirects |

---

## Definition of Done

- Each crate declares an independent `[package].version`; the shared `[workspace.package].version` is removed, and every `[workspace.dependencies]` internal dependency keeps a matching `version` requirement.
- `release-plz` (or equivalent) produces per-crate version bumps, changelogs, and tags from commits; a dry-run release publishes only changed crates and does not abort on unchanged ones.
- All 12 library crates publish to crates.io with complete metadata; `avio-examples`, `tools`, and `fuzz` crates are excluded.
- Primitive crates' public docs contain no references to the editing model; the `ff-preview` scene / `TimelinePlayer` doc references are reconciled.
- The repository lives under the GitHub organization; the crate `repository` slug, `CODEOWNERS`, `.github/*`, and each crate's crates.io Trusted Publisher point to the org.
- `cargo clippy --workspace --all-features -- -D warnings` clean; `cargo test --workspace` passes (CI full sweep).

---

## Candidate child issues

These decompose the requirements into implementable units (to be filed):

1. **Decouple crate versions** — move `version` from `[workspace.package]` into each `[package]`; keep `[workspace.dependencies]` `path` + `version` requirements in sync. (Highest release-risk; do first.)
2. **Adopt `release-plz`** — replace the all-crates publish loop in `release.yml` with per-crate, change-driven releases (tolerant of already-published crates); per-crate changelog convention.
3. **Cosmetic doc reconciliation** — refresh the stale `scene_adapter` / `avio::TimelinePlayer` references and review primitive-surface vocabulary (`ff-preview` `Scene` fields).
4. **Publishing readiness gate** — a CI/dry-run check that every library crate is publishable (metadata + `--dry-run` per crate) and non-library members stay excluded.
5. **Org migration** — repository transfer; update `repository` slug, `CODEOWNERS`, `.github/*`; re-point crates.io Trusted Publishers. (Partly maintainer-performed outside the repo.)
6. **Engine + primitives documentation** — README/story updates positioning `avio` as engine and `ff-*` as standalone libraries.
