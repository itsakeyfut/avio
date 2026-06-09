//! `TransformNode` — UV-space translate / rotate / scale (GPU; CPU passthrough).

#[cfg(feature = "wgpu")]
use super::helpers::{
    fullscreen_pipeline, linear_sampler, one_tex_sampler_uniform_bgl, pack_f32, submit_render_pass,
};
use crate::nodes::RenderNodeCpu;

// ── TransformNode ─────────────────────────────────────────────────────────────

#[cfg(feature = "wgpu")]
struct TransformPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

/// Apply a 2D affine transform (translate, rotate, scale) to a texture.
///
/// Pixels that fall outside the [0, 1] UV range after the inverse transform
/// are rendered as fully transparent.
///
/// The CPU path is a no-op (passthrough); use the GPU path for actual
/// transformation.
pub struct TransformNode {
    /// UV-space translation (positive = shift right/down).
    pub translate: [f32; 2],
    /// Counter-clockwise rotation in radians.
    pub rotate: f32,
    /// Scale factors (1.0 = no change; > 1.0 = zoom in).
    pub scale: [f32; 2],
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<TransformPipeline>,
}

impl TransformNode {
    #[must_use]
    pub fn new(translate: [f32; 2], rotate: f32, scale: [f32; 2]) -> Self {
        Self {
            translate,
            rotate,
            scale,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

impl Default for TransformNode {
    fn default() -> Self {
        Self::new([0.0, 0.0], 0.0, [1.0, 1.0])
    }
}

impl RenderNodeCpu for TransformNode {
    fn process_cpu(&self, _rgba: &mut [u8], _w: u32, _h: u32) {
        // Affine transform is not implemented in the CPU fallback path.
    }
}

#[cfg(feature = "wgpu")]
impl TransformNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &TransformPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Transform shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../shaders/transform.wgsl").into(),
                ),
            });
            let bgl = one_tex_sampler_uniform_bgl(device, "Transform");
            let render_pipeline = fullscreen_pipeline(device, &shader, "Transform", &bgl);
            let sampler = linear_sampler(device, "Transform");
            // Uniform: translate[2], rotate, _pad, scale[2], _pad, _pad = 32 bytes.
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Transform uniforms"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            TransformPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                sampler,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl crate::nodes::RenderNode for TransformNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("TransformNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("TransformNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);

        // Pack uniforms: translate(2), rotate(1), pad(1), scale(2), pad(2) → 8×f32 = 32 bytes.
        let uniforms = pack_f32(&[
            self.translate[0],
            self.translate[1],
            self.rotate,
            0.0,
            self.scale[0],
            self.scale[1],
            0.0,
            0.0,
        ]);
        ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniforms);

        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Transform BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&in_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pd.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pd.uniform_buf.as_entire_binding(),
                },
            ],
        });
        submit_render_pass(
            ctx,
            &pd.render_pipeline,
            &bind_group,
            &out_view,
            "Transform",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::RenderNodeCpu;

    #[test]
    fn transform_node_cpu_path_should_be_passthrough() {
        let node = TransformNode::new([0.1, 0.0], 0.0, [2.0, 2.0]);
        let original = vec![10u8, 20, 30, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(rgba, original, "TransformNode CPU must be a no-op");
    }

    #[test]
    fn transform_node_default_should_be_identity() {
        let node = TransformNode::default();
        assert_eq!(node.translate, [0.0, 0.0]);
        assert_eq!(node.rotate, 0.0);
        assert_eq!(node.scale, [1.0, 1.0]);
    }
}
