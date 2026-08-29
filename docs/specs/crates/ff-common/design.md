# ff-common

Provides a shared foundation for reusing memory efficiently in media processing and keeping practical speed.

## Purpose

In video and audio processing, large frame buffers are repeatedly acquired and released every frame, which becomes a cause of slowdown and memory pressure. ff-common provides, as a shared foundation, a mechanism that reuses memory once finished with instead of discarding it, curbing wasteful acquisition and release. By sharing this foundation, each capability obtains practical performance without building memory management individually.

## What it solves

- **Memory reuse** — instead of letting go of a buffer once finished with, reuses it in later processing to reduce acquisition cost.
- **Fewer acquisitions and releases** — avoids repeated allocation and prevents slowdown across the whole process.
- **Automatic cleanup** — a buffer once finished with automatically returns to the reuse pool, requiring no manual release.
- **Safe sharing** — the same reuse mechanism can be used safely from multiple processes.
- **Commonization** — each capability shares a unified foundation rather than building memory management individually.

## Capabilities

- A reuse mechanism for the buffers used in media processing
- A reuse framework that can adjust how much is kept on hand according to usage
- Automatic return and re-acquisition of buffers once finished with
- Consistent handling that works even for one-off use with no reuse pool
- Safe shared use from multiple processes
- A lightweight, independent foundation with no dependence on FFmpeg

## Out of scope

- Reading, writing, or processing video, audio, and images themselves
- Dependence on or coupling to FFmpeg's features
- Defining the common types that represent media
- The editing model such as timeline, editing, and history
