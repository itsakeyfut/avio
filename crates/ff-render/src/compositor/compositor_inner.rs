use std::collections::HashMap;
use std::sync::Arc;

use ff_format::{PixelFormat, VideoFrame};

use crate::context::RenderContext;
use crate::error::RenderError;
use crate::nodes::composite::{
    fullscreen_pipeline, linear_sampler, submit_render_pass, two_tex_sampler_uniform_bgl,
    upload_rgba_texture,
};
use crate::nodes::upload::chroma_dims;
use crate::nodes::{BlendMode, RenderNode, TransformNode, YuvFormat, YuvUploadNode};

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
    /// GPU YUV-upload nodes, one per distinct `(format, width, height)`. Cached
    /// so the upload pipeline is built once per config and reused across frames
    /// (creating a node per frame would recompile the shader every frame).
    yuv_nodes: HashMap<(YuvFormat, u32, u32), YuvUploadNode>,
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
            yuv_nodes: HashMap::new(),
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
            let src_tex = ingest_layer(&mut self.yuv_nodes, ctx, &layer.frame, fw, fh)?;

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

// Layer ingest

/// Produce a GPU RGBA texture for one layer's frame.
///
/// Planar 8-bit YUV is uploaded and converted on the GPU via a cached
/// [`YuvUploadNode`] (the single YUV → RGB path, BT.601). Every other supported
/// format is converted on the CPU by [`frame_to_rgba`] and uploaded.
fn ingest_layer(
    yuv_nodes: &mut HashMap<(YuvFormat, u32, u32), YuvUploadNode>,
    ctx: &Arc<RenderContext>,
    frame: &VideoFrame,
    fw: u32,
    fh: u32,
) -> Result<wgpu::Texture, RenderError> {
    if let Some(yuv_format) = yuv_format_of(frame.format()) {
        let (y, cb, cr) = extract_dense_yuv(frame, yuv_format, fw, fh)?;
        let node = yuv_nodes
            .entry((yuv_format, fw, fh))
            .or_insert_with(|| YuvUploadNode::new(yuv_format, fw, fh));
        node.set_planes(y, cb, cr);
        let out_tex = create_output_tex(ctx, fw, fh);
        node.process(&[], &[&out_tex], ctx);
        Ok(out_tex)
    } else {
        let rgba = frame_to_rgba(frame)?;
        Ok(upload_rgba_texture(ctx, &rgba, fw, fh, "Compositor src"))
    }
}

/// Map a planar 8-bit YUV [`PixelFormat`] to a [`YuvFormat`]; `None` for
/// anything the GPU upload node does not handle.
fn yuv_format_of(format: PixelFormat) -> Option<YuvFormat> {
    match format {
        PixelFormat::Yuv420p => Some(YuvFormat::Yuv420p),
        PixelFormat::Yuv422p => Some(YuvFormat::Yuv422p),
        PixelFormat::Yuv444p => Some(YuvFormat::Yuv444p),
        _ => None,
    }
}

/// Dense Y, Cb, Cr plane buffers.
type YuvPlanes = (Vec<u8>, Vec<u8>, Vec<u8>);

/// Extract dense (stride-free) Y, Cb, Cr planes sized as [`YuvUploadNode`]
/// expects: Y is `fw × fh`, Cb/Cr are the sub-sampled chroma dimensions.
fn extract_dense_yuv(
    frame: &VideoFrame,
    format: YuvFormat,
    fw: u32,
    fh: u32,
) -> Result<YuvPlanes, RenderError> {
    let (cw, ch) = chroma_dims(format, fw, fh);
    let y = dense_plane(frame, 0, fw as usize, fh as usize)?;
    let cb = dense_plane(frame, 1, cw as usize, ch as usize)?;
    let cr = dense_plane(frame, 2, cw as usize, ch as usize)?;
    Ok((y, cb, cr))
}

/// Copy plane `idx` into a tightly packed `w × h` buffer, stripping any stride
/// padding.
fn dense_plane(frame: &VideoFrame, idx: usize, w: usize, h: usize) -> Result<Vec<u8>, RenderError> {
    let plane = frame.plane(idx).ok_or_else(|| RenderError::Composite {
        message: format!("YUV frame: missing plane {idx}"),
    })?;
    let stride = frame.stride(idx).unwrap_or(w);
    if stride == w {
        Ok(plane[..w * h].to_vec())
    } else {
        let mut out = Vec::with_capacity(w * h);
        for r in 0..h {
            out.extend_from_slice(&plane[r * stride..r * stride + w]);
        }
        Ok(out)
    }
}

// Frame → RGBA conversion

/// Convert a supported non-YUV `VideoFrame` to a dense RGBA byte buffer.
///
/// Planar YUV is handled on the GPU by [`ingest_layer`] (via [`YuvUploadNode`]),
/// so it is rejected here with `RenderError::UnsupportedFormat`, as is any other
/// unrecognised format.
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
        other => Err(RenderError::UnsupportedFormat {
            format: format!("{other:?}"),
        }),
    }
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
    fn frame_to_rgba_yuv_should_be_unsupported() {
        // YUV now goes through the GPU YuvUploadNode path (single YUV → RGB
        // source of truth), so the CPU frame_to_rgba rejects it.
        let frame = yuv420_frame(4, 4);
        let result = frame_to_rgba(&frame);
        assert!(
            matches!(result, Err(RenderError::UnsupportedFormat { .. })),
            "YUV must be routed through the GPU node, not frame_to_rgba"
        );
    }

    #[test]
    fn yuv_format_of_should_map_planar_yuv_and_reject_others() {
        assert_eq!(
            yuv_format_of(PixelFormat::Yuv420p),
            Some(YuvFormat::Yuv420p)
        );
        assert_eq!(
            yuv_format_of(PixelFormat::Yuv422p),
            Some(YuvFormat::Yuv422p)
        );
        assert_eq!(
            yuv_format_of(PixelFormat::Yuv444p),
            Some(YuvFormat::Yuv444p)
        );
        assert_eq!(yuv_format_of(PixelFormat::Rgba), None);
        assert_eq!(yuv_format_of(PixelFormat::Yuv420p10le), None);
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

    /// A headless GPU context, or `None` when no adapter is available (CI).
    fn gpu_ctx() -> Option<Arc<RenderContext>> {
        match futures::executor::block_on(RenderContext::init()) {
            Ok(ctx) => Some(Arc::new(ctx)),
            Err(_) => None,
        }
    }

    /// Read an `Rgba8Unorm` texture back to a dense `w * h * 4` byte buffer.
    fn readback(ctx: &RenderContext, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bpr = (w * 4 + align - 1) & !(align - 1);
        let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test readback"),
            size: u64::from(bpr) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test readback"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit(std::iter::once(enc.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).ok();
        });
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        rx.recv().expect("map channel").expect("map result");
        let raw = slice.get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let s = y * bpr as usize;
            out.extend_from_slice(&raw[s..s + (w * 4) as usize]);
        }
        drop(raw);
        staging.unmap();
        out
    }

    /// A Yuv420p frame filled with constant Y, U, V (dense, no stride padding).
    fn yuv420_solid(w: u32, h: u32, yv: u8, uv: u8, vv: u8) -> VideoFrame {
        let (cw, ch) = chroma_dims(YuvFormat::Yuv420p, w, h);
        let y = vec![yv; (w * h) as usize];
        let u = vec![uv; (cw * ch) as usize];
        let v = vec![vv; (cw * ch) as usize];
        VideoFrame::new(
            vec![
                PooledBuffer::standalone(y),
                PooledBuffer::standalone(u),
                PooledBuffer::standalone(v),
            ],
            vec![w as usize, cw as usize, cw as usize],
            w,
            h,
            PixelFormat::Yuv420p,
            Timestamp::default(),
            false,
        )
        .expect("yuv420 frame")
    }

    #[test]
    fn compositor_yuv_ingest_should_match_node_path_within_tolerance() {
        let Some(ctx) = gpu_ctx() else {
            return;
        };
        let (w, h) = (4u32, 4u32);
        let (yv, uv, vv) = (150u8, 100u8, 170u8);
        let (cw, ch) = chroma_dims(YuvFormat::Yuv420p, w, h);

        // Node path: the same planes straight through YuvUploadNode.
        let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, w, h);
        node.set_planes(
            vec![yv; (w * h) as usize],
            vec![uv; (cw * ch) as usize],
            vec![vv; (cw * ch) as usize],
        );
        let node_rgb = crate::graph::RenderGraph::new(Arc::clone(&ctx))
            .push(node)
            .process_gpu(&vec![0u8; (w * h * 4) as usize], w, h)
            .expect("node path");

        // Compositor path: one opaque YUV layer over the (black) canvas.
        let mut compositor = crate::Compositor::new(Arc::clone(&ctx), w, h);
        let mut layers = vec![crate::FrameLayer {
            frame: yuv420_solid(w, h, yv, uv, vv),
            transform: crate::LayerTransform::default(),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
            z_order: 0,
        }];
        let tex = compositor.composite(&mut layers).expect("compositor path");
        let comp_rgb = readback(&ctx, &tex, w, h);

        assert_eq!(
            comp_rgb.len(),
            node_rgb.len(),
            "both paths must produce w*h*4 bytes"
        );
        for (c, n) in comp_rgb.chunks_exact(4).zip(node_rgb.chunks_exact(4)) {
            for ch in 0..3 {
                assert!(
                    (i32::from(c[ch]) - i32::from(n[ch])).abs() <= 6,
                    "compositor and node RGB must match within tolerance; ch={ch} comp={} node={}",
                    c[ch],
                    n[ch]
                );
            }
        }
    }
}
