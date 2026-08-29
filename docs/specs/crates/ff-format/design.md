# ff-format

Provides the common types shared across crates so that every capability can handle media in the same language.

## Purpose

When each capability such as reading, processing, and writing represents media in a different language, translation is needed every time capabilities are connected, and mismatches and omissions arise. ff-format defines the basic elements of media such as frames, color and audio formats, codecs, and time positions as a shared vocabulary, so that all capabilities can hand media over in the same language. This makes the cooperation between capabilities consistent and lets them be combined with confidence.

## What it solves

- **A shared vocabulary** — represents media formats and attributes in a unified language shared by all capabilities.
- **Consistent handoff** — produces no translation or mismatch when passing media from capability to capability.
- **Preserving color and HDR information** — conveys important attributes such as color space and HDR without dropping them.
- **Consistent expression of time position** — handles positions within media in a common form that keeps precision.
- **FFmpeg independence** — keeps the shared vocabulary in a pure form not tied to any particular implementation.

## Capabilities

- Common types that represent the formats and attributes of video, audio, and images
- A representation of frames passed between capabilities as decode results and encode inputs
- A representation of color-related information such as color space, color gamut, and HDR
- A representation of time positions within media, and conversion of positions that keeps precision
- A consistent foundation for each capability to handle media in the same vocabulary
- An independent type system not tied to FFmpeg's internal structures

## Out of scope

- Reading, processing, or writing media itself
- Dependence on or coupling to FFmpeg's features
- Mechanisms for performance such as memory reuse
- The editing model such as timeline, editing, and history
