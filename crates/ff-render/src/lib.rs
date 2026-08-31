//! # ff-render
//!
//! GPU compositing and effects pipeline for video, built on [wgpu].
//!
//! `ff-render` sits above `ff-preview` in the crate stack and implements
//! [`ff_preview::FrameSink`], so a [`RenderGraph`] can be attached to a
//! [`ff_preview::PlayerRunner`] as an opt-in [`GpuFrameSink`]. It is not the
//! default preview compositor: avio composites preview on the CPU today, and
//! making GPU compositing the default across avio is tracked separately in
//! issue #1365.
//!
//! ## Feature flags
//!
//! | Feature | Description | Default |
//! |---------|-------------|---------|
//! | `wgpu`  | GPU processing via wgpu (Metal / Vulkan / DX12 / WebGPU) | no |
//!
//! Without `wgpu` only the CPU fallback path is available via
//! [`RenderGraph::process_cpu`].
//!
//! ## Usage — wiring to `PlayerRunner`
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! use ff_preview::{PreviewPlayer, RgbaSink};
//! use ff_render::context::RenderContext;
//! use ff_render::graph::RenderGraph;
//! use ff_render::nodes::ColorGradeNode;
//! use ff_render::sink::GpuFrameSink;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. Initialise the GPU device (headless — no window required).
//! let ctx = Arc::new(RenderContext::init().await?);
//!
//! // 2. Build a render graph: apply a gentle brightness boost.
//! let graph = RenderGraph::new(Arc::clone(&ctx)).push(ColorGradeNode {
//!     brightness: 0.1,
//!     ..Default::default()
//! });
//!
//! // 3. Open the player, attach the GPU sink, and run on a dedicated thread.
//! let downstream = RgbaSink::new();
//! let handle = downstream.frame_handle();
//! let (mut runner, _player_handle) = PreviewPlayer::open("clip.mp4")?.split();
//! runner.set_sink(Box::new(GpuFrameSink::new(graph, Box::new(downstream))));
//!
//! std::thread::spawn(move || runner.run());
//!
//! // 4. Retrieve the latest processed frame from any thread.
//! if let Some(frame) = handle.lock().unwrap().as_ref() {
//!     println!("frame: {}×{} pts={:?}", frame.width, frame.height, frame.pts);
//! }
//! # Ok(())
//! # }
//! ```

#![warn(clippy::all)]
#![warn(clippy::pedantic)]

pub mod error;
pub mod graph;
pub mod nodes;
pub mod sink;

#[cfg(feature = "wgpu")]
pub mod compositor;
#[cfg(feature = "wgpu")]
pub mod context;
#[cfg(feature = "wgpu")]
mod pool;

// Top-level re-exports

#[cfg(feature = "wgpu")]
pub use compositor::{Compositor, FrameLayer, LayerTransform};
pub use error::RenderError;
pub use ff_format::{ErrorSeverity, MediaError};
pub use graph::RenderGraph;
pub use nodes::{
    AlphaMatteNode, BlendMode, BlendModeNode, ChromaKeyNode, ColorGradeNode, CrossfadeNode,
    FilmGrainNode, GaussianBlurNode, GlowNode, LumaMaskNode, OverlayNode, RenderNodeCpu,
    ScaleAlgorithm, ScaleNode, ShapeMaskNode, SharpenNode, TransformNode, VignetteNode, YuvFormat,
    YuvUploadNode,
};
pub use sink::GpuFrameSink;

#[cfg(feature = "wgpu")]
pub use context::RenderContext;
#[cfg(feature = "wgpu")]
pub use nodes::RenderNode;
#[cfg(feature = "wgpu")]
pub use sink::TextureHandle;
