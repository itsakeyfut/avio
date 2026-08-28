#[cfg(feature = "wgpu")]
mod graph_inner;

use crate::nodes::RenderNodeCpu;

#[cfg(feature = "wgpu")]
use crate::error::RenderError;

#[cfg(feature = "wgpu")]
use crate::context::RenderContext;
#[cfg(feature = "wgpu")]
use crate::nodes::RenderNode;
#[cfg(feature = "wgpu")]
use std::sync::Arc;

// RenderGraph

/// Linear chain of render nodes executed in insertion order.
///
/// The CPU fallback path ([`process_cpu`](Self::process_cpu)) is always
/// available and does not require the `wgpu` feature.  When the `wgpu` feature
/// is enabled, [`process_gpu`](Self::process_gpu) runs every node on the GPU.
///
/// # Construction
///
/// ```ignore
/// // GPU+CPU graph (wgpu feature):
/// let ctx = Arc::new(RenderContext::init().await?);
/// let graph = RenderGraph::new(Arc::clone(&ctx))
///     .push(ColorGradeNode { brightness: 0.1, ..Default::default() });
///
/// // CPU-only graph (no wgpu feature needed):
/// let graph = RenderGraph::new_cpu()
///     .push_cpu(ColorGradeNode { brightness: 0.1, ..Default::default() });
/// ```
pub struct RenderGraph {
    /// Nodes for the CPU fallback path only (added via `push_cpu`).
    cpu_nodes: Vec<Box<dyn RenderNodeCpu>>,
    #[cfg(feature = "wgpu")]
    gpu_nodes: Vec<Box<dyn RenderNode>>,
    /// `None` when constructed via `new_cpu` — `process_gpu` will return an error.
    #[cfg(feature = "wgpu")]
    ctx: Option<Arc<RenderContext>>,
}

impl RenderGraph {
    /// Create a GPU+CPU graph.
    ///
    /// Nodes added via [`push`](Self::push) run on the GPU and expose a CPU
    /// fallback via [`RenderNodeCpu`].  Nodes added via
    /// [`push_cpu`](Self::push_cpu) run on the CPU path only.
    #[cfg(feature = "wgpu")]
    #[must_use]
    pub fn new(ctx: Arc<RenderContext>) -> Self {
        Self {
            cpu_nodes: Vec::new(),
            gpu_nodes: Vec::new(),
            ctx: Some(ctx),
        }
    }

    /// Create a CPU-only graph (no GPU context required).
    ///
    /// [`process_gpu`](Self::process_gpu) returns [`RenderError::Composite`]
    /// when called on a CPU-only graph. Use [`process_cpu`](Self::process_cpu)
    /// instead.
    #[must_use]
    pub fn new_cpu() -> Self {
        Self {
            cpu_nodes: Vec::new(),
            #[cfg(feature = "wgpu")]
            gpu_nodes: Vec::new(),
            #[cfg(feature = "wgpu")]
            ctx: None,
        }
    }

    /// Append a GPU+CPU node to the chain.
    ///
    /// The node must implement both [`RenderNode`] (GPU, `wgpu` feature only)
    /// and [`RenderNodeCpu`] (CPU, always available) — the `RenderNode`
    /// supertrait bound guarantees this.
    #[cfg(feature = "wgpu")]
    #[must_use]
    pub fn push(mut self, node: impl RenderNode + 'static) -> Self {
        self.gpu_nodes.push(Box::new(node));
        self
    }

    /// Append a CPU-only node to the chain.
    ///
    /// CPU-only nodes participate in [`process_cpu`](Self::process_cpu) but
    /// not in [`process_gpu`](Self::process_gpu).
    ///
    /// When the `wgpu` feature is not enabled, this is the only `push` method.
    #[cfg(not(feature = "wgpu"))]
    #[must_use]
    pub fn push(mut self, node: impl RenderNodeCpu + 'static) -> Self {
        self.cpu_nodes.push(Box::new(node));
        self
    }

    /// Append a CPU-only node (available regardless of the `wgpu` feature).
    #[must_use]
    pub fn push_cpu(mut self, node: impl RenderNodeCpu + 'static) -> Self {
        self.cpu_nodes.push(Box::new(node));
        self
    }

    // Processing

    /// Run the GPU pipeline: upload `rgba` → execute all GPU nodes → download result.
    ///
    /// Requires the `wgpu` feature and a GPU context (created via [`new`](Self::new)).
    /// Returns [`RenderError::Composite`] if called on a CPU-only graph.
    ///
    /// # Errors
    ///
    /// Returns an error on GPU device failure or staging-buffer readback failure.
    #[cfg(feature = "wgpu")]
    pub fn process_gpu(&self, rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, RenderError> {
        let ctx = self.ctx.as_ref().ok_or_else(|| RenderError::Composite {
            message: "process_gpu called on a CPU-only RenderGraph (no RenderContext)".to_string(),
        })?;
        graph_inner::run_gpu(&self.gpu_nodes, ctx, rgba, w, h)
    }

    /// Run the CPU fallback pipeline: apply each node's `process_cpu` in order.
    ///
    /// Both CPU-only nodes (`push_cpu`) and GPU nodes (`push`, wgpu feature)
    /// participate — GPU nodes expose a CPU path via the `RenderNodeCpu`
    /// supertrait.
    #[must_use]
    pub fn process_cpu(&self, rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
        let mut out = rgba.to_vec();

        for node in &self.cpu_nodes {
            node.process_cpu(&mut out, w, h);
        }

        #[cfg(feature = "wgpu")]
        for node in &self.gpu_nodes {
            node.process_cpu(&mut out, w, h);
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::ColorGradeNode;

    #[test]
    fn render_graph_empty_cpu_should_return_input_unchanged() {
        let graph = RenderGraph::new_cpu();
        let rgba = vec![100u8, 150, 200, 255];
        let result = graph.process_cpu(&rgba, 1, 1);
        assert_eq!(result, rgba, "empty graph must return input unchanged");
    }

    #[test]
    fn render_graph_push_cpu_color_grade_should_brighten() {
        let graph = RenderGraph::new_cpu().push_cpu(ColorGradeNode::new(0.5, 1.0, 1.0, 0.0, 0.0));
        let rgba = vec![128u8, 128, 128, 255];
        let result = graph.process_cpu(&rgba, 1, 1);
        assert!(
            result[0] > 128,
            "brightness +0.5 must increase R; got {}",
            result[0]
        );
    }

    #[test]
    fn render_graph_multiple_cpu_nodes_should_chain() {
        // Two brightness boosts: +0.1 then +0.1 → total ≈ +0.2.
        let graph = RenderGraph::new_cpu()
            .push_cpu(ColorGradeNode::new(0.1, 1.0, 1.0, 0.0, 0.0))
            .push_cpu(ColorGradeNode::new(0.1, 1.0, 1.0, 0.0, 0.0));
        let single = RenderGraph::new_cpu().push_cpu(ColorGradeNode::new(0.2, 1.0, 1.0, 0.0, 0.0));

        let rgba = vec![100u8, 100, 100, 255];
        let chained = graph.process_cpu(&rgba, 1, 1);
        let single_result = single.process_cpu(&rgba, 1, 1);

        // Both should produce similar (but not necessarily identical) results.
        let diff = (chained[0] as i32 - single_result[0] as i32).abs();
        assert!(
            diff <= 2,
            "chained vs single brightness boost must be close; got chained={} single={}",
            chained[0],
            single_result[0]
        );
    }
}
