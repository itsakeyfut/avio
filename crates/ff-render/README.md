# ff-render

GPU compositing and effects pipeline for video, built on [wgpu]. Applies per-frame visual effects (colour grading, blending, masking, chroma key, YUV upload) in a linear render graph that can be attached to `ff-preview`'s `PlayerRunner` as an opt-in `GpuFrameSink`.

`ff-render` is a GPU compositing and effects pipeline built on [wgpu](https://github.com/gfx-rs/wgpu), not FFmpeg: WGSL shaders run colour grading, blends, chroma-key, masks, transforms, scaling, and a crossfade transition on GPU textures, with a CPU software fallback. It consumes decoded frames and plugs into `ff-preview` through the `FrameSink` trait. Errors are typed and contextual (`RenderError`), so a shader-compile or device failure reads as an actionable message.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Status

Implemented today: the CPU and GPU render graph (`RenderGraph`), all built-in nodes listed below, the 18 `BlendMode` variants (CPU and GPU), the native YUV upload path, the multi-layer `Compositor` (wgpu feature), and `GpuFrameSink` for `ff-preview` integration.

`GpuFrameSink` is **opt-in**: you attach it explicitly with `runner.set_sink(...)`. It is not the default preview compositor. avio's standard preview path composites on the CPU today; making GPU compositing the default preview and export compositor across avio is a separate, deferred effort tracked in [#1365](https://github.com/itsakeyfut/avio/issues/1365).

## Installation

```toml
[dependencies]
ff-render = "0.17"

# Enable GPU processing (requires wgpu-compatible hardware)
ff-render = { version = "0.17", features = ["wgpu"] }
```

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `wgpu` | GPU processing via wgpu (Metal / Vulkan / DX12 / WebGPU) | no |

Without `wgpu` only the CPU fallback path is available via `RenderGraph::process_cpu`. The CPU path is suitable for unit tests, CI, and software-only environments.

## CPU Path (no wgpu required)

All built-in nodes implement `RenderNodeCpu`, which processes raw RGBA bytes without any GPU dependency.

```rust
use ff_render::{BlendMode, BlendModeNode, ColorGradeNode, RenderGraph};

fn main() {
    // Build a pipeline: boost brightness then multiply-blend with an overlay.
    let overlay_rgba: Vec<u8> = vec![0u8; 4 * 4 * 4];
    let graph = RenderGraph::new_cpu()
        .push_cpu(ColorGradeNode::new(0.2, 1.0, 1.0, 0.0, 0.0))
        .push_cpu(BlendModeNode::new(BlendMode::Multiply, 0.8, overlay_rgba, 4, 4));

    // A decoded 4×4 RGBA frame (mid-grey stand-in).
    let input_rgba: Vec<u8> = vec![128u8; 4 * 4 * 4];
    let output: Vec<u8> = graph.process_cpu(&input_rgba, 4, 4);

    println!("processed {} bytes", output.len());
}
```

## GPU Path (wgpu feature)

When the `wgpu` feature is enabled, nodes run on the GPU via `RenderGraph::process_gpu`. The same nodes implement both `RenderNode` (GPU) and `RenderNodeCpu` (CPU fallback).

```rust
#[cfg(feature = "wgpu")]
use std::sync::Arc;
#[cfg(feature = "wgpu")]
use ff_render::{RenderContext, RenderGraph, ColorGradeNode};

#[cfg(feature = "wgpu")]
async fn example() -> Result<(), ff_render::RenderError> {
    let ctx = Arc::new(RenderContext::init().await?);
    let graph = RenderGraph::new(Arc::clone(&ctx))
        .push(ColorGradeNode::new(0.1, 1.2, 1.0, 0.0, 0.0));

    let input_rgba = vec![128u8; 1920 * 1080 * 4];
    let output = graph.process_gpu(&input_rgba, 1920, 1080)?;
    Ok(())
}
```

## Integration with ff-preview

`GpuFrameSink` implements `ff_preview::FrameSink`, wiring the render graph directly into a `PlayerRunner` pipeline.

```rust
#[cfg(feature = "wgpu")]
use std::sync::Arc;
#[cfg(feature = "wgpu")]
use ff_preview::{PreviewPlayer, RgbaSink};
#[cfg(feature = "wgpu")]
use ff_render::{RenderContext, RenderGraph, ColorGradeNode, GpuFrameSink};

#[cfg(feature = "wgpu")]
async fn with_preview() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = Arc::new(RenderContext::init().await?);

    let graph = RenderGraph::new(Arc::clone(&ctx))
        .push(ColorGradeNode::new(0.1, 1.0, 1.0, 0.0, 0.0));

    let downstream = RgbaSink::new();
    let handle = downstream.frame_handle();

    let (mut runner, _player_handle) = PreviewPlayer::open("clip.mp4")?.split();
    runner.set_sink(Box::new(GpuFrameSink::new(graph, Box::new(downstream))));

    std::thread::spawn(move || runner.run());

    // Retrieve the latest processed frame from any thread.
    if let Some(frame) = handle.lock().unwrap().as_ref() {
        println!("frame: {}×{} pts={:?}", frame.width, frame.height, frame.pts);
    }
    Ok(())
}
```

## Multi-Layer Compositor (wgpu feature)

`Compositor` accepts a `Vec<FrameLayer>`, sorts layers by `z_order`, applies per-layer transforms and blend modes, and returns the composited `wgpu::Texture`.

```rust
#[cfg(feature = "wgpu")]
use std::sync::Arc;
#[cfg(feature = "wgpu")]
use ff_render::{
    RenderContext, Compositor, FrameLayer, LayerTransform, BlendMode,
};

#[cfg(feature = "wgpu")]
async fn compositor_example() -> Result<(), ff_render::RenderError> {
    let ctx = Arc::new(RenderContext::init().await?);
    let mut comp = Compositor::new(Arc::clone(&ctx), 1920, 1080);

    // Layer frames are decoded elsewhere (e.g. via ff-decode) as `ff_format::VideoFrame`.
    let background_frame = /* a VideoFrame */ unimplemented!();
    let overlay_frame = /* a VideoFrame */ unimplemented!();

    let mut layers = vec![
        FrameLayer {
            frame:      background_frame,
            transform:  LayerTransform::default(),   // identity
            blend_mode: BlendMode::Normal,
            opacity:    1.0,
            z_order:    0,
        },
        FrameLayer {
            frame:      overlay_frame,
            transform:  LayerTransform { x: 0.1, scale_x: 0.5, scale_y: 0.5, ..Default::default() },
            blend_mode: BlendMode::Screen,
            opacity:    0.8,
            z_order:    1,
        },
    ];

    let texture: wgpu::Texture = comp.composite(&mut layers)?;
    Ok(())
}
```

## Built-in Nodes

| Node | CPU | GPU | Description |
|------|-----|-----|-------------|
| `ColorGradeNode` | ✓ | ✓ | Brightness, contrast, saturation, temperature, tint |
| `ScaleNode` | passthrough | ✓ | Resize to target dimensions (Bilinear / Nearest) |
| `OverlayNode` | ✓ | ✓ | Alpha-composite a static overlay image over the base |
| `CrossfadeNode` | ✓ | ✓ | Linear crossfade between base and a target image |
| `BlendModeNode` | ✓ | ✓ | Photoshop-style blend modes with per-node opacity |
| `TransformNode` | passthrough | ✓ | Translate, rotate, and scale the frame in UV space |
| `ChromaKeyNode` | ✓ | ✓ | Chroma key (green screen): removes a specified colour range |
| `ShapeMaskNode` | ✓ | ✓ | Binary alpha mask from an RGBA mask image |
| `LumaMaskNode` | ✓ | ✓ | Luma-derived alpha mask (bright = keep, dark = cut) |
| `AlphaMatteNode` | ✓ | ✓ | Alpha-composite foreground over a background using fg alpha |
| `YuvUploadNode` | ✓ | ✓ | Upload native YUV planes (4:2:0 / 4:2:2 / 4:4:4) without `sws_scale` |

### Blend Modes

`BlendModeNode` supports the following modes via `BlendMode`:

`Normal` · `Multiply` · `Screen` · `Overlay` · `SoftLight` · `HardLight` · `ColorDodge` · `ColorBurn` · `Difference` · `Exclusion` · `Add` · `Subtract` · `Darken` · `Lighten` · `Hue` · `Saturation` · `Color` · `Luminosity`

## YUV Upload

`YuvUploadNode` accepts planar YUV data directly, bypassing `sws_scale`:

```rust
use ff_render::{RenderGraph, YuvFormat, YuvUploadNode};

fn main() {
    // Y at full resolution; Cb/Cr sub-sampled to half width and height (4:2:0).
    let y_plane = vec![0u8; 1920 * 1080];
    let cb_plane = vec![128u8; 960 * 540];
    let cr_plane = vec![128u8; 960 * 540];

    let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 1920, 1080);
    node.set_planes(y_plane, cb_plane, cr_plane);

    let graph = RenderGraph::new_cpu().push_cpu(node);
    let rgba = graph.process_cpu(&vec![0u8; 1920 * 1080 * 4], 1920, 1080);
    println!("produced {} rgba bytes", rgba.len());
}
```

Supported formats: `Yuv420p`, `Yuv422p`, `Yuv444p`.

## Error Handling

All fallible operations return `RenderError`:

```rust
use ff_render::RenderError;

match result {
    Err(RenderError::DeviceCreation { message })     => { /* GPU device init failed */ }
    Err(RenderError::UnsupportedFormat { format })   => { /* pixel format not supported */ }
    Err(RenderError::Composite { message })          => { /* compositor error */ }
    Err(other) => { /* shader compile, texture, GPU timeout, I/O, ... */ }
    Ok(output) => { /* process output */ }
}
```

## Crate stack

```
ff-sys → ff-common → ff-format → ff-preview → ff-render
```

`ff-render` depends on `ff-preview` for the `FrameSink` trait and `VideoFrame` type. It has no direct dependency on `ff-decode` or `ff-filter`; frames can come from any source.
