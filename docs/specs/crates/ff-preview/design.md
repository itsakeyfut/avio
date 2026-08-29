# ff-preview

Provides the ability to check results instantly while editing and to keep working smoothly even with heavy material.

## Purpose

In editing work, being able to confirm the result of a change on the spot is indispensable. High-resolution material, however, is heavy to play back as-is, and waiting on every check stalls the work. ff-preview lets **the video being edited be played back and checked instantly, while heavy material is replaced with lightweight proxy data for comfortable handling**. Callers do not build their own playback and checking machinery; they can focus on confirming how the edit looks.

## What it solves

- **Instant result confirmation** — play back the video being edited on the spot and confirm the changes made.
- **Confirmation at a targeted position** — move precisely to any time position and check the video at that moment.
- **Audio-video synchronization** — during playback, keep audio and video from drifting so the result is checked close to how it will actually look.
- **Smooth handling of heavy material** — replace high-resolution material with lightweight proxy data and work comfortably.
- **Automatic switching from proxy to original** — check with lightweight data and switch to the original material where needed.

## Capabilities

- Instant playback of the video being edited (play, pause, and stop operations)
- Precise movement to any time position and confirmation there
- Audio-synchronized playback of audio and video
- Handoff of video in a form suited to rendering, where the display target can be prepared by the caller
- Generation of lightweight proxy data for high-resolution material
- Automatic use of proxy data versus original material as appropriate

## Out of scope

- Writing out (encoding) the finished video or output for distribution
- The editing model itself such as timeline, clips, editing, and history
- The substance of video processing such as effects and compositing
- Control of screen display, window management, and audio output devices
