---
status: "accepted"
date: 2026-08-31
decision-makers: project owner
---

# Introduce `ffx`, a feature-gated facade over the `ff-*` primitive family, and make `avio` depend only on it

## Context and Problem Statement

`avio` is the editing engine and the `ff-*` crates are model-agnostic primitives
(ADR-0004). The intent was that `avio` would depend only on the few `ff-*` crates it
needs. In practice, growing the engine pulled in almost the entire family
(`ff-decode`, `ff-encode`, `ff-filter`, `ff-pipeline`, `ff-format`, `ff-probe`,
`ff-analysis`, `ff-preview`, `ff-render`, and their transitive deps), so `avio`'s
`Cargo.toml` and imports fan out across a dozen crates. We want one dependency for the
engine to consume, a single place to describe the family's optional surface, and a
clean boundary at which the `ff-*` family can later split into its own repository
(a standalone "safe FFmpeg for Rust" library) separate from the engine.

This decision does not change ADR-0004: `avio` stays the engine and is not itself the
facade. The facade role moves to a dedicated crate so the primitives, not the engine,
carry the umbrella.

## Decision Drivers

* One dependency for the engine (and for any external consumer of the primitives) to
  add, instead of a dozen.
* A single, feature-gated description of the family's optional surface (the heavy
  `ff-render`/wgpu path, preview, analysis, stream, serde, hwaccel, gpl).
* A clean cut for a future two-repository split (engine vs primitive family) without
  re-plumbing every `avio` dependency at that time.
* The primitives must remain usable without the engine; the facade must not depend on
  `avio` (dependency direction stays one-way).
* Do not reintroduce the primitive-facade responsibility into `avio` (ADR-0004).

## Considered Options

* Status quo: `avio` depends on each `ff-*` crate directly.
* Make `avio` the facade (re-export the primitives from `avio`) — already rejected by
  ADR-0004.
* A dedicated facade crate `ffx` that aggregates the whole `ff-*` family, feature-gated
  (Bevy's `bevy` over `bevy_*` pattern); `avio` depends only on `ffx`.
* Within the facade: flat re-export of everything vs feature-gated sub-crates.

## Decision Outcome

Chosen: a dedicated facade crate **`ffx`** aggregating the whole `ff-*` family with
**Bevy-style feature gating**, and `avio` depending **only on `ffx`**.

* `ffx` re-exports each sub-crate under a namespaced module (`ffx::decode`,
  `ffx::filter`, `ffx::render`, `ffx::format`, ...), plus an `ffx::prelude`. Namespaced
  (not flat) so same-named items across crates (e.g. each crate's `Error`) do not
  collide.
* `default` enables a lightweight core (`decode`, `encode`, `filter`, `format`,
  `common`, `probe`, `remux`, `pipeline`). Heavy or optional crates sit behind features:
  `render` (→ `ff-render`/wgpu), `preview`, `analysis`, `stream`, `serde`, `hwaccel`,
  `gpl`. Each optional sub-crate is an `optional` dependency toggled by its feature.
* `avio` drops all direct `ff-*` dependencies and depends on `ffx`, forwarding its own
  features (`gpu` → `ffx/render`, `preview` → `ffx/preview`, `serde` → `ffx/serde`,
  `hwaccel`/`gpl` passthrough). `avio`'s public re-exports (the model-facing types) are
  preserved, sourced through `ffx`, so `avio`'s public API is unchanged.
* **Phase 1 (this decision) is the in-repo facade only**: `ffx` is introduced in the
  existing workspace under the shared lockstep version. The **two-repository split**
  (independent versioning, CI, and publishing of the `ffx` + `ff-*` family, with `avio`
  depending on a published `ffx`) is deferred to its own milestone; the `ffx` boundary
  is the prerequisite that makes that split a single-dependency change.

### Confirmation

* A dependency guard asserts `avio`'s `Cargo.toml` names no `ff-*` crate directly (only
  `ffx`); the workspace still builds and the `avio` `lib.rs` accessibility tests pass
  unchanged (the public API is preserved).
* `ffx` re-export tests assert each namespaced module exposes its crate's surface, and
  that a `default`-features build does **not** pull in `ff-render`/wgpu (the heavy path
  is behind the `render` feature).
* No dependency cycle: `ffx` does not depend on `avio` (compile-time, one-way).

### Consequences

* Good: the engine (and external users) add one crate; the family's optional surface is
  described once; the primitives are usable without the engine; the later repo split is
  a one-line dependency swap rather than a re-plumb.
* Bad: an extra crate layer; a one-time import churn in `avio` (`ff_decode::X` →
  `ffx::decode::X` across its source); `ffx` must track the family's public API as it
  evolves.
* What would reverse this: drop `ffx` and restore `avio`'s direct `ff-*` dependencies
  (mechanical); the sub-crates are unchanged, so the facade is purely additive and
  removable.

## Pros and Cons of the Options

### Status quo (direct deps)

* Good, because no new crate; simplest today.
* Bad, because the dependency fan-out persists and a future repo split must re-plumb
  every `avio` dependency at once.

### `avio` as the facade

* Good, because no new crate.
* Bad, because it makes `avio` a primitive facade again — reversed by ADR-0004 — and
  couples the primitive surface to the engine, blocking a clean engine/primitives repo
  split.

### Dedicated `ffx` facade (chosen)

* Good, because one dependency, one feature surface, primitives independent of the
  engine, and a clean split boundary.
* Bad, because of the extra layer and the one-time `avio` import churn.

### Flat vs feature-gated re-export

* Feature-gated (chosen), because depending on `ffx` must not force `ff-render`/wgpu and
  the other heavy optional crates on every consumer.
* Flat, rejected because it makes the facade as heavy as the whole family.

## More Information

* Complements ADR-0004 (`avio` engine, not facade): the facade role lives in `ffx`, not
  `avio`.
* `docs/specs/engine-and-primitives.md` (the `ffx` layer in the architecture of record).
* Pattern reference: Bevy's `bevy` facade over the `bevy_*` sub-crates.
* Naming: `ff`/`ffm`/`ffav` were unavailable on crates.io (`ff` is the finite-field
  crate; `ffav` is an unrelated FFmpeg wrapper); `ffx` was chosen as the shortest
  available umbrella name, with the "safe FFmpeg family" signal carried by the crate
  description / README rather than the name.
