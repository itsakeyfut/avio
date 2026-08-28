use std::sync::Arc;

use ff_format::{PixelFormat, VideoFrame};

use crate::context::RenderContext;
use crate::error::RenderError;
use crate::nodes::composite::{
    fullscreen_pipeline, linear_sampler, submit_render_pass, two_tex_sampler_uniform_bgl,
    upload_rgba_texture,
};
use crate::nodes::{BlendMode, RenderNode, TransformNode};

use super::FrameLayer;

// CompositorGraph

/// Internal GPU state for the compositor.
///
/// Holds the blend shader pipeline and a reusable `TransformNode`. Built once
/// per unique layer count and reused across frames.
pub(super) struct CompositorGraph {
    blend_pipeline: wgpu::RenderPipeline,
    blend_bgl: wgpu::BindGroupLayout,
    blend_sampler: wgpu::Sampler,
    blend_uniform_buf: wgpu::Buffer,
    transform_node: TransformNode,
}

impl CompositorGraph {
    pub(super) fn build(
        ctx: &Arc<RenderContext>,
        _layer_count: usize,
        _width: u32,
        _height: u32,
    ) -> Self {
        let device = &ctx.device;

        let blend_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compositor blend shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/blend.wgsl").into()),
        });
        let blend_bgl = two_tex_sampler_uniform_bgl(device, "Compositor blend");
        let blend_pipeline =
            fullscreen_pipeline(device, &blend_shader, "Compositor blend", &blend_bgl);
        let blend_sampler = linear_sampler(device, "Compositor blend");
        let blend_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Compositor blend uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            blend_pipeline,
            blend_bgl,
            blend_sampler,
            blend_uniform_buf,
            transform_node: TransformNode::default(),
        }
    }

    pub(super) fn composite(
        &mut self,
        ctx: &Arc<RenderContext>,
        layers: &[FrameLayer],
        w: u32,
        h: u32,
    ) -> Result<wgpu::Texture, RenderError> {
        let mut canvas = create_canvas(ctx, w, h);

        for layer in layers {
            let (fw, fh) = layer.frame.resolution();
            let rgba = frame_to_rgba(&layer.frame)?;
            let src_tex = upload_rgba_texture(ctx, &rgba, fw, fh, "Compositor src");

            let layer_tex = if layer.transform.is_identity() {
                src_tex
            } else {
                let xfm_tex = create_output_tex(ctx, w, h);
                self.transform_node.translate = [layer.transform.x, layer.transform.y];
                self.transform_node.rotate = layer.transform.rotation;
                self.transform_node.scale = [layer.transform.scale_x, layer.transform.scale_y];
                self.transform_node.process(&[&src_tex], &[&xfm_tex], ctx);
                xfm_tex
            };

            let new_canvas = create_output_tex(ctx, w, h);
            blend_textures(
                ctx,
                &self.blend_pipeline,
                &self.blend_bgl,
                &self.blend_sampler,
                &self.blend_uniform_buf,
                &canvas,
                &layer_tex,
                &new_canvas,
                layer.blend_mode,
                layer.opacity,
            );
            canvas = new_canvas;
        }

        Ok(canvas)
    }
}

// Frame → RGBA conversion

/// Convert any supported `VideoFrame` to a dense RGBA byte buffer.
///
/// Uses BT.601 coefficients for YUV formats (matching `YuvUploadNode`).
/// Returns `RenderError::UnsupportedFormat` for unrecognised formats.
fn frame_to_rgba(frame: &VideoFrame) -> Result<Vec<u8>, RenderError> {
    let w = frame.width() as usize;
    let h = frame.height() as usize;

    match frame.format() {
        PixelFormat::Rgba => {
            let plane = frame.plane(0).ok_or_else(|| RenderError::Composite {
                message: "Rgba frame: missing plane 0".to_string(),
            })?;
            let stride = frame.stride(0).unwrap_or(w * 4);
            let row = w * 4;
            if stride == row {
                Ok(plane[..row * h].to_vec())
            } else {
                let mut out = Vec::with_capacity(row * h);
                for r in 0..h {
                    out.extend_from_slice(&plane[r * stride..r * stride + row]);
                }
                Ok(out)
            }
        }
        PixelFormat::Bgra => {
            let plane = frame.plane(0).ok_or_else(|| RenderError::Composite {
                message: "Bgra frame: missing plane 0".to_string(),
            })?;
            let stride = frame.stride(0).unwrap_or(w * 4);
            let mut out = Vec::with_capacity(w * h * 4);
            for r in 0..h {
                let base = r * stride;
                for px in 0..w {
                    let i = base + px * 4;
                    out.push(plane[i + 2]); // R (was B)
                    out.push(plane[i + 1]); // G
                    out.push(plane[i]); // B (was R)
                    out.push(plane[i + 3]); // A
                }
            }
            Ok(out)
        }
        PixelFormat::Rgb24 => {
            let plane = frame.plane(0).ok_or_else(|| RenderError::Composite {
                message: "Rgb24 frame: missing plane 0".to_string(),
            })?;
            let stride = frame.stride(0).unwrap_or(w * 3);
            let mut out = Vec::with_capacity(w * h * 4);
            for r in 0..h {
                let base = r * stride;
                for px in 0..w {
                    let i = base + px * 3;
                    out.push(plane[i]);
                    out.push(plane[i + 1]);
                    out.push(plane[i + 2]);
                    out.push(255);
                }
            }
            Ok(out)
        }
        PixelFormat::Bgr24 => {
            let plane = frame.plane(0).ok_or_else(|| RenderError::Composite {
                message: "Bgr24 frame: missing plane 0".to_string(),
            })?;
            let stride = frame.stride(0).unwrap_or(w * 3);
            let mut out = Vec::with_capacity(w * h * 4);
            for r in 0..h {
                let base = r * stride;
                for px in 0..w {
                    let i = base + px * 3;
                    out.push(plane[i + 2]); // R (was B)
                    out.push(plane[i + 1]); // G
                    out.push(plane[i]); // B (was R)
                    out.push(255);
                }
            }
            Ok(out)
        }
        PixelFormat::Yuv420p => yuv_to_rgba(frame, 2, 2),
        PixelFormat::Yuv422p => yuv_to_rgba(frame, 2, 1),
        PixelFormat::Yuv444p => yuv_to_rgba(frame, 1, 1),
        other => Err(RenderError::UnsupportedFormat {
            format: format!("{other:?}"),
        }),
    }
}

/// Inline BT.601 YCbCr → RGBA conversion for planar 8-bit YUV formats.
///
/// `chroma_x_div` / `chroma_y_div` are the horizontal / vertical subsampling
/// divisors (e.g. 2/2 for 4:2:0, 2/1 for 4:2:2, 1/1 for 4:4:4).
#[allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn yuv_to_rgba(
    frame: &VideoFrame,
    chroma_x_div: usize,
    chroma_y_div: usize,
) -> Result<Vec<u8>, RenderError> {
    let w = frame.width() as usize;
    let h = frame.height() as usize;

    let y_plane = frame.plane(0).ok_or_else(|| RenderError::Composite {
        message: "YUV frame: missing Y plane".to_string(),
    })?;
    let u_plane = frame.plane(1).ok_or_else(|| RenderError::Composite {
        message: "YUV frame: missing U plane".to_string(),
    })?;
    let v_plane = frame.plane(2).ok_or_else(|| RenderError::Composite {
        message: "YUV frame: missing V plane".to_string(),
    })?;

    let y_stride = frame.stride(0).unwrap_or(w);
    let u_stride = frame.stride(1).unwrap_or(w.div_ceil(chroma_x_div));
    let v_stride = frame.stride(2).unwrap_or(w.div_ceil(chroma_x_div));

    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        for col in 0..w {
            let y = f32::from(y_plane[row * y_stride + col]) / 255.0;
            let u = f32::from(u_plane[(row / chroma_y_div) * u_stride + col / chroma_x_div])
                / 255.0
                - 0.5;
            let v = f32::from(v_plane[(row / chroma_y_div) * v_stride + col / chroma_x_div])
                / 255.0
                - 0.5;
            let r = (y + 1.402 * v).clamp(0.0, 1.0);
            let g = (y - 0.344_136 * u - 0.714_136 * v).clamp(0.0, 1.0);
            let b = (y + 1.772 * u).clamp(0.0, 1.0);
            out.push((r * 255.0 + 0.5) as u8);
            out.push((g * 255.0 + 0.5) as u8);
            out.push((b * 255.0 + 0.5) as u8);
            out.push(255);
        }
    }
    Ok(out)
}

// GPU helpers

/// Create a black `Rgba8Unorm` canvas texture suitable as a render target.
fn create_canvas(ctx: &Arc<RenderContext>, w: u32, h: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Compositor canvas"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// Create an intermediate output texture (same usage flags as the canvas).
fn create_output_tex(ctx: &Arc<RenderContext>, w: u32, h: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Compositor output"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// Run one blend shader pass: `base + overlay → output`.
#[allow(clippy::too_many_arguments)]
fn blend_textures(
    ctx: &Arc<RenderContext>,
    pipeline: &wgpu::RenderPipeline,
    bgl: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    uniform_buf: &wgpu::Buffer,
    base_tex: &wgpu::Texture,
    overlay_tex: &wgpu::Texture,
    output_tex: &wgpu::Texture,
    mode: BlendMode,
    opacity: f32,
) {
    let mode_bytes = (mode as u32).to_le_bytes();
    let opac_bytes = opacity.to_le_bytes();
    let uniforms: [u8; 16] = [
        mode_bytes[0],
        mode_bytes[1],
        mode_bytes[2],
        mode_bytes[3],
        opac_bytes[0],
        opac_bytes[1],
        opac_bytes[2],
        opac_bytes[3],
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    ctx.queue.write_buffer(uniform_buf, 0, &uniforms);

    let base_view = base_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let ov_view = overlay_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let out_view = output_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Compositor blend BG"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&base_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&ov_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniform_buf.as_entire_binding(),
            },
        ],
    });

    submit_render_pass(ctx, pipeline, &bind_group, &out_view, "Compositor blend");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

    fn rgba_frame(w: u32, h: u32) -> VideoFrame {
        VideoFrame::empty(w, h, PixelFormat::Rgba).expect("test frame")
    }

    fn yuv420_frame(w: u32, h: u32) -> VideoFrame {
        VideoFrame::empty(w, h, PixelFormat::Yuv420p).expect("test yuv frame")
    }

    fn rgb24_frame(w: u32, h: u32) -> VideoFrame {
        let stride = w as usize * 3;
        let data = vec![100u8, 150, 200].repeat(w as usize * h as usize);
        VideoFrame::new(
            vec![PooledBuffer::standalone(data)],
            vec![stride],
            w,
            h,
            PixelFormat::Rgb24,
            Timestamp::default(),
            false,
        )
        .expect("rgb24 frame")
    }

    fn bgra_frame(w: u32, h: u32) -> VideoFrame {
        let stride = w as usize * 4;
        let mut data = vec![0u8; stride * h as usize];
        for px in 0..w as usize * h as usize {
            data[px * 4] = 10; // B
            data[px * 4 + 1] = 20; // G
            data[px * 4 + 2] = 30; // R
            data[px * 4 + 3] = 255; // A
        }
        VideoFrame::new(
            vec![PooledBuffer::standalone(data)],
            vec![stride],
            w,
            h,
            PixelFormat::Bgra,
            Timestamp::default(),
            false,
        )
        .expect("bgra frame")
    }

    #[test]
    fn frame_to_rgba_rgba_should_return_correct_size() {
        let frame = rgba_frame(4, 4);
        let result = frame_to_rgba(&frame).expect("Rgba must succeed");
        assert_eq!(result.len(), 4 * 4 * 4, "output must be w*h*4 bytes");
    }

    #[test]
    fn frame_to_rgba_yuv420p_should_produce_rgba_output() {
        let frame = yuv420_frame(4, 4);
        let result = frame_to_rgba(&frame).expect("Yuv420p must succeed");
        assert_eq!(result.len(), 4 * 4 * 4, "YUV output must be w*h*4 bytes");
        // All pixels should have alpha=255.
        for chunk in result.chunks_exact(4) {
            assert_eq!(chunk[3], 255, "YUV output alpha must be 255");
        }
    }

    #[test]
    fn frame_to_rgba_rgb24_should_add_opaque_alpha() {
        let frame = rgb24_frame(2, 2);
        let result = frame_to_rgba(&frame).expect("Rgb24 must succeed");
        assert_eq!(result.len(), 2 * 2 * 4);
        for chunk in result.chunks_exact(4) {
            assert_eq!(chunk[0], 100, "R must be 100");
            assert_eq!(chunk[1], 150, "G must be 150");
            assert_eq!(chunk[2], 200, "B must be 200");
            assert_eq!(chunk[3], 255, "alpha must be 255");
        }
    }

    #[test]
    fn frame_to_rgba_bgra_should_swap_channels() {
        let frame = bgra_frame(1, 1);
        let result = frame_to_rgba(&frame).expect("Bgra must succeed");
        assert_eq!(result.len(), 4);
        assert_eq!(result[0], 30, "R must come from BGRA.r (index 2)");
        assert_eq!(result[1], 20, "G stays");
        assert_eq!(result[2], 10, "B must come from BGRA.b (index 0)");
        assert_eq!(result[3], 255, "A stays");
    }
}
