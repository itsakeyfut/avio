# ff-decode

Provides the ability to read video, audio, and images of many formats into a form the
application can work with.

## Purpose

Media files and streams come in countless containers, codecs, resolutions, and color
systems, so they cannot be used for editing, playback, or analysis as they are. ff-decode
absorbs these differences and lets input media be read out in a consistent form. Callers
can read media without any knowledge of the individual formats.

## What it solves

- **Uniform ingestion of diverse inputs** — read supported video, audio, and image formats
  without having to account for their differences.
- **Access to any position** — not only sequential reading from the start, but seeking to a
  specified time position.
- **Practical performance** — hardware acceleration and buffer reuse keep reading fast
  enough for real use.
- **Robustness** — keep reading as far as possible even with corrupt data or network
  interruptions.
- **Preview material** — obtain representative frames for lists and thumbnails.

## Capabilities

- Decoding of video, audio, and images (extraction frame by frame / sample by sample)
- Seeking to a specified time position
- Hardware acceleration (auto-selected where available)
- Ingestion of network inputs (URLs and streams)
- Asynchronous reading
- Representative-frame extraction and thumbnail generation

## Out of scope

- Writing out (encoding) or muxing
- Effects, compositing, and other processing
- Timeline, editing, and history (the editing model)
- Playback (clock, audio output, screen display)
