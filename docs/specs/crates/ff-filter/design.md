# ff-filter

Provides the ability to apply processing to video and audio (effects such as resizing, color adjustment, compositing, and fades).

## Purpose

Outputting a material as it is does not reach the intended look or sound. Processing such as size adjustment, color correction, overlaying multiple materials, and fades shapes the finished result. Applying such effects in combination normally requires specialized knowledge, but ff-filter makes it possible to **assemble the needed processing step by step and apply it to video and audio**, and to notice specifications that cannot hold before applying them.

## What it solves

- **Assembling effects** — compose processing such as resizing, color adjustment, compositing, and fades by layering it in order.
- **Specification that does not rely on specialized knowledge** — specify the processing on a purpose basis, without having to be conscious of the fine internal description.
- **Early detection of misspecification** — grasp out-of-range values and empty procedures before processing begins.
- **Compositing multiple materials** — overlay or mix two or more videos or audios into one.
- **Practical performance** — leverage hardware acceleration to process at a practical speed.

## Capabilities

- Video processing (resizing, cropping, rotation, trimming, aspect adjustment, and so on)
- Color and appearance effects (color adjustment, blur, noise removal, keying, HDR-to-SDR conversion, and so on)
- Temporal effects (fade in/out, transitions between clips)
- Overlaying and compositing text or other materials
- Audio processing (volume adjustment, parametric equalizer, mixing of multiple audios)
- Use of hardware acceleration

## Out of scope

- Reading materials (decoding) or writing them out (encoding)
- Extracting metadata
- The editing model, such as timelines, edits, and history
- GPU compositing (compositing as a rendering engine)
- Playback (clock, audio output, screen display)
