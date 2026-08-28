pub mod color_grade;
pub mod composite;
pub mod crossfade;
pub mod overlay;
pub mod scale;
pub mod upload;

pub use color_grade::ColorGradeNode;
pub use composite::{
    AlphaMatteNode, BlendMode, BlendModeNode, ChromaKeyNode, LumaMaskNode, ShapeMaskNode,
    TransformNode,
};
pub use crossfade::CrossfadeNode;
pub use overlay::OverlayNode;
pub use scale::{ScaleAlgorithm, ScaleNode};
pub use upload::{YuvFormat, YuvUploadNode};

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

    /// Run the GPU render pass.
    ///
    /// `inputs[i]` are the source textures (`len == input_count()`).
    /// `outputs[i]` are pre-allocated `Rgba8Unorm` target textures
    /// (`len == pass_count()`). Write the final result into `outputs[pass_count()-1]`.
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    );
}
