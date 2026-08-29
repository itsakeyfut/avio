# ff-encode

Provides the ability to "write out" edited and processed video, audio, and images in a format suited to their purpose.

## Purpose

The results of editing and conversion generate no value until they are assembled into a form that can finally be distributed, stored, or played back. Yet output targets serve diverse purposes, and the suitable codec and quality settings differ greatly depending on whether the destination is web delivery, archiving, professional use, or something else. ff-encode makes it possible to **reliably write out deliverables in a format that fits their purpose**, and to notice inconsistent settings at an early stage.

## What it solves

- **Writing out to diverse output formats** — produce deliverables by choosing the codec and container that suit the purpose.
- **Control over quality and size** — adjust to the goal with constant bitrate, quality criteria, variable bitrate, and the like.
- **Early detection of misconfiguration** — grasp combinations that cannot hold before writing begins.
- **Support for specialized uses** — output across a broad range, from delivery to archiving, professional formats, and HDR.
- **Insight into progress** — check how far a long write has advanced.

## Capabilities

- Writing out video, audio, and images
- A broad choice of codecs and containers, with detailed per-codec quality and characteristic specification
- A choice of quality modes such as constant bitrate, quality criteria, or variable bitrate
- Support for professional formats (ProRes, DNxHD/HR, and so on) and HDR metadata
- Use of hardware encoding, with automatic fallback to software when it is unavailable
- Notification of write progress
- Asynchronous writing

## Out of scope

- Reading materials (decoding) or extracting metadata
- Processing such as effects and compositing
- The editing model, such as timelines, edits, and history
- Stream copy without re-encoding (mux-only processing)
- Playback (clock, audio output, screen display)
