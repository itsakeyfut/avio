---
status: accepted
date: 2026-08-19
decision-makers: itsakeyfut
---

# Address clips and tracks by a document-scoped, monotonic `u64` id

## Context and Problem Statement

`avio` is the editing engine, and #1414 established that its model must be usable
as a host's single editing document. The model addressed clips and tracks
**positionally**: `Timeline` stored `Vec<Vec<Clip>>` and a `Command` carried
`(TrackId{kind, index}, clip_index)`. Any insert, remove, or reorder invalidates
an in-flight edit, the document cannot be serialized as a stable reference, and a
host cannot key its UI state (selection, thumbnails) to a clip across edits.

#1416 introduces a stable identity for clips and tracks. This record decides how
those ids are represented and assigned. The codebase already relies on this
decision (it is implemented in #1416), so the record is `accepted`, not
`proposed`.

## Decision Drivers

* Ids must be stable across insert, remove, reorder, and undo/redo. This is the
  whole point of the change.
* The model is immutable and declarative: `apply` clones the document, and
  `Editor` stores full `Timeline` snapshots. The scheme must fit that.
* Clip and track order is semantically meaningful (track 0 is the bottom layer;
  clip order within a track drives transitions), so storage stays ordered.
* Ids must be deterministic and serializable for project save/load (#1426).
* Prefer the minimal scheme the current scale needs. Editing is paced by user
  gestures over tens to hundreds of objects, not a per-frame hot path.

## Considered Options

* A **monotonic per-document `u64` counter**; `Clip`/`Track` carry an id;
  resolution is a linear scan.
* A **generational-index arena** (the `slotmap` crate): `SlotMap<Key, Clip>` plus
  a per-track `Vec<Key>` to keep order.
* A **UUID/GUID** per object.
* **Positional addressing** (the status quo).

## Decision Outcome

Chosen option: a **monotonic per-document `u64` counter** held in the `Timeline`
(`next_clip_id` / `next_track_id`). `ClipId(u64)` and `TrackId(u64)` are opaque
newtypes; `Clip::new` leaves `ClipId::UNSET` until the clip is placed; the
document stamps a fresh id at `build()`, `Command::AddClip`, and
`Command::AddTrack`; ids are never reused; commands resolve an id by linear scan.

This matches how in-memory NLE models commonly represent identity: compact,
deterministic, and trivially serializable. Because ids are never reused, a command
naming a removed clip resolves to no track and returns an `EditError`, giving the
same stale-reference safety a generational index provides, without a second data
structure or a generation counter. It preserves the ordered-`Vec` storage the
derivation already depends on, and fits the immutable/full-snapshot model (undo
and redo restore whole documents, so ids come back intact).

UUIDs are what interchange and asset boundaries need (FCPXML string ids, AAF
MobIDs); that layer is deferred to the backlog. A `slotmap` arena or a persistent
ordered map pays off at larger scale, or when structural-sharing history (#1352)
lands; neither is needed now.

### Confirmation

Unit tests in `crates/avio/src/edit.rs` fail if the scheme is violated:

* `build_should_assign_set_and_unique_ids` - ids are set and unique after build.
* `apply_add_clip_should_append_with_fresh_id`,
  `apply_add_track_should_append_empty_track_with_fresh_id` - added objects get a
  fresh, distinct id.
* `apply_should_preserve_clip_ids_across_an_unrelated_edit` - ids are stable
  across an unrelated edit.
* `apply_unknown_clip_should_err`, `apply_unknown_track_should_err` - an absent
  or removed id resolves to an `EditError` (the stale-reference safety).

The monotonic, never-reused property is a code invariant: the counters only
increment, in `Timeline::build` and `apply`. Serialization determinism will be
confirmed by the serde round-trip test in #1426.

### Consequences

* Good, because ids survive edits and undo/redo, are deterministic and
  serde-ready, and add minimal code over the existing ordered `Vec`s.
* Good, because resolution is `O(n)` but editing is gesture-paced, so it is not a
  hot path.
* Bad, because ids are unique only within one document; cross-document copy/paste
  needs remapping (acceptable until interchange exists).
* Bad, because each command does a linear scan; if a profile ever shows it matters
  at scale, an index map is a localized fix.
* What would reverse this: adding cross-project copy/paste or interchange (move to
  UUIDs), or structural-sharing history and large-scale random-access editing
  making the scan or the ordered-`Vec` clone costly (move to `slotmap` or a
  persistent ordered map). A reversal is a new record that supersedes this one.

## Pros and Cons of the Options

### Monotonic per-document `u64` counter (chosen)

* Good, because it is compact, deterministic, and serde-trivial.
* Good, because it fits the immutable/snapshot model and the ordered storage.
* Good, because never-reused ids give stale-reference safety with no extra
  machinery.
* Bad, because it is document-scoped only, and resolution is `O(n)`.

### Generational-index arena (`slotmap`)

* Good, because resolution is `O(1)` and it detects stale keys via the generation.
* Good, because storage is decoupled from order.
* Bad, because it is unordered, so it still needs a parallel `Vec<Key>` for layer
  and sequence order.
* Bad, because a mutable arena fits poorly with the immutable/snapshot model and
  the future persistent-sharing direction (#1352), and the generation is redundant
  when ids are never reused.

### UUID/GUID

* Good, because it is globally unique and safe across documents; it is what
  interchange formats use.
* Bad, because it is 128-bit and not human-friendly, and is not needed until
  cross-document or interchange work exists.

### Positional addressing (status quo)

* Good, because there is no id field and storage is simplest.
* Bad, because it is invalidated by insert/remove/reorder, is not a stable
  serializable reference, and cannot key host UI state. This is the defect #1414
  recorded.

## More Information

* Requirement catalog: #1414 (A7 stable identity, A8 serde). Implemented in #1416.
* Design of record: `docs/specs/engine-and-primitives.md`;
  `docs/roadmap/v0-17-0/ROADMAP.md`.
* Code: `crates/avio/src/ids.rs` (`ClipId`/`TrackId`), `track.rs` (`Track`),
  `timeline.rs` (counters and `build` stamping), `edit.rs` (id-addressed commands
  and `apply`).
* Industry references: FCPXML element ids, AAF MobID (SMPTE UMID), and the
  `slotmap` crate (generational index).
