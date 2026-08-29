use std::sync::Arc;

use crate::context::RenderContext;
use crate::error::RenderError;
use crate::nodes::RenderNode;
use crate::pool::FrameScope;
use crate::sink::TextureHandle;

/// Acquire textures, upload the input, and run every node. Returns the scope
/// (holding all frame textures) and the index of the final composited texture.
///
/// Textures are drawn from the context's [`TexturePool`](crate::pool) via the
/// returned [`FrameScope`]; how the final texture is consumed (read back, or
/// taken out for display) is up to the caller, which owns the scope.
#[allow(clippy::too_many_lines)]
fn execute_nodes<'a>(
    nodes: &[Box<dyn RenderNode>],
    ctx: &'a Arc<RenderContext>,
    rgba: &[u8],
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> (FrameScope<'a>, usize) {
    let input_usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING;
    // Outputs double as general-purpose scratch: `COPY_DST` lets a node (or test)
    // fill a pass target via `write_texture`, `COPY_SRC` allows readback, and
    // `TEXTURE_BINDING` lets a later node sample it. Harmless to render-only nodes.
    let output_usage = wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST
        | wgpu::TextureUsages::TEXTURE_BINDING;

    // Acquire every texture for the frame up front, so the process loop below
    // holds only shared borrows of the scope (further acquires would need
    // `&mut`). The scope returns all of them to the pool when it drops.
    //
    // Per node we allocate `pass_count()` output textures; the node writes its
    // final result into the last one, which becomes the next node's input[0].
    let mut scope = FrameScope::new(&ctx.pool, &ctx.device);
    let input_idx = scope.acquire(w, h, format, input_usage);
    // A node may resize (e.g. ScaleNode); thread the running size so each node's
    // output targets — and thus the next node's input — are the right size.
    // input[1..] (the original source) stays at the initial `w` x `h`.
    let mut cur_w = w;
    let mut cur_h = h;
    let mut output_idx: Vec<Vec<usize>> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let (out_w, out_h) = node.output_dimensions(cur_w, cur_h);
        let passes: Vec<usize> = (0..node.pass_count().max(1))
            .map(|_| scope.acquire(out_w, out_h, format, output_usage))
            .collect();
        output_idx.push(passes);
        cur_w = out_w;
        cur_h = out_h;
    }

    // Upload the 8-bit `rgba` frame to the initial GPU texture. Only valid for
    // an `Rgba8Unorm` working format: an HDR (`Rgba16Float`) graph is driven by a
    // source node (e.g. `YuvUploadNode`, `input_count() == 0`) that supplies its
    // own high-bit-depth pixels and ignores this seed, so the 8-bit upload — which
    // would not fit an 8-bytes-per-pixel texture — is skipped.
    if format == wgpu::TextureFormat::Rgba8Unorm {
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: scope.get(input_idx),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
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

    // Run each node. input[0] is the chained texture (the source frame for the
    // first node, the previous node's final pass otherwise); input[1..] are the
    // original source frame. Outputs are the node's `pass_count()` targets; the
    // node's final pass, output_idx[last], becomes the next input[0].
    let mut current_idx = input_idx;
    for (node, passes) in nodes.iter().zip(&output_idx) {
        let inputs: Vec<&wgpu::Texture> = (0..node.input_count())
            .map(|i| {
                if i == 0 {
                    scope.get(current_idx)
                } else {
                    scope.get(input_idx)
                }
            })
            .collect();
        let outputs: Vec<&wgpu::Texture> = passes.iter().map(|&idx| scope.get(idx)).collect();
        node.process(&inputs, &outputs, ctx);
        // `passes` is always non-empty (`pass_count().max(1) >= 1`); the final
        // pass becomes the next node's input[0].
        if let Some(&last) = passes.last() {
            current_idx = last;
        }
    }

    (scope, current_idx)
}

/// Execute all `nodes` on the `rgba` input and return the processed RGBA bytes.
///
/// Reads the final texture back to system memory (a GPU-to-CPU copy per frame).
/// For a zero-copy display path, use [`run_gpu_to_texture`] instead.
pub(super) fn run_gpu(
    nodes: &[Box<dyn RenderNode>],
    ctx: &Arc<RenderContext>,
    rgba: &[u8],
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> Result<Vec<u8>, RenderError> {
    if nodes.is_empty() {
        return Ok(rgba.to_vec());
    }

    // Bytes per pixel of the working format: 8 for Rgba16Float (HDR), 4 for the
    // 4-channel 8-bit default. Drives the readback stride.
    let bpp = bytes_per_pixel(format);
    let (scope, current_idx) = execute_nodes(nodes, ctx, rgba, w, h, format);
    ctx.note_readback();

    // Read back at the final texture's actual size — a node may have resized it
    // (e.g. ScaleNode), so it can differ from the input `w` x `h`.
    let (out_w, out_h) = {
        let tex = scope.get(current_idx);
        (tex.width(), tex.height())
    };

    // Copy the final texture to a CPU-readable staging buffer.
    let bytes_per_row_padded = align_up(out_w * bpp, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer_size = u64::from(bytes_per_row_padded) * u64::from(out_h);

    let staging_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ff-render staging"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ff-render readback"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: scope.get(current_idx),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging_buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row_padded),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: out_w,
            height: out_h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit(std::iter::once(encoder.finish()));

    // Map the staging buffer synchronously.
    let staging_slice = staging_buf.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    staging_slice.map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).ok();
    });
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).ok();

    receiver
        .recv()
        .map_err(|_| RenderError::Composite {
            message: "staging buffer channel closed unexpectedly".to_string(),
        })?
        .map_err(|e| RenderError::Composite {
            message: format!("staging buffer map failed: {e}"),
        })?;

    // Strip row padding from the staged data.
    let raw = staging_slice
        .get_mapped_range()
        .map_err(|e| RenderError::Composite {
            message: format!("staging buffer get_mapped_range failed: {e}"),
        })?;
    let mut out = Vec::with_capacity((out_w * out_h * bpp) as usize);
    for y in 0..out_h as usize {
        let row_start = y * bytes_per_row_padded as usize;
        let row_end = row_start + (out_w * bpp) as usize;
        out.extend_from_slice(&raw[row_start..row_end]);
    }
    drop(raw);
    staging_buf.unmap();

    Ok(out)
}

/// Execute all `nodes` and hand the final composited texture to the caller as a
/// [`TextureHandle`], **without** any GPU-to-CPU readback.
///
/// The texture is taken out of the pool (ownership transfers to the caller), so
/// it stays valid for direct display; the graph's intermediate textures are
/// returned to the pool. No staging buffer is mapped, so [`RenderContext`]'s
/// readback counter is not incremented.
pub(super) fn run_gpu_to_texture(
    nodes: &[Box<dyn RenderNode>],
    ctx: &Arc<RenderContext>,
    rgba: &[u8],
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> Result<TextureHandle, RenderError> {
    let (scope, current_idx) = execute_nodes(nodes, ctx, rgba, w, h, format);
    let texture = scope
        .take(current_idx)
        .ok_or_else(|| RenderError::Composite {
            message: "no composited texture to display".to_string(),
        })?;
    // The final texture may have been resized by a node (e.g. ScaleNode), so the
    // handle reports the texture's actual dimensions rather than the input size.
    let (out_w, out_h) = (texture.width(), texture.height());
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(TextureHandle {
        texture,
        view,
        width: out_w,
        height: out_h,
    })
}

fn align_up(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
}

/// Bytes per pixel of a working texture format. `Rgba16Float` is 8 (four 16-bit
/// half-floats); every other format the graph uses is 4-channel 8-bit (4 bytes).
fn bytes_per_pixel(format: wgpu::TextureFormat) -> u32 {
    match format {
        wgpu::TextureFormat::Rgba16Float => 8,
        _ => 4,
    }
}
