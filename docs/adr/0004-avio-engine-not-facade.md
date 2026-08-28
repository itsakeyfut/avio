---
status: accepted
date: 2026-08-21
decision-makers: itsakeyfut
---

# avio is the editing engine, not a facade over the primitives

## Context and Problem Statement

`avio` currently plays two roles. It is the **editing engine** (it owns the
`Timeline` / `Clip` model, the model-to-frame derivation, and edit history), and
it is also a **facade**: `crates/avio/src/lib.rs` re-exports all twelve `ff-*`
primitives behind feature flags (`pub use ff_decode::…`, `ff_encode`, `ff_stream`,
`ff_preview`, `ff_render`, …), so a caller can add one crate and reach the raw
decoders, encoders, pipeline, HLS output, player, and render graph directly.

The facade role blurs what `avio` is: the crate's Quick Start is almost entirely
primitive usage (`avio::VideoDecoder`, `Pipeline`, …), so `avio` reads as a
general-purpose FFmpeg wrapper rather than an editing engine, and it competes on
the same axis as `ffmpeg-next` / `rsmpeg`. It is also a large re-export and
feature matrix to keep in sync. This record decides to drop the facade role.
`avio` is pre-1.0 with few external users, so this is the moment to change the
public surface. It is `proposed`; the change lands in later PRs.

## Decision Drivers

* A single, clear identity (an editing engine), consistent with the
  engine/primitive separation (#1326): the boundary should be visible in `avio`'s
  public API, not only in the dependency direction.
* Avoid competing with general-purpose FFmpeg wrappers on the primitive-wrapper
  axis; the differentiation is the editing model (the same reasoning as ADR-0003
  for `ff-sys`).
* Shrink the re-export / feature-flag maintenance surface.
* The engine's own API unavoidably surfaces some `ff-format` value types
  (`PixelFormat`, `Color`, `VideoCodec`, `TextSpec`, `Hdr10Metadata`, subtitle
  types), because `Timeline` / `Clip` take and return them; those must stay.

## Considered Options

* **Engine plus model-facing types only**: drop the standalone primitive-engine
  re-exports; keep the `ff-format` value types the model API surfaces; keep the
  feature flags as engine capabilities.
* **Keep the full facade** (status quo).
* **Pure model only**: expose only `Timeline` / `Clip` / `Editor` and nothing from
  `ff-*`.
* **Split into `avio-engine` + `avio-facade` crates**.

## Decision Outcome

Chosen option: **engine plus model-facing types only**. The litmus for what stays
re-exported mirrors the model/primitive litmus in `docs/rules/design.md`:

> Does `avio`'s own public engine API (`Timeline` / `Clip` / `Editor` / `render` /
> `derive`) name the type? Yes then re-export it (it is part of the engine
> surface); no (it is a standalone primitive tool) then drop it, and the consumer
> depends on the `ff-*` crate directly.

So the `ff-format` value types the model takes and returns stay re-exported, and
the feature flags (`preview` / `stream` / `render` / …) stay as engine
capabilities (the engine can preview, export HLS, and GPU-composite a `Timeline`).
What goes is the public re-export of the standalone primitive types
(`VideoDecoder`, `VideoEncoder`, `Pipeline`, `HlsOutput`, `PreviewPlayer`,
`RenderGraph`, and their siblings). A caller who wants those depends on
`ff-decode` / `ff-encode` / `ff-pipeline` / `ff-stream` / `ff-preview` /
`ff-render` directly; lockstep versioning (the shared `[workspace.package]`
version in `Cargo.toml`) keeps the multi-crate dependency painless.

Delivered as a direct removal: `avio` is pre-1.0 with few external users, so the
primitive-engine re-exports are removed outright rather than through a
`#[deprecated]` window, and the `avio` README / Quick Start is rewritten
engine-first.

Pure-model-only is rejected: the model API cannot avoid surfacing `ff-format`
value types, so a strict "nothing from `ff-*`" is not actually achievable and
would force duplicate types. A crate split is rejected as extra crate surface for
a role being removed.

### Confirmation

Realized and `accepted`. What confirms it:

* The primitive-facade re-exports are gone from `crates/avio/src/lib.rs`: it exposes
  only the editing engine (`Timeline` / `Clip` / `Editor` / `render`), the
  `ff-format` / `ff-filter` value types the model speaks, `EncoderConfig` /
  `Progress`, the preview `Scene` surface, and the `open` / analysis convenience
  keeps. The `#[cfg(test)]` accessibility tests in `lib.rs` assert that kept surface,
  so removing a kept type breaks them.
* `avio-examples` and the docs build against the engine surface and import no
  primitive engine from `avio`; a consumer that needs a primitive depends on the
  `ff-*` crate directly (e.g. `avio-examples` reaches for `ff_decode` / `ff_encode` /
  `ff_common` for the fixtures it synthesizes).

Delivered by #1482 (classified the re-exports), #1483 (made the model unconditional),
and #1484 (relocated the primitive examples, then removed the facade). A dedicated
public-API regression guard (#1485) was considered and declined: `avio` is still
pre-1.0 and may legitimately re-export further `ff-*` types as it matures, so
hard-enforcing "no primitive re-exports" is premature — reconsider once the public
surface stabilizes.

### Consequences

* Good, because `avio` gains a crisp identity, a smaller public surface, and less
  re-export/feature maintenance, and the engine/primitive boundary becomes visible
  in the API.
* Good, because the differentiation (the editing model) is what the crate leads
  with, instead of a wrapper surface that invites the wrong comparison.
* Bad, because it is a breaking change: every consumer of `avio::VideoDecoder` and
  friends migrates to the `ff-*` crate.
* Bad, because it drops the one-dependency "just transcode" convenience (mitigated:
  lockstep versioning makes multi-crate deps easy, and that user is served by the
  `ff-*` crates directly or by a general-purpose wrapper).
* Bad, because the `avio` README / Quick Start must be rewritten engine-first; the
  current facade examples go.
* What would reverse this: demand for a batteries-included single crate strong
  enough to reintroduce a facade, which would be a new crate (for example
  `avio-full`), not a change to `avio` (a new record superseding this one).

## Pros and Cons of the Options

### Engine plus model-facing types only (chosen)

* Good, because it gives one identity and the smallest surface that still lets the
  model API speak in its own types.
* Good, because the litmus makes "what stays" mechanical and matches the existing
  model/primitive rule.
* Bad, because it is a breaking change and a README rewrite.

### Keep the full facade (status quo)

* Good, because it is the least work and gives one-dependency convenience.
* Bad, because it blurs the crate's identity, invites the wrong comparison, and is
  a large re-export/feature surface to maintain. This is the defect this record
  addresses.

### Pure model only

* Good, because it would be the most minimal surface.
* Bad, because the model API cannot avoid surfacing `ff-format` value types, so it
  is not actually achievable and would force duplicate types.

### Split into `avio-engine` + `avio-facade`

* Good, because each crate would have a single role.
* Bad, because it adds crate surface for a role being removed, and re-creates the
  facade under a new name rather than dropping it.

## More Information

* Code: `crates/avio/src/lib.rs` — after #1484 it re-exports only the engine surface
  (the model `Clip` / `Timeline` / `Editor` / `derive`, the `ff-format` / `ff-filter`
  value types the model speaks, `EncoderConfig` / `Progress`, the preview `Scene`
  surface, and the `open` / analysis convenience keeps). `avio-examples` exercises
  the engine surface.
* Architecture: the engine/primitive separation (#1326) in
  `docs/specs/engine-and-primitives.md`.
* Related: ADR-0003 (`ff-sys` curated safe layer) shares the "curated and
  opinionated, not general-purpose" philosophy. The re-export removal and the
  engine-first README rewrite landed across #1482-#1484 (v0.17.0).
