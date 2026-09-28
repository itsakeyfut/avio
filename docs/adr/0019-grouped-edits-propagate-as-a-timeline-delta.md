---
status: "accepted"
date: 2026-09-28
decision-makers: itsakeyfut
---

# A grouped edit propagates as a timeline-time delta, clamped per member

## Context and Problem Statement

`Command::GroupClips` links clips so they are edited together, which is what keeps a
camera clip's picture and its dialogue in sync. Which commands honoured the link was
decided one command at a time and drifted: `MoveClip`, `MoveClipToTrack` and
`RippleDelete` propagated, `TrimClip`, `RippleTrim` and `SplitClip` did not (#1813).
Trimming and razoring are the two most frequent operations performed on a linked pair,
so the feature looked correct until the first trim.

A razor made it worse. `split_clip` clones the clip, so both halves kept the original
`GroupId` and a later `MoveClip` on one half dragged the other half on the same track:
measured as `offsets=[7s, 9s]` after moving a right half to 9s, which tears the
material apart.

Extending propagation needs a rule that holds for every command, because "what does a
trim mean for a member with a different window, a different speed or less material" has
no answer in the code today and would otherwise be invented per command.

## Decision Drivers

* A group exists to protect timeline sync; the rule has to be stated in terms of what a
  viewer sees, not of the source values a clip happens to carry.
* `apply` is pure and performs no I/O, so it cannot know how long any source is.
* The same rule must serve trims and razors, or the drift this record ends will recur.

## Considered Options

* Propagate the timeline-time change of each edge.
* Propagate the source-time change of each edge.
* Copy the addressed clip's new in/out to every member.
* Refuse the whole command when any member cannot follow it.

## Decision Outcome

Chosen option: "propagate the timeline-time change", with a per-member clamp.

1. **A trim carries a delta, not a value.** The addressed clip's in/out change is
   divided by its `speed` to get the timeline-time change, and multiplied by each
   member's own `speed` to get back to that member's source time. When speeds match
   this is identical to a plain source delta; when they differ it is what keeps the
   timeline edges together. It mirrors `shift_group_offsets`, which already carries the
   *delta* of `offset` rather than an absolute position.
2. **An edge the trim cannot express does not propagate.** An unset in-point is exactly
   the start of the file, so a head trim always has a delta. An unset out-point means
   "to end of file", a position `apply` cannot know, so an out-point that is unset
   before or after the trim leaves every member's tail alone. An edge whose delta is
   zero is left untouched, unset included, so a no-op trim cannot give a member an
   explicit window it did not have (which would change `Clip::duration` from unknown to
   known and make a later ripple move clips that should not move).
3. **A member that cannot follow is clamped, not refused.** The only clamps available
   without I/O are `in >= 0` and `in <= out`; the in-point is pulled back rather than
   the out-point pushed out, because moving an out-point beyond where the trim put it
   would claim material that may not exist. The addressed clip itself is never clamped:
   the caller named it. A clamped member is where a grouped trim loses sync, and that
   is the accepted price of not refusing a trim because a linked clip is fractionally
   shorter.
4. **A razor cuts every member whose span contains the cut**, and leaves the others
   whole: a group whose members are not aligned stays editable. The addressed clip must
   still be cuttable, or the command fails with `EditError::SplitOutOfRange` and
   changes nothing.
5. **The right halves form one new group.** The left halves keep the original
   `GroupId`, every right half takes one fresh one. Audio and video stay linked on each
   side of the cut, and the two sides stop dragging each other.

### Confirmation

In `crates/avio/src/edit.rs`: `trim_clip_should_propagate_to_every_group_member`,
`trim_clip_should_carry_the_delta_not_the_value_to_a_member`,
`trim_propagation_should_scale_by_each_member_speed` (the one that separates this rule
from a source-time delta), `trim_propagation_should_clamp_a_member_instead_of_inverting_it`,
`trim_should_not_propagate_an_unset_out_point`,
`ripple_trim_should_propagate_to_every_group_member_and_ripple_each_track`,
`split_clip_should_razor_every_group_member_that_spans_the_cut`,
`split_clip_should_skip_a_group_member_that_does_not_span_the_cut`,
`split_clip_should_put_the_right_halves_in_one_new_group`,
`moving_one_half_after_a_grouped_split_should_not_move_the_other_half` and
`split_clip_should_leave_an_ungrouped_clip_ungrouped`. Across commands,
`every_propagating_command_should_reach_the_linked_member`
(`crates/avio/tests/group_propagation.rs`) walks each propagating command in turn and
fails as soon as one stops propagating.

### Consequences

* Good, because a linked pair survives the two most frequent edits, and the rule is
  written once instead of per command.
* Good, because a razor no longer produces a group spanning both sides of the cut.
* Bad, because a clamped member is out of sync with the rest of its group and the
  model cannot detect by how much, since source lengths are unknown to `apply`. A
  member clamped all the way to an empty window is at least visible: `Timeline::validate`
  reports it as `EmptyFootprint`, which `trim_propagation_should_clamp_a_member_instead_of_inverting_it`
  asserts.
* Bad, because a member whose out-point is unset cannot follow a tail trim at all, so a
  group mixing explicit and open-ended windows is partly unprotected.
* `Command::RemoveClip` stays deliberately single-clip (`RippleDelete` is the grouped
  removal), and `SetClip` / `SetClipProperty` do not propagate: they carry per-clip
  values, not a change with a timeline meaning.
* What would reverse this: giving the model resolved source durations (an edit that
  carries them the way it already carries canvas and fps), which would let a clamp
  become a refusal and let an unset out-point follow a trim.

## Pros and Cons of the Options

### Timeline-time delta

* Good, because it states the rule in the terms the group exists to protect.
* Good, because it degenerates to the obvious behaviour when speeds match.
* Bad, because it costs a division and a multiplication per member and an extra test to
  pin the case that separates it from the simpler rule.

### Source-time delta

* Good, because it is the least code.
* Bad, because a member at a different speed ends up at a different point on the
  timeline, which is the very failure the group exists to prevent.

### Copy the new in/out to every member

* Good, because it needs no arithmetic at all.
* Bad, because it destroys any member whose window differs, which is every pair edited
  from separate files.

### Refuse when a member cannot follow

* Good, because nothing is ever silently out of sync.
* Bad, because `apply` cannot see source lengths, so the refusal would be triggered by
  the arithmetic rather than by the material, and a trim would fail for reasons the
  user cannot see.

## More Information

* #1813 for the measurement, including the `offsets=[7s, 9s]` tear after a grouped
  razor.
* `crates/avio/src/edit.rs`: `trim_deltas`, `trimmed_edges`, `ripple_trim_one`,
  `split_one`.
* ADR-0001 for clip and group identity, which is why the right halves take a fresh
  `GroupId` rather than reusing one.
