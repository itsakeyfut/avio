---
status: "accepted"
date: 2026-09-29
decision-makers: itsakeyfut
---

# A locked track refuses edits in the model, and `SetTrackLock` is the way out

## Context and Problem Statement

`Track::lock` was documented as "Authoring flag protecting the track from edits; ignored by the
derivation". The second half held; the first did not. Measured on the code before this record:

```
lock flag = true
MoveClip on a locked track    -> Ok, offset = 99s
RemoveTrack on a locked track -> Ok, tracks = 0
```

The flag was read nowhere outside `track.rs`. It round-tripped through serde and constrained nothing,
so a host drawing a lock icon was the only thing enforcing it (#1805).

Two readings were open, and the issue named both: the model enforces the flag, or the flag is a hint
the host honours and the documentation should say so. The state to avoid was the one that existed,
where the documentation promised protection and nothing delivered it.

A second fact shaped the answer. `lock` could only be set by `Track::locked` at build time: no command
touched a track's flags and `Timeline` exposes its tracks immutably, so enforcing the flag without
adding a way to clear it would have made a locked track permanently uneditable through the API.

## Decision Drivers

* A model that documents protection should provide it, or every host reimplements the same check and
  any path that skips the host's UI (a batch, a macro, a script) walks through the lock.
* The flag must not be able to strand a document.
* Whatever the edit path does, the render path must not change: a locked track is still part of the
  programme.

## Considered Options

* Enforce the lock in `apply`.
* Document the flag as a host hint and leave the model out of it.

## Decision Outcome

Chosen option: "enforce it in `apply`", with a command to release it.

1. **`apply` refuses an edit aimed at a locked track**, returning
   `EditError::TrackLocked { id }` before anything is cloned or changed. One guard at the top of
   `apply` covers every command, and `Command::Batch` re-enters `apply` per sub-command, so a batch
   containing a locked edit changes nothing.
2. **The lock covers the clips and the track.** Commands that change a clip on the track, add a clip
   to it, or remove the track are refused. `MoveClipToTrack` is refused when either the source or the
   destination is locked.
3. **A grouped edit that reaches a locked member is refused whole.** The commands that propagate to a
   group (ADR-0019) check every member's track. Skipping the locked member instead would silently
   break the A/V sync the group exists to protect. `Command::UngroupClips` is checked the same way
   although it does not propagate an edit: it clears `group` on **every** member, so the members'
   tracks are what it writes to.
4. **`Command::SetTrackLock` is added and is exempt**, because it is how the lock is released. It is
   the only command a locked track accepts.
5. **The derivation still ignores the flag.** `Track::is_active` reads `enabled` / `mute` / `solo`
   and nothing else, so a locked track renders and previews exactly as an unlocked one.

### Confirmation

In `crates/avio/src/edit.rs`: `move_clip_on_a_locked_track_should_be_refused`,
`add_clip_to_a_locked_track_should_be_refused`,
`remove_track_should_be_refused_when_the_track_is_locked`,
`move_clip_to_a_locked_destination_should_be_refused`,
`an_effect_command_on_a_locked_track_should_be_refused`,
`a_grouped_edit_reaching_a_locked_member_should_be_refused`,
`a_grouped_move_should_be_refused_when_a_member_is_locked`,
`ungrouping_from_a_free_member_should_be_refused_when_another_is_locked`,
`set_track_lock_should_release_a_locked_track` (the exemption, without which a lock is permanent),
`a_batch_containing_a_locked_edit_should_change_nothing` and
`timeline_level_commands_should_be_unaffected_by_a_lock`.

For the derivation half: `is_active_should_ignore_the_lock_flag` (`crates/avio/src/track.rs`) and
`a_locked_track_should_render_the_same_as_an_unlocked_one`
(`crates/avio/tests/locked_track_render.rs`), which renders the same timeline with the flag on and
off and compares the frames read back.

### Consequences

* Good, because the flag now means what its name says on every path, including the ones that do not
  go through a host's UI.
* Good, because a host that wants the old behaviour can simply not set the flag.
* Bad, because a host that locked a track through an older version of the model and never carries
  `SetTrackLock` in its command vocabulary will find those tracks refusing edits. The error message
  names the command that releases them.
* The wider gap stays open: no command edits `name`, `mute`, `solo` or `enabled` either, so those
  four remain build-time only. This record does not close that; it adds the one command the lock
  makes indispensable.
* What would reverse this: deciding the model should stay out of authoring policy, which would mean
  removing the guard and rewording `Track::lock` to say who is responsible.

## Pros and Cons of the Options

### Enforce it in `apply`

* Good, because the protection holds for every caller and every path.
* Good, because the refusal is a typed error a host can present.
* Bad, because it needs a command to release the lock, which is API surface the issue did not ask
  for.

### Document it as a host hint

* Good, because it is a documentation change and nothing else.
* Bad, because every host reimplements the same check, and any of them that forgets destroys a
  locked track's contents with no signal from the model.

## More Information

* #1805 for the measurement and for both readings.
* ADR-0001 (ids and stale references) and ADR-0019 (grouped edits propagate), whose rules this one
  has to sit beside.
* `crates/avio/src/edit.rs`: `locked_target`, the guard at the top of `apply`.
