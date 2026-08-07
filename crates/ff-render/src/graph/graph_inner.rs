use std::sync::Arc;

use crate::context::RenderContext;
use crate::error::RenderError;
use crate::nodes::RenderNode;

/// Execute all `nodes` on the `rgba` input and return the processed RGBA bytes.
///
/// Allocates one pair of GPU textures per node — no texture pooling in Phase 1.
#[allow(clippy::too_many_lines)]
pub(super) fn run_gpu(
    nodes: &[Box<dyn RenderNode>],
    ctx: &Arc<RenderContext>,
    rgba: &[u8],
    w: u32,
    h: u32,
) -> Result<Vec<u8>, RenderError> {
    if nodes.is_empty() {
        return Ok(rgba.to_vec());
    }

    // Upload the input frame to the initial GPU texture.
    let input_tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ff-render input"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &input_tex,
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

    // Run each node: output of one node is the input of the next.
    let mut current_tex = input_tex;

    for node in nodes {
        let output_tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ff-render node output"),
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
        });

        node.process(&[&current_tex], &[&output_tex], ctx);
        current_tex = output_tex;
    }

    // Copy the final texture to a CPU-readable staging buffer.
    let bytes_per_row_padded = align_up(w * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer_size = u64::from(bytes_per_row_padded) * u64::from(h);

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
            texture: &current_tex,
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
            width: w,
            height: h,
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
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row_start = y * bytes_per_row_padded as usize;
        let row_end = row_start + (w * 4) as usize;
        out.extend_from_slice(&raw[row_start..row_end]);
    }
    drop(raw);
    staging_buf.unmap();

    Ok(out)
}

fn align_up(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
}
