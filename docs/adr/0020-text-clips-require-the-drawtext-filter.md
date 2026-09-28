---
status: "accepted"
date: 2026-09-28
decision-makers: itsakeyfut
---

# Text clips stay on `drawtext`, and the capability is asked rather than assumed

## Context and Problem Statement

`ClipSource::Text` is a first-class part of the editing model, but rendering one failed on the
install this repository documents for Windows (#1809):

```
render FAILED: filter failed: composition failed: failed to build text drawtext layer=1
```

Read off the linked FFmpeg 8.0.1 rather than the manual:

```
CONFIG_DRAWTEXT_FILTER 0     CONFIG_LIBFREETYPE 0     CONFIG_LIBFONTCONFIG 0
CONFIG_ASS_FILTER      1     CONFIG_SUBTITLES_FILTER 1    CONFIG_LIBASS 1
```

`drawtext` needs freetype, which vcpkg's default `ffmpeg` triplet does not build. Three things
followed: the documented install could not render text at all, the failure arrived only at render
time in words that name neither the missing piece nor the cure, and `Timeline::validate` returned
`[]` for the very timeline that was about to fail.

The same build carries **libass**, so a second text-capable route exists in it. That is what makes
this a decision rather than a bug fix.

## Decision Drivers

* A published crate's model accepts text; a user following the project's own instructions must be
  able to render it, or be told exactly why not.
* A host building a UI needs the answer before the user makes a text clip, not at export.
* An `FFmpeg` capability is a property of the linked build, so anything that claims one has to be
  checked against that build rather than assumed.

## Considered Options

* Keep `drawtext` and fix the documented install.
* Render text through libass when `drawtext` is absent.
* Replace `drawtext` with a Rust text rasteriser.

## Decision Outcome

Chosen option: "keep `drawtext` and fix the documented install", plus a capability query that both a
host and `validate` can ask.

1. **Text stays on `drawtext`.** A libass fallback would work on the documented install untouched,
   but it means owning ASS generation, a second mapping for `anchor` / `offset` / `box_*`, and a
   standing obligation that the two routes produce the same picture. The measurement above is
   recorded here so the option stays open; #1809's own notes raise the Rust-rasteriser variant of the
   same question.
2. **The install documentation names the requirement.** Every copy moves to
   `vcpkg install ffmpeg[core,drawtext]:x64-windows`, confirmed to resolve with
   `vcpkg install ... --dry-run` (the `drawtext` feature is `ffmpeg[freetype]` plus `harfbuzz`).
3. **The capability is queryable**: `ff_filter::text_rendering_available()`, with
   `ff_filter::TEXT_FILTER` naming the filter so a caller can say what is missing without hard-coding
   FFmpeg vocabulary. Both are re-exported from `avio`.
4. **The primitive names the filter; the engine names the cure.** `ff-filter` keeps the message shape
   it already uses (`filter not found: scale`); the install advice is environment-specific and lives
   in `avio`'s `TimelineError::TextRendererUnavailable` and in the documentation.
5. **`validate` may consult the linked build.** `TimelineIssue::TextRendererUnavailable` is the first
   check here whose answer depends on the machine rather than on the document: the same timeline is
   clean on a build that has the filter. `validate`'s promise of performing no I/O still holds, since
   a filter lookup opens nothing.

### Confirmation

`text_rendering_available_should_agree_with_building_a_text_source`
(`crates/ff-filter/src/capability.rs`) is the one test grounded outside the query: it checks the
claim against `TextSource::new` actually succeeding, and it is what fails when the probe is made to
lie. `text_filter_should_name_the_filter_the_text_layer_builds` pins `TEXT_FILTER` to the name
`FilterStep::DrawText` builds under. `text_clip_should_be_flagged_when_its_renderer_is_unavailable`
and `a_timeline_without_text_should_not_be_flagged_for_the_text_renderer`
(`crates/avio/src/validate.rs`) cover the mirror, and
`rendering_a_text_clip_should_be_refused_by_name_when_the_build_cannot_draw_it`
(`crates/avio/tests/text_renderer_gate.rs`) covers the render-time refusal and its message.

### Consequences

* Good, because a user following the documentation can render text, and one who cannot is told which
  filter is missing and what to install.
* Good, because a host can disable its text tool before a clip exists.
* Bad, because text remains tied to an optional filter in someone else's build; a package that drops
  freetype disables a first-class model feature, and this record only makes that visible rather than
  fixing it.
* Bad, because `validate` is no longer a pure function of the document, so two machines can disagree
  about the same timeline. The variant's documentation says so.
* Tests that assert agreement with the capability query cannot catch a query that lies; exactly one
  test is grounded against the real graph, and it is named above.
* What would reverse this: a text route that does not depend on an optional FFmpeg filter, whether
  through libass (present in more builds) or a Rust rasteriser.

## Pros and Cons of the Options

### Keep `drawtext`, fix the documentation

* Good, because the renderer, the styling mapping and the tests stay as they are.
* Good, because the install command is verifiable today.
* Bad, because it asks the user to rebuild FFmpeg.

### A libass fallback

* Good, because the documented install would work with no rebuild.
* Bad, because two text routes must agree forever, and ASS styling is not a subset of the current
  `TextStyle`.

### A Rust text rasteriser

* Good, because it removes the dependency on someone else's build entirely.
* Bad, because it means owning font discovery, shaping and layout, which is a project rather than a
  fix.

## More Information

* #1809 for the reproduction, the `config_components.h` reading and the vcpkg dry run.
* `crates/ff-filter/src/capability.rs`, `crates/avio/src/validate.rs`, `crates/avio/src/timeline.rs`.
* RK-002 in the review knowledge bank is the same lesson from the test side: CI's FFmpeg is a minimal
  build, so a test must probe rather than assume.
