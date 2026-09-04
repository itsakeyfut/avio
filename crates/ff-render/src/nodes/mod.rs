pub mod blur;
pub mod color_grade;
pub mod color_wheels;
pub mod composite;
pub mod crossfade;
pub mod curves;
pub mod film_grain;
pub mod glow;
pub mod hsl;
pub mod lut;
pub mod overlay;
pub mod scale;
pub mod transition;
pub mod upload;
pub mod vignette;

pub use blur::{GaussianBlurNode, MotionBlurNode, SharpenNode};
pub use color_grade::ColorGradeNode;
pub use color_wheels::ColorWheelsNode;
pub use composite::{
    AlphaMatteNode, BlendMode, BlendModeNode, ChromaKeyNode, CompositeOp, LumaMaskNode,
    ShapeMaskNode, TransformNode,
};
pub use crossfade::CrossfadeNode;
pub use curves::CurvesNode;
pub use film_grain::FilmGrainNode;
pub use glow::GlowNode;
pub use hsl::HslNode;
pub use lut::LutNode;
pub use overlay::OverlayNode;
pub use scale::{ScaleAlgorithm, ScaleNode};
pub use transition::{
    DipToColorNode, DissolveTransitionNode, FadeTransitionNode, WipeTransitionNode,
};
pub use upload::{YuvFormat, YuvUploadNode};
pub use vignette::VignetteNode;

// RenderNodeCpu

/// CPU fallback processing for a render node.
///
/// Implemented by all built-in nodes. Nodes that do not change frame
/// dimensions modify `rgba` in-place. Multi-input nodes (e.g. [`CrossfadeNode`])
/// store their secondary inputs as fields and access them during `process_cpu`.
pub trait RenderNodeCpu: Send {
    /// Process `rgba` in-place.
    ///
    /// `rgba` is a row-major RGBA buffer of size `w × h × 4` bytes.
    /// Nodes that cannot implement a CPU path leave `rgba` unchanged.
    fn process_cpu(&self, rgba: &mut [u8], w: u32, h: u32);
}

// RenderNode

/// GPU render node. Extends [`RenderNodeCpu`] so both paths are available.
///
/// Each node is responsible for creating and caching its own wgpu pipeline
/// on first use. The pipeline is stored in a [`std::sync::OnceLock`] field
/// so it is created exactly once per node instance.
///
/// `process` may submit one or more `wgpu::CommandEncoder` buffers. The
/// [`RenderGraph`](crate::graph::RenderGraph) guarantees that the queue
/// processes them in submission order.
#[cfg(feature = "wgpu")]
pub trait RenderNode: RenderNodeCpu {
    /// Number of input textures required by this node (default: 1).
    fn input_count(&self) -> usize {
        1
    }

    /// Number of render passes (default: 1). Multi-pass nodes (e.g. gaussian
    /// blur) return 2 or more.
    fn pass_count(&self) -> usize {
        1
    }

    /// Output dimensions this node produces given its input dimensions.
    ///
    /// Default: unchanged (`(in_w, in_h)`). A resampling node (e.g.
    /// [`ScaleNode`]) overrides this so the executor allocates its output target
    /// — and every following node's input — at the new size. All of a node's
    /// `pass_count()` targets are allocated at this size.
    fn output_dimensions(&self, in_w: u32, in_h: u32) -> (u32, u32) {
        (in_w, in_h)
    }

    /// Run the GPU render pass.
    ///
    /// `inputs` has `input_count()` textures: `inputs[0]` is the previous node's
    /// final-pass output (or the source frame for the first node), and
    /// `inputs[1..]` are the original source frame. `outputs` has `pass_count()`
    /// pre-allocated `Rgba8Unorm` targets; write the final result into
    /// `outputs[pass_count()-1]`, which the executor feeds to the next node.
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    );
}
