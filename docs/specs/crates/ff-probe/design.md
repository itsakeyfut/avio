# ff-probe

Provides the ability to quickly learn what a piece of media contains (duration, resolution, codec, stream layout, and so on) without decoding it.

## Purpose

Before editing, converting, or playing back, you first need to know "what this material is." Without knowing its duration, resolution, codec, or audio/video layout, you can neither judge whether it can be ingested nor choose the appropriate processing. ff-probe makes it possible to **accurately grasp a material's nature in a single query**, without actually playing back or converting the file.

## What it solves

- **Immediate insight into material information** — obtain duration, resolution, codec, and the like in a short time without decoding the contents.
- **Judgment that does not rely on guesswork** — determine what a material contains based on its actual contents, not its extension or file name.
- **Insight into stream layout** — check the layout of the tracks contained in a material, such as video and audio.
- **Typed results** — receive information in a form you can work with directly, without interpreting strings.
- **Clear failures** — when a file cannot be opened or is corrupted, the result comes back in a form from which the cause can be read.

## Capabilities

- Obtaining a material's total playback duration
- Obtaining a video's resolution, frame rate, codec, and pixel format
- Obtaining an audio's sample rate, channel count, codec, and sample format
- Listing the video/audio streams contained in a material and identifying the representative stream
- Identifying the container (the format of the enclosing file)
- Distinguishing the reason a read failed (not found, invalid format, absent stream, and so on)

## Out of scope

- Decoding a material (extracting frames or samples)
- Writing out (encoding) or muxing
- Effects, compositing, or processing
- The editing model, such as timelines, edits, and history
- Rewriting metadata (read-only)
