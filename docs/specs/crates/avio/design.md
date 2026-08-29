# avio

The core of a video-editing application, handling everything end to end: arranging sources on a timeline, editing them, and going from preview through export.

## Purpose

Building a video-editing application requires not only the individual features of reading in, compositing, and writing out sources, but also a mechanism that binds them together and handles the editing itself: "which source is placed where, arranged how, and processed how." avio takes on this core as the **body of the editing engine**, representing the edit content as data and deriving video from it. Callers can achieve everything from editing to output simply by assembling a timeline, without delving into the details of each feature.

## What it solves

- **Representing edits as data** — the placement, trim, opacity, transitions, and more of sources can be described consistently in the form of a timeline.
- **Non-destructive editing** — the original sources are not rewritten and only the edit content is manipulated, so it can be recomposed at any time.
- **Undoable editing** — every editing operation can be undone and redone one step at a time, so you can experiment with confidence.
- **Turning edits into video** — from the timeline, derive and export video that reflects compositing, transitions, and effects.
- **Consistency of preview and output** — the preview during editing and the final export are obtained from the same edit content.
- **Grasping source information** — the resolution, frame rate, codec, and so on of the sources to be read in can be checked in advance.

## Capabilities

- Placing and editing clips on the timeline (video and audio tracks)
- Per-clip trim, placement position, opacity, and transition specification
- Compositing by layering multiple tracks
- Applying editing operations, with history management involving undo and redo
- Exporting the edit result to a file
- Real-time preview (on supported configurations)
- Checking source metadata
- A consistent entry point that binds together the various media features (decode, encode, analysis, compositing, and so on)

## Out of scope

- The internal implementation of individual processes such as decode, encode, and compositing (delegated to lower-level features)
- Any specific screen UI or application operation scheme
- Management and storage of the source files themselves
- Low-level media processing used on its own (uses that call each feature directly are delegated to the lower levels)
