# ff-stream

Provides the ability to write out media for web distribution in a form whose quality switches according to the viewer's connection conditions.

## Purpose

Delivering media over the internet calls not for a single file but for a distribution format whose quality can switch to match the viewer's connection speed and playback environment. Preparing such formats by hand is cumbersome and error-prone. ff-stream lets a **distribution package containing multiple qualities be generated in one pass from the input material and put on distribution as-is**. Callers need not be aware of the details of the distribution format; they only specify the arrangement of qualities they want to deliver.

## What it solves

- **Quality switching according to the connection** — output in a form where playback quality switches automatically to match the viewer's environment.
- **One-shot generation of a distribution package** — build, from the input material, a complete set that can be put on distribution as-is, all at once.
- **Simultaneous output of multiple qualities** — prepare multiple qualities, from high to low, with a single specification.
- **Support for major distribution methods** — output in the widely used, representative streaming methods.
- **Errors that identify the cause** — report configuration flaws and unsupported requests in a form that can be acted on.

## Capabilities

- Writing out in major adaptive streaming formats
- Specification of a tiered quality arrangement (combinations of resolution and bitrate)
- Generation of a multi-quality package from a single input
- Specification of basic distribution-related parameters such as segment length
- Generation of the playlists/manifests and split data required for distribution
- Clear failure notification for configuration flaws and unsupported requests

## Out of scope

- Distribution of the generated package, placement on a CDN, or playback to viewers
- The editing model such as timeline, clips, editing, and history
- Real-time preview or playback during editing
- Ordinary writing out to a single file (output not intended for distribution)
