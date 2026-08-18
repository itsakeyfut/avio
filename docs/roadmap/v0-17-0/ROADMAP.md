# v0.17.0 — Editing Model Maturity & Library Hardening

**Goal**: Make `avio`'s editing model genuinely adoptable as an editor host's *single editing
document* — a lean, modern NLE model edited through a usable command/history API — and harden the
`ff-*` primitives while adding the source and audio primitives that the new model relies on.

The engine requirements are drawn from #1414, the capability catalog produced by making
`avio::Timeline` / `avio::Clip` the single editing document of `avio-editor-demo`. The design goal
is a lean, modern model built without waste — not a one-to-one copy of the demo's host-side model.

**Prerequisite**: v0.16.0 complete.

**Crates in scope**: `avio` (engine), `ff-filter`, `ff-encode`, `ff-remux`, `ff-preview`, `ff-render`.

**Out of scope (deferred to later model milestones)**: a per-clip retiming / segment model (freeze
that extends clip length, speed ramps / time remap); a typed, re-editable per-clip effect-parameter
model. Both are independent subsystems and get their own milestones.

---

## Requirements — Engine (`avio`): a modern editing model

### Stable identity

- Every clip and every track has a stable identity assigned at construction and preserved across
  edits, undo/redo, insertion, removal, and reordering.
- Edit commands address clips and tracks by identity, not by position, so an in-progress gesture
  stays valid when the timeline changes around it.
- A host can key its own authoring state (selection, thumbnails, per-clip UI) to a clip by its
  identity without stuffing a surrogate id into clip metadata.

### Track model

- Tracks are first-class objects (video and audio) carrying at least name, mute, solo, enabled, and
  lock state.
- Mute, solo, and enabled are honored consistently by both export render and preview derivation.

### Host-adoptable edit & history API

- The editor can update the current document version in place, without pushing a new undo step
  (amend), so an in-place rich edit or a live drag does not flood the history.
- A set of commands can be grouped so that one user gesture (a drag emitting many values, a ripple,
  a multi-clip edit) collapses to exactly one undo step.
- An externally constructed or externally mutated `Timeline` can be seated as the current version, so
  a host that assembles a timeline outside the command stream can still adopt the editor's history.
- The full per-clip property surface — color correction, fades, transition, scale, rotation, the
  effect chain, keyframe/animation tracks, metadata, proxy, and pitch — is editable through the
  undoable command path (for example, an opaque per-clip patch command alongside the existing typed
  commands), so per-clip editing is no longer limited to a handful of typed properties.

### Core timeline operations

- A clip can be split at a point into two contiguous clips; the right-hand clip keeps the source
  properties and clears any leading transition/fade.
- A clip can be moved to a different track in a single operation that preserves its identity.
- Ripple delete/edit — removing or trimming a clip and closing the resulting gap by shifting later
  clips — is expressible as a single atomic (one-undo-step) operation.

### Per-clip transform, framing, and audio completeness

- Per-clip scale and rotation are first-class fields with keyframe tracks, at parity with the
  existing per-clip opacity, position, and volume.
- Each clip has a framing mode against the project canvas — fill (cover + crop) and fit
  (contain + letterbox) — applied by the compositor without a hand-built crop/scale/pad chain.
- Per-clip audio pitch is a first-class, animatable (keyframe-able), undoable field.
- A track- or timeline-level audio effect chain (for example, EBU R128 loudness normalization) can be
  applied on render, mirroring per-clip audio effects.

### Source model

- A clip's source is a typed value (a `ClipSource`), not only a file path: at minimum a media file
  and a text/title source (a solid-color source is desirable in this milestone; further generated
  sources are future work).
- Text/title clips are first-class: each carries its own text, style, position, and duration, is
  independently movable and trimmable, can appear multiple times on its own lane, and is composited
  per clip — distinct from a single whole-canvas overlay string.

### Persistence

- The editing document (timeline, tracks, clips, and their animation tracks) can be serialized and
  deserialized behind an optional `serde` feature, so a host can save and reload a project.

---

## Requirements — Primitives (`ff-*`): hardening and aligned capabilities

### Robustness and safety

- `ff-encode` and `ff-remux` build clean under the workspace clippy configuration without a
  crate-wide lint-suppression block; any remaining allowances are narrowed to justified, per-item
  cases.
- `ff-preview` reports a poisoned decode thread as a typed, recoverable error rather than panicking,
  and its playback/seek/AV-sync paths have integration coverage.
- `ff-render` GPU nodes have test coverage, exercised headlessly where the environment permits.

### Source primitives (backing the engine's `ClipSource`)

- A text/title layer can be rendered from a text spec (string, style, position) into a compositable
  frame/layer, with no knowledge of timeline, track, or clip (model-agnostic).
- A solid/color source can generate a compositable frame/layer.

### High-quality audio pitch and time-stretch (Oto-MAD)

- Pitch-shift and time-stretch offer a high-quality, formant-preserving backend (for example,
  librubberband) in addition to the existing `asetrate`/`atempo` path, selectable by the caller, so a
  voice clip can be mapped across roughly two octaves without the artifacts of the simple method.
- When the high-quality backend is unavailable in the FFmpeg build, the primitive falls back to the
  existing path.

---

## Design Decisions

| Topic | Decision |
|---|---|
| Clip / track identity | Opaque stable ids assigned at construction; commands are id-addressed |
| Command coverage | An opaque per-clip patch command plus `Batch`, rather than one command per property |
| Editor API | Gains amend / in-place update, group (coalesce) commit, and seating an external `Timeline` |
| Source model | `ClipSource` enum replaces `source: PathBuf`; file + text this milestone (solid color desirable) |
| Text/solid sources | Provided as `ff-filter` source primitives; no new crate unless design requires it |
| Framing | A per-clip fit/fill mode composed from existing scale/crop/pad; no new filter primitive |
| Per-clip pitch | Engine exposes a declarative/animatable field over the existing `ff-filter` pitch step |
| HQ pitch backend | librubberband behind availability/feature detection, with graceful fallback |
| serde | Optional, behind a `serde` feature (the `ff-filter` animation types already gate one this way) |
| Deferred | Retiming / freeze-length / speed ramps and typed effect parameters are separate milestones |

---

## Definition of Done

- An editor host can adopt `avio::Timeline` / `avio::Clip` / `avio::Editor` as its single editing
  document without the #1414 host-side workarounds for the in-scope gaps (surrogate ids, serde mirror
  DTO, parallel track flags, a re-implemented history, hand-composed split / cross-track move / ripple,
  and sidecar title clips).
- A project can be saved and reloaded round-trip, reconstructing an equivalent document.
- A gesture that emits many values collapses to a single undo step; undo and redo restore prior
  versions.
- Splitting, cross-track move, and ripple delete are each achievable as one atomic operation.
- Per-clip scale, rotation, fit/fill, and pitch render and preview identically through the shared
  derivation.
- The high-quality pitch/time-stretch backend produces formant-preserving output, verifiable against
  the existing `asetrate`/`atempo` path.
- `ff-encode` and `ff-remux` build with no crate-wide clippy-allow block; `ff-preview` no longer
  panics on a decode-thread failure.
