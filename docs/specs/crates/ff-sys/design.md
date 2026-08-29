# ff-sys

Provides the foundation that lets the application safely use FFmpeg's power for video and audio processing.

## Purpose

Actually processing video and audio depends on FFmpeg's capabilities, but its entry point is very hard to handle, and a small mistake can lead to incorrect behavior or memory corruption. ff-sys consolidates the contact surface with FFmpeg into a single place and confines dangerous handling inside this layer. This lets each higher-level capability move forward on a stable foundation without worrying about FFmpeg's details or pitfalls.

## What it solves

- **Making FFmpeg safe to use** — wraps easily misused operations into a safe form so higher levels never touch dangerous handling.
- **Automatic cleanup** — releases acquired resources without ever missing one, preventing memory leaks and double frees.
- **Clarity about supported versions** — absorbs FFmpeg's version differences and specification changes at this layer.
- **Absorbing per-environment setup** — automatically locates FFmpeg, whose whereabouts differ per OS, and makes the build succeed.
- **Centralizing the danger boundary** — confines risky processing to this layer alone and keeps higher levels in a safe world.

## Capabilities

- A foundation on which higher-level crates can safely call FFmpeg's features
- Acquisition of the resources needed for video, audio, and image processing, and their automatic cleanup
- Guaranteed behavior against the assumed FFmpeg version and absorption of its differences
- FFmpeg detection and setup support on Windows, Linux, and macOS respectively
- Confirmation of whether the linked FFmpeg provides a given feature
- A means to handle FFmpeg-originated errors in a readable form

## Out of scope

- Providing concrete media processing such as decoding and encoding
- High-level APIs intended for direct use by callers
- The editing model such as timeline, editing, and history
- Bundling or distributing FFmpeg itself (it uses the FFmpeg provided by the environment)
