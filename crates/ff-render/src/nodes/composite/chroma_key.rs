//! `ChromaKeyNode` — green-screen keying (CPU + GPU) with tolerance/softness.

#[cfg(feature = "wgpu")]
use super::helpers::{
    fullscreen_pipeline, linear_sampler, one_tex_sampler_uniform_bgl, pack_f32, submit_render_pass,
};
use crate::nodes::RenderNodeCpu;

// ── ChromaKeyNode ─────────────────────────────────────────────────────────────

#[cfg(feature = "wgpu")]
struct ChromaKeyPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

/// Remove a solid colour from a texture by chroma distance, producing alpha.
///
/// The algorithm computes the Euclidean distance between the pixel's chroma
/// vector (RGB − luma) and the key colour's chroma vector, then applies a soft
/// threshold to set the alpha channel.  Pixels that match `key_color` within
/// `tolerance` become fully transparent; pixels further than `tolerance +
/// softness` stay fully opaque.
pub struct ChromaKeyNode {
    /// Key colour in linear RGB [0.0, 1.0].
    pub key_color: [f32; 3],
    /// Chroma distance threshold (0.0–1.0).
    pub tolerance: f32,
    /// Edge feather width (0.0–1.0).
    pub softness: f32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<ChromaKeyPipeline>,
}

impl ChromaKeyNode {
    #[must_use]
    pub fn new(key_color: [f32; 3], tolerance: f32, softness: f32) -> Self {
        Self {
            key_color,
            tolerance,
            softness,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }
}

// ── CPU helpers ───────────────────────────────────────────────────────────────

pub(super) fn bt709_luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn chroma_dist_cpu(pixel: [f32; 3], key: [f32; 3]) -> f32 {
    let pl = bt709_luma(pixel[0], pixel[1], pixel[2]);
    let kl = bt709_luma(key[0], key[1], key[2]);
    let dp = [pixel[0] - pl, pixel[1] - pl, pixel[2] - pl];
    let dk = [key[0] - kl, key[1] - kl, key[2] - kl];
    let d = [dp[0] - dk[0], dp[1] - dk[1], dp[2] - dk[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl RenderNodeCpu for ChromaKeyNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let r = f32::from(pixel[0]) / 255.0;
            let g = f32::from(pixel[1]) / 255.0;
            let b = f32::from(pixel[2]) / 255.0;
            let a = f32::from(pixel[3]) / 255.0;
            let dist = chroma_dist_cpu([r, g, b], self.key_color);
            let alpha_factor = smoothstep(
                self.tolerance - self.softness,
                self.tolerance + self.softness,
                dist,
            );
            pixel[3] = ((a * alpha_factor).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
}

#[cfg(feature = "wgpu")]
impl ChromaKeyNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &ChromaKeyPipeline {
        self.pipeline.get_or_init(|| {
            let device = &ctx.device;
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ChromaKey shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../shaders/chroma_key.wgsl").into(),
                ),
            });
            let bgl = one_tex_sampler_uniform_bgl(device, "ChromaKey");
            let render_pipeline = fullscreen_pipeline(device, &shader, "ChromaKey", &bgl);
            let sampler = linear_sampler(device, "ChromaKey");
            // Uniform: key_color(3) + tolerance(1) + softness(1) + pad(3) = 8×f32 = 32 bytes.
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ChromaKey uniforms"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ChromaKeyPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                sampler,
                uniform_buf,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl crate::nodes::RenderNode for ChromaKeyNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("ChromaKeyNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("ChromaKeyNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);

        let uniforms = pack_f32(&[
            self.key_color[0],
            self.key_color[1],
            self.key_color[2],
            self.tolerance,
            self.softness,
            0.0,
            0.0,
            0.0,
        ]);
        ctx.queue.write_buffer(&pd.uniform_buf, 0, &uniforms);

        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ChromaKey BG"),
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
            "ChromaKey",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::RenderNodeCpu;

    #[test]
    fn chroma_key_node_pure_green_should_become_transparent() {
        let mut rgba = vec![0u8, 255, 0, 255]; // pure green
        let node = ChromaKeyNode::new([0.0, 1.0, 0.0], 0.1, 0.05);
        node.process_cpu(&mut rgba, 1, 1);
        assert_eq!(
            rgba[3], 0,
            "pure green key must produce fully transparent alpha"
        );
    }

    #[test]
    fn chroma_key_node_non_key_colour_should_stay_opaque() {
        let mut rgba = vec![255u8, 0, 0, 255]; // pure red
        let node = ChromaKeyNode::new([0.0, 1.0, 0.0], 0.1, 0.05);
        node.process_cpu(&mut rgba, 1, 1);
        assert!(
            rgba[3] > 200,
            "non-key colour must stay opaque; got alpha={}",
            rgba[3]
        );
    }

    #[test]
    fn chroma_key_node_tolerances_should_control_threshold() {
        // A dark green should be keyed with a generous tolerance but not with a tight one.
        let mut rgba_tight = vec![0u8, 100, 0, 255]; // dark green
        let mut rgba_loose = rgba_tight.clone();
        let node_tight = ChromaKeyNode::new([0.0, 1.0, 0.0], 0.05, 0.01);
        let node_loose = ChromaKeyNode::new([0.0, 1.0, 0.0], 0.8, 0.1);
        node_tight.process_cpu(&mut rgba_tight, 1, 1);
        node_loose.process_cpu(&mut rgba_loose, 1, 1);
        assert!(
            rgba_loose[3] < rgba_tight[3],
            "loose tolerance must key more aggressively than tight"
        );
    }
}
