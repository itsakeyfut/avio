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
    /// Working texture format for the GPU pipeline. `Rgba8Unorm` by default;
    /// [`with_pixel_format`](Self::with_pixel_format) promotes it to `Rgba16Float`
    /// for high-bit-depth input so precision is not lost before any node runs.
    #[cfg(feature = "wgpu")]
    internal_format: wgpu::TextureFormat,
}

/// Select the working GPU texture format for a source pixel format: `Rgba16Float`
/// for high-bit-depth (10/12-bit) input, `Rgba8Unorm` otherwise.
#[cfg(feature = "wgpu")]
#[must_use]
pub(crate) fn select_texture_format(pf: ff_format::PixelFormat) -> wgpu::TextureFormat {
    if pf.is_high_bit_depth() {
        wgpu::TextureFormat::Rgba16Float
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
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
            internal_format: wgpu::TextureFormat::Rgba8Unorm,
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
            #[cfg(feature = "wgpu")]
            internal_format: wgpu::TextureFormat::Rgba8Unorm,
        }
    }

    /// Set the source pixel format so the GPU pipeline runs at the matching
    /// working precision: high-bit-depth (10/12-bit) input promotes every
    /// internal texture to `Rgba16Float`; 8-bit input stays `Rgba8Unorm`.
    ///
    /// The chosen format flows through the texture-pool key and every
    /// intermediate target. For `Rgba16Float`, drive the graph with a
    /// high-bit-depth source node (e.g. [`YuvUploadNode::new_high_bit_depth`]);
    /// [`process_gpu`](Self::process_gpu) then returns raw `Rgba16Float` texels
    /// (8 bytes/pixel) rather than 8-bit RGBA.
    ///
    /// [`YuvUploadNode::new_high_bit_depth`]: crate::nodes::YuvUploadNode::new_high_bit_depth
    #[cfg(feature = "wgpu")]
    #[must_use]
    pub fn with_pixel_format(mut self, pf: ff_format::PixelFormat) -> Self {
        self.internal_format = select_texture_format(pf);
        self
    }

    /// The working GPU texture format ([`Rgba8Unorm`] by default,
    /// [`Rgba16Float`] after [`with_pixel_format`](Self::with_pixel_format) with a
    /// high-bit-depth format). Lets a caller interpret the byte layout of the
    /// buffer [`process_gpu`](Self::process_gpu) returns.
    ///
    /// [`Rgba8Unorm`]: wgpu::TextureFormat::Rgba8Unorm
    /// [`Rgba16Float`]: wgpu::TextureFormat::Rgba16Float
    #[cfg(feature = "wgpu")]
    #[must_use]
    pub fn internal_format(&self) -> wgpu::TextureFormat {
        self.internal_format
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
    /// `rgba` is the 8-bit source frame and the returned buffer is 8-bit RGBA by
    /// default. After [`with_pixel_format`](Self::with_pixel_format) selects an
    /// `Rgba16Float` working format, `rgba` is ignored (the graph is driven by a
    /// high-bit-depth source node) and the returned buffer is raw `Rgba16Float`
    /// texels (8 bytes/pixel); see [`internal_format`](Self::internal_format).
    ///
    /// # Errors
    ///
    /// Returns an error on GPU device failure or staging-buffer readback failure.
    #[cfg(feature = "wgpu")]
    pub fn process_gpu(&self, rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, RenderError> {
        let ctx = self.ctx.as_ref().ok_or_else(|| RenderError::Composite {
            message: "process_gpu called on a CPU-only RenderGraph (no RenderContext)".to_string(),
        })?;
        graph_inner::run_gpu(&self.gpu_nodes, ctx, rgba, w, h, self.internal_format)
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

#[cfg(all(test, feature = "wgpu"))]
mod gpu_tests {
    use super::{Arc, RenderContext, RenderGraph};
    use crate::nodes::{ColorGradeNode, RenderNode, RenderNodeCpu};

    /// A headless GPU context, or `None` when no adapter is available (CI).
    fn ctx() -> Option<Arc<RenderContext>> {
        match futures::executor::block_on(RenderContext::init()) {
            Ok(ctx) => Some(Arc::new(ctx)),
            Err(_) => None,
        }
    }

    /// Fill a whole texture with a solid RGBA color via `write_texture`.
    fn fill(ctx: &RenderContext, tex: &wgpu::Texture, color: [u8; 4]) {
        let (w, h) = (tex.width(), tex.height());
        let data: Vec<u8> = color
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect();
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    const COLOR_A: [u8; 4] = [10, 20, 30, 255];
    const COLOR_B: [u8; 4] = [200, 150, 100, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];

    /// Two-pass node: writes COLOR_A into the first pass and COLOR_B into the
    /// second. If the executor allocated only one output (ignoring `pass_count`)
    /// it writes COLOR_A instead, so a readback of COLOR_B proves both passes ran.
    struct TwoPassNode;
    impl RenderNodeCpu for TwoPassNode {
        fn process_cpu(&self, _rgba: &mut [u8], _w: u32, _h: u32) {}
    }
    impl RenderNode for TwoPassNode {
        fn pass_count(&self) -> usize {
            2
        }
        fn process(
            &self,
            _inputs: &[&wgpu::Texture],
            outputs: &[&wgpu::Texture],
            ctx: &RenderContext,
        ) {
            if outputs.len() >= 2 {
                fill(ctx, outputs[0], COLOR_A);
                fill(ctx, outputs[1], COLOR_B);
            } else {
                fill(ctx, outputs[0], COLOR_A);
            }
        }
    }

    /// Two-input node: writes GREEN when it receives both inputs, RED otherwise.
    /// A readback of GREEN proves the executor passed `input_count()` inputs.
    struct TwoInputNode;
    impl RenderNodeCpu for TwoInputNode {
        fn process_cpu(&self, _rgba: &mut [u8], _w: u32, _h: u32) {}
    }
    impl RenderNode for TwoInputNode {
        fn input_count(&self) -> usize {
            2
        }
        fn process(
            &self,
            inputs: &[&wgpu::Texture],
            outputs: &[&wgpu::Texture],
            ctx: &RenderContext,
        ) {
            let color = if inputs.len() == 2 { GREEN } else { RED };
            fill(ctx, outputs[0], color);
        }
    }

    #[test]
    fn executor_should_run_a_two_pass_node_and_read_back_the_final_pass() {
        let Some(ctx) = ctx() else {
            return;
        };
        let graph = RenderGraph::new(Arc::clone(&ctx)).push(TwoPassNode);
        let (w, h) = (16u32, 16u32);
        let rgba = vec![0u8; (w * h * 4) as usize];

        let out = graph.process_gpu(&rgba, w, h).expect("two-pass frame");
        assert_eq!(
            &out[0..4],
            &COLOR_B,
            "the final pass (COLOR_B) must be read back; got {:?}",
            &out[0..4]
        );
    }

    #[test]
    fn executor_should_feed_two_inputs_to_a_multi_input_node() {
        let Some(ctx) = ctx() else {
            return;
        };
        let graph = RenderGraph::new(Arc::clone(&ctx)).push(TwoInputNode);
        let (w, h) = (16u32, 16u32);
        let rgba = vec![0u8; (w * h * 4) as usize];

        let out = graph.process_gpu(&rgba, w, h).expect("two-input frame");
        assert_eq!(
            &out[0..4],
            &GREEN,
            "receiving two inputs must produce GREEN; got {:?}",
            &out[0..4]
        );
    }

    fn alloc_count(ctx: &RenderContext) -> usize {
        ctx.pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .alloc_count()
    }

    #[test]
    fn render_graph_should_not_allocate_textures_after_the_first_frame() {
        let Some(ctx) = ctx() else {
            return;
        };
        // Identity grade: a real GPU node so run_gpu acquires input + output.
        let graph =
            RenderGraph::new(Arc::clone(&ctx)).push(ColorGradeNode::new(0.0, 1.0, 1.0, 0.0, 0.0));
        let (w, h) = (16u32, 16u32);
        let rgba = vec![128u8; (w * h * 4) as usize];

        graph.process_gpu(&rgba, w, h).expect("first frame");
        let after_first = alloc_count(&ctx);
        assert!(
            after_first > 0,
            "the first frame must allocate its textures; got {after_first}"
        );

        for _ in 0..3 {
            graph.process_gpu(&rgba, w, h).expect("subsequent frame");
        }
        assert_eq!(
            alloc_count(&ctx),
            after_first,
            "same-size frames must reuse pooled textures (steady state = 0 allocations/frame)"
        );
    }

    /// Decode an IEEE-754 half-float (as read back from an `Rgba16Float` target)
    /// to `f32`. Adequate for the [0, 1] RGB values these tests read.
    #[allow(clippy::cast_precision_loss)]
    fn f16_to_f32(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exp = i32::from((bits >> 10) & 0x1f);
        let frac = f32::from(bits & 0x3ff);
        if exp == 0 {
            sign * frac * 2f32.powi(-24)
        } else if exp == 0x1f {
            sign * f32::INFINITY
        } else {
            sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15)
        }
    }

    /// A plane of `count` little-endian `u16` samples all equal to `value`.
    fn plane10(value: u16, count: usize) -> Vec<u8> {
        value
            .to_le_bytes()
            .iter()
            .copied()
            .cycle()
            .take(count * 2)
            .collect()
    }

    #[test]
    fn pipeline_should_select_rgba16float_for_10bit_input() {
        use super::select_texture_format;
        use ff_format::PixelFormat;

        assert_eq!(
            select_texture_format(PixelFormat::Yuv420p10le),
            wgpu::TextureFormat::Rgba16Float,
            "10-bit planar input must select Rgba16Float"
        );
        assert_eq!(
            select_texture_format(PixelFormat::P010le),
            wgpu::TextureFormat::Rgba16Float,
            "10-bit semi-planar input must select Rgba16Float"
        );
        assert_eq!(
            select_texture_format(PixelFormat::Yuv420p),
            wgpu::TextureFormat::Rgba8Unorm,
            "8-bit input must stay Rgba8Unorm"
        );
        assert_eq!(
            select_texture_format(PixelFormat::Rgba),
            wgpu::TextureFormat::Rgba8Unorm,
            "8-bit RGBA input must stay Rgba8Unorm"
        );
        // The builder reflects the same choice (no adapter required).
        assert_eq!(
            RenderGraph::new_cpu()
                .with_pixel_format(PixelFormat::Yuv420p10le)
                .internal_format(),
            wgpu::TextureFormat::Rgba16Float
        );
        assert_eq!(
            RenderGraph::new_cpu()
                .with_pixel_format(PixelFormat::Yuv420p)
                .internal_format(),
            wgpu::TextureFormat::Rgba8Unorm
        );
    }

    #[test]
    fn yuv_upload_should_preserve_10bit_precision_into_rgba16float() {
        use ff_format::PixelFormat;

        use crate::nodes::{YuvFormat, YuvUploadNode};

        let Some(ctx) = ctx() else {
            return;
        };
        let (w, h) = (2u32, 2u32);

        // Render one 10-bit luma value (neutral chroma) at Rgba16Float and return
        // the read-back R channel as f32.
        let render = |y10: u16| -> f32 {
            let mut node = YuvUploadNode::new_high_bit_depth(YuvFormat::Yuv420p, w, h);
            // 2×2 420p: 4 luma samples, 1 chroma sample; neutral chroma = 512.
            node.set_planes(plane10(y10, 4), plane10(512, 1), plane10(512, 1));
            let graph = RenderGraph::new(Arc::clone(&ctx))
                .with_pixel_format(PixelFormat::Yuv420p10le)
                .push(node);
            // The 8-bit `rgba` arg is ignored for an Rgba16Float graph.
            let out = graph.process_gpu(&[], w, h).expect("hdr frame");
            assert_eq!(
                out.len(),
                (w * h * 8) as usize,
                "Rgba16Float readback must be 8 bytes/pixel"
            );
            f16_to_f32(u16::from_le_bytes([out[0], out[1]]))
        };

        // Y = 512 and Y = 515 both round to 128 (0.502) in 8-bit, but differ by
        // 3/1023 ≈ 0.0029 in 10-bit. Preserving that proves the pipeline ran at
        // >8-bit precision (non-vacuous: an 8-bit path would make them identical).
        let a = render(512);
        let b = render(515);
        assert!(
            (a - 512.0 / 1023.0).abs() < 0.01,
            "Y=512 must decode to ~0.5005; got {a}"
        );
        assert!(
            (b - 515.0 / 1023.0).abs() < 0.01,
            "Y=515 must decode to ~0.5034; got {b}"
        );
        assert!(
            (b - a).abs() > 0.0015,
            "10-bit precision must distinguish Y=512 from Y=515; got a={a} b={b}"
        );
    }
}
