---
status: accepted
date: 2026-08-30
decision-makers: itsakeyfut
---

# Represent per-clip effects as a typed, id-addressed, keyframable model instead of flat fields

## Context and Problem Statement

A clip's video effects were split across two shapes: three flat scalar fields
(`Clip::brightness`/`contrast`/`saturation`, compiled to an `eq` step) and an opaque
`Vec<FilterStep>` (`Clip::video_effects`). Neither lets a host present a clip's
effects as an editable, ordered list, enable/disable or reorder an individual effect,
or keyframe an individual parameter: the flat fields are unaddressable and
single-instance, and a `FilterStep` is an execution primitive, not an authoring value
(a host would have to parse and rebuild it). Issue #1458 (the v0.18.0 axis-A flagship)
requires deciding how per-clip effects are represented and edited in the model.

## Decision Drivers

* A host must address, reorder, toggle, and set the parameters of each effect through
  undoable edit commands, and keyframe any parameter (const or animated).
* `derive` must stay one pure interpretation shared by export and preview
  (ADR-0004), and the compiled result must render identically to the prior flat-field
  / `FilterStep` path (no visual regression).
* Effects are a CLIP/EDIT concern, so the model lives in `avio`; the `ff-*`
  primitives stay model-free (the engine/primitive litmus in `CLAUDE.md`).
* Clips and tracks are already addressed by document-scoped monotonic ids
  (ADR-0001); effects should reuse that identity scheme rather than invent another.

## Considered Options

* **A. A typed authoring layer in `avio`** — `Clip::effects: Vec<ClipEffect>`, each a
  `#[non_exhaustive] EffectKind` with `Param` (const-or-keyframed) fields, addressed by
  a new `EffectId`, edited via five id-addressed commands, and compiled to `FilterStep`
  by `EffectKind::to_filter_step`. Color correction folds into `EffectKind::ColorCorrect`.
* B. Keep the flat fields and the opaque `Vec<FilterStep>`, and layer editing helpers
  on top (parse/patch `FilterStep`s in place).
* C. Make `FilterStep` itself the authoring type (add ids/enable/params to it in
  `ff-filter`).

## Decision Outcome

Chosen option: **A**, as a curated vertical slice: the model, the five commands, the
derive contract, and serde are implemented end-to-end, but only two `EffectKind`s
(`ColorCorrect`, `Blur`) ship in v1 — enough to exercise ordering, enable/disable,
reorder, and both the const and keyframed parameter paths. The enum is
`#[non_exhaustive]`, so further kinds are added without a breaking change.

This reverses the flat `brightness`/`contrast`/`saturation` representation: those
fields are removed and folded into `EffectKind::ColorCorrect`. A neutral, all-constant
`ColorCorrect` compiles to no filter, preserving the prior "skip `eq` when neutral"
bit-identical output. The opaque `Vec<FilterStep>` (`video_effects`/`audio_effects`)
is **kept** additively; replacing that surface with the typed model is deferred to a
follow-up.

`EffectId` reuses ADR-0001's scheme: document-scoped, monotonic, never reused, minted
only by the document (`Timeline::build` and `edit::apply`). Effect ids are stamped when
a clip enters the document (`AddClip`), when a clip is created from another
(`SplitClip`'s right half), and when a whole clip value is installed (`SetClip`), and
the `Editor` carries a session high-water for the effect counter so an edit after
`undo` cannot re-mint a discarded id.

### Confirmation

`crates/avio/src/effect.rs` unit tests
(`color_correct_neutral_const_should_compile_to_nothing`,
`color_correct_non_neutral_const_should_compile_to_eq`,
`color_correct_animated_should_compile_to_eq_animated`,
`blur_const_should_compile_to_gblur`, `blur_animated_should_compile_to_gblur_animated`)
fail if the derive contract or the neutral-skip equivalence is broken.
`crates/avio/src/edit.rs` tests (`apply_add_effect_*`, `apply_remove_effect_*`,
`apply_set_effect_*`, `apply_reorder_effects_*`, `apply_*_should_stamp_effect_ids*`,
`apply_split_clip_should_remint_right_half_effect_ids`) and
`crates/avio/src/editor.rs`'s `editor_should_not_reuse_effect_ids_across_undo` fail if
the commands or the id invariant regress. `crates/avio/tests/serde_persistence.rs`'s
`clip_typed_effects_should_round_trip_through_serde` fails if the model does not
persist.

### Consequences

* Good: a host can present/edit effects parameter-by-parameter, keyframe any
  parameter, and reorder/toggle them, all undoable; preview and export share one
  derive so they stay equal.
* Good: the static color path is byte-identical (neutral `ColorCorrect` emits nothing;
  all-const emits the same `Eq`).
* Bad/limitation: two representations coexist during the additive phase (typed
  `effects` plus the opaque `video_effects`), until the deferred follow-up unifies them.
* Bad/limitation: v1 covers only `ColorCorrect`/`Blur`; other kinds (e.g. `Hue`, which
  has no animated `FilterStep`) are follow-ups.
* What would reverse this: moving effect compilation into a GPU node (#1365) could
  change what an `EffectKind` derives to, but not the authoring model itself.

## Pros and Cons of the Options

### A. Typed authoring layer in `avio`

* Good: id-addressed, ordered, keyframable, undoable; model-free primitives preserved;
  reuses ADR-0001 identity; derive keeps preview == export.
* Bad: two effect surfaces during the additive phase; a derive step (`to_filter_step`)
  to maintain per kind.

### B. Editing helpers over the opaque `Vec<FilterStep>`

* Good: no new types.
* Bad: `FilterStep` is an execution primitive with no id/enable/typed params; editing
  it means parse-and-rebuild, no stable identity, and leaks the execution layer into
  the host.

### C. Make `FilterStep` the authoring type

* Good: one representation.
* Bad: pushes model concerns (id, enable, keyframed params) into `ff-filter`, violating
  the engine/primitive boundary; every primitive consumer would carry authoring weight.

## More Information

* Issue #1458 (v0.18.0 axis A). Reuses the identity scheme of
  [ADR-0001](./0001-clip-and-track-identity.md) and the model-carries-animation
  boundary of [ADR-0002](./0002-per-clip-animation-in-the-model.md).
* `EffectKind::to_filter_step` (`crates/avio/src/effect.rs`); the id high-water in
  `crates/avio/src/editor.rs`.
