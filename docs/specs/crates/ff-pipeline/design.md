# ff-pipeline

Provides the ability to run reading, processing, and writing out as a single connected operation, executed together.

## Purpose

Transforming media requires connecting several stages in sequence: reading the input, applying the necessary processing, and writing it out in a specified format. Assembling each stage individually invites configuration mismatches and rework. ff-pipeline lets these be **composed as a single validated flow and executed together**. Callers do not manage each stage individually; they simply specify the input, output, and processing content to run a consistent transformation.

## What it solves

- **One-shot execution of a connected operation** — run everything from reading through processing to writing out together, without assembling it piece by piece.
- **Rework prevention through up-front validation** — detect configuration flaws before processing begins and avoid wasted runs.
- **Progress visibility** — receive how far processing has advanced at any time and present it to the user.
- **Mid-way cancellation** — safely stop even a long operation at any time at the user's discretion.
- **Errors that identify the cause** — report failures in a form that traces what happened at which stage.

## Capabilities

- Composition of a transformation with specified input, output, quality settings, and processing content
- Pre-execution validation of settings (processing does not start if there are flaws)
- Specification of output format and quality (codec, bitrate, resolution, and so on)
- Execution of a transformation with processing stages in between
- Ongoing progress notification
- Cancellation of processing at the user's discretion
- Failure notification including which stage it originated from

## Out of scope

- The internal means of realizing each of reading, processing, and writing out (delegated to other foundational capabilities)
- The editing model such as timeline, clips, editing, and history
- Adaptive streaming output for distribution
- Real-time preview or playback during editing
