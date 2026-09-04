# ff-remux

Cuts out clips and replaces, extracts, or adds audio without re-encoding, quickly and without quality loss.

## Purpose

Operations such as cutting out part of a source or replacing its audio can be achieved without rebuilding the content of the video and audio. Even so, performing a rebuild (re-encoding) takes time and also degrades picture and sound quality. ff-remux **copies the content as-is to perform** such container-level operations, so they can be finished **quickly and without quality loss**.

## What it solves

- **Lossless cutout** — copy the specified time range without rebuilding it, extracting it while preserving picture and sound quality.
- **Replacing, extracting, and adding audio** — with the video left as-is, swap out, take out, or add the audio track.
- **Fast processing** — because no re-encoding is involved, it completes in a short time even for long content.
- **Preserving quality** — carry over the original video and audio as-is, producing no generational degradation.
- **Understandable failures** — return unsupported combinations and invalid inputs in a form whose cause can be read.

## Capabilities

- Cutout of a time range (no re-encoding)
- Replacing an audio track
- Extracting an audio track
- Adding an audio track
- Performing all of the above without any rebuild of video or audio
- Applying a chosen bitstream filter to the copied stream, for the rewrites `FFmpeg` does not perform on its own

## Out of scope

- Re-encoding or format conversion (export that involves a rebuild)
- Processing such as effects and compositing
- Analysis or metadata extraction of sources
- The editing model such as timeline, editing, and history
