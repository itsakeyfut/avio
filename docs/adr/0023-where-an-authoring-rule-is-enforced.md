---
status: "accepted"
date: 2026-09-29
decision-makers: itsakeyfut
---

# An authoring rule is enforced where it can afford to be: `apply` is pure, `build` may read files, `validate` only reports

## Context and Problem Statement

Two open issues asked the same question and would have answered it twice. #1927 found that
`MoveClipToTrack` accepts a destination of the wrong kind, so a title can land on an audio track and
silently vanish at render; #1850 found that a source the render cannot use is discovered at export
rather than when the clip is added. Both had to decide whether the rule belongs on the edit path, at
build, or in `validate`, and #1850's fifth criterion asks explicitly for the relationship to `validate`
to be recorded so a timeline is not checked in two places with two answers.

## Decision Drivers

* `apply(&Timeline, &Command) -> Timeline` is pure by the immutable-model design (#1327, ADR-0019).
* `validate` was given an advisory contract by #1816, and #1809 established that it may ask the linked
  library about a capability.
* `TimelineBuilder::build` already reads files, which both issue bodies assumed would be a new cost.
* A rule nobody can reach from the API that creates the bad state is not enforced.

## Considered Options

* Split the enforcement by what each site can afford
* Put every authoring rule on the edit path, doing I/O there when needed
* Put every authoring rule in `validate` and let the render fail

## Decision Outcome

Chosen option: "split by what each site can afford".

| Site | Cost it may pay | What it does |
|---|---|---|
| `apply` | pure: no I/O, no registry | **refuses** an edit decidable from the timeline value alone |
| `TimelineBuilder::build` | file I/O | **refuses** a timeline whose sources the render cannot use |
| `Timeline::validate` | the linked library's registry, but no file I/O | **reports**, never refuses |

`apply` cannot do I/O because its result must depend only on `(timeline, command)`. With a filesystem
read inside it, the same edit replayed on the same value can give a different answer, and undo and redo
stop being deterministic. That is what forces the split: a rule can be conceptually an edit-path rule
and still be unable to live there.

`build` is where the I/O already is. `resolve_canvas_and_fps` checks `source.exists()` and opens a
`VideoDecoder` on the first video clip whenever the canvas or frame rate is implicit, so asking the
build what it can decode is an existing mechanism applied more widely, not a new kind of work.

`validate` keeps #1816's contract. It may query the linked build (#1809's `TextRendererUnavailable`),
because that opens nothing, and it does no file I/O, so it never produces a second answer to a question
`build` already answered. It is also the only site a deserialized `Timeline` passes, which is why a rule
enforced in `apply` and `build` still needs an advisory twin here.

A rule therefore lands in one, two or three of these places depending on what deciding it costs, and the
first consequence is that **a rule with an I/O-free part and an I/O-dependent part is split, not
deferred**. #1927 is the worked example: a generated (`Text`/`Solid`) source carries no audio, which the
model knows, so that half is refused in `apply` and in `build` and reported by `validate`; whether a
*file* can serve a track needs a probe, so that half is #1850's and `apply` deliberately accepts it.

The order of guards within `apply` is also fixed: the lock is consulted first (ADR-0021), so an edit that
is both locked out and otherwise invalid reports the lock.

### Confirmation

`a_locked_track_should_be_refused_before_the_kind_rule` (`crates/avio/src/edit.rs`) fails if the guard
order is reversed. `add_clip_should_refuse_a_generated_source_on_an_audio_track`,
`move_clip_to_track_should_refuse_a_generated_source_on_an_audio_track` and
`set_clip_should_refuse_a_patch_that_cannot_serve_the_holding_track` fail if the pure half leaves
`apply`; `move_clip_to_track_should_accept_a_file_source_on_an_audio_track` and
`build_should_accept_a_file_clip_on_an_audio_track` (`crates/avio/src/timeline.rs`) fail if the
I/O-dependent half is decided there anyway. `build_should_refuse_a_generated_clip_on_an_audio_track`
fails if `build` stops refusing what the builder can construct directly, and
`validate_should_report_a_clip_that_cannot_serve_its_track` (`crates/avio/src/validate.rs`) fails if the
advisory twin stops reporting, or reports the wrong ids.

### Consequences

* Good, because each rule is enforced at the earliest site that can decide it, and the reason a rule is
  *not* enforced earlier is a property of the site rather than an oversight.
* Good, because `validate`'s contract survives contact with two rules that wanted to break it.
* Bad, because one rule can now live in three places, and a reader has to follow the cross-links to see
  the whole of it. The variant docs carry them in both directions.
* Bad, because a deserialized `Timeline` is still only advised, never refused. The render keeps the
  defensive `continue`s that skip a clip carrying no audio while summing an audio track
  (`crates/avio/src/timeline.rs`), and they are deliberately left in place and deliberately not covered
  by a render test: `render()` silently picks the GPU route when the timeline is eligible, and a
  generated clip with no out-point has nothing to terminate the graph, so such a test would prove
  little and could hang.
* What would reverse this: making `apply` fallible in a way that admits I/O, which would mean giving up
  the determinism #1327 bought.

## Pros and Cons of the Options

### Split by what each site can afford

* Good, because it is forced by `apply`'s purity rather than chosen by taste.
* Good, because it explains why two issues that look like one are two.
* Bad, because the split has to be re-derived for each new rule.

### Every rule on the edit path

* Good, because a host would have exactly one place to look.
* Bad, because `apply` would have to read files, and an edit's outcome would depend on the filesystem at
  the moment it ran. Undo, redo and replay would stop agreeing.

### Every rule in `validate`

* Good, because nothing would ever be refused, so no API breaks.
* Bad, because `validate` is advisory; a host that does not call it would still build a document that
  cannot render, which is the defect #1927 and #1850 describe.

## More Information

* #1927 and #1850, and the joint design comment on #1927 that this record formalises.
* ADR-0019 (the immutable model and grouped edits), ADR-0021 (a locked track refuses edits), #1816
  (`validate` is advisory), #1809 (`validate` may ask the linked build).
