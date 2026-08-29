# ff-render

Composites multiple video layers, effects, and transitions into a single image used for preview and export.

## Purpose

An edited video only becomes a finished image once multiple sources are layered, colors are adjusted, and transition effects are added. Such compositing involves a large amount of computation per frame, and doing it naively tends to be slow. ff-render performs this compositing and processing quickly with the power of the GPU, taking on the role of **assembling the edit result into a video you can actually see**. So that the same result can be obtained even in environments where the GPU is unavailable, it also provides an alternative processing path.

## What it solves

- **Layering** — composite multiple videos such as background, foreground, and overlay into one according to stacking order, opacity, and position.
- **Look adjustment** — apply color correction such as brightness, contrast, saturation, and color temperature to video.
- **Compositing expression** — blend sources together with a variety of blend modes such as multiply, screen, and overlay.
- **Cutout and masking** — keep only the needed parts with chroma key (green-screen removal) and various masks.
- **Transition effects** — smoothly show the change between clips, such as crossfades.
- **Runs in any environment** — fast where a GPU is available, and producing the same result even where one is not.

## Capabilities

- Compositing of multiple layers (reflecting stacking order, opacity, scaling, translation, and rotation)
- Color correction (brightness, contrast, saturation, color temperature, tint)
- Blending with a rich set of blend modes
- Cutout via chroma key, luma mask, shape mask, and the like
- Transitions between clips (crossfade)
- Integration into preview playback (continuous handoff of processed frames)
- Fast processing on GPU environments, with an alternative path on unsupported environments

## Out of scope

- Reading in (decoding) or writing out (encoding) sources themselves
- The timeline or editing decisions of which source is placed when and how
- Edit history management (Undo/Redo)
- Audio processing
