//! 3D colour LUT node loaded from an Adobe `.cube` or Resolve `.3dl` file.
//!
//! The LUT is a `size^3` grid of RGB output triples applied with trilinear
//! interpolation. The grid is stored red-fastest (`idx = r + size*(g + size*b)`),
//! which is the order a `wgpu` 3D texture expects (x = r, y = g, z = b), so the
//! CPU path, the GPU upload, and the shader all index it the same way.

use std::path::Path;

use super::RenderNodeCpu;
use crate::error::RenderError;

/// Applies a 3D colour LUT to a frame via trilinear interpolation.
///
/// Load one with [`LutNode::from_cube`] or [`LutNode::from_3dl`]. The GPU path
/// renders into an `Rgba8Unorm` target like the other effect nodes, so it is
/// 8-bit only (a high-bit-depth graph falls back as it does for every effect
/// node today).
pub struct LutNode {
    /// `size^3` RGB output triples, red-fastest: `lut[r + size*(g + size*b)]`.
    lut: Vec<[f32; 3]>,
    /// Grid size per axis (typically 17, 33, or 64).
    size: u32,
    #[cfg(feature = "wgpu")]
    pipeline: std::sync::OnceLock<LutPipeline>,
}

impl LutNode {
    /// Builds a node from an already-parsed grid (red-fastest order). Used by the
    /// parsers and tests.
    fn from_grid(lut: Vec<[f32; 3]>, size: u32) -> Self {
        Self {
            lut,
            size,
            #[cfg(feature = "wgpu")]
            pipeline: std::sync::OnceLock::new(),
        }
    }

    /// Loads a 3D LUT from an Adobe `.cube` file.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::LutLoad`] if the file cannot be read or is malformed
    /// (missing `LUT_3D_SIZE`, a bad data line, or the wrong entry count).
    pub fn from_cube(path: &Path) -> Result<Self, RenderError> {
        let text = read_lut_file(path)?;
        let (lut, size) = parse_cube(&text).map_err(|reason| RenderError::LutLoad {
            path: path.display().to_string(),
            reason,
        })?;
        Ok(Self::from_grid(lut, size))
    }

    /// Loads a 3D LUT from a Resolve `.3dl` file.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::LutLoad`] if the file cannot be read or the entry
    /// count is not a perfect cube.
    pub fn from_3dl(path: &Path) -> Result<Self, RenderError> {
        let text = read_lut_file(path)?;
        let (lut, size) = parse_3dl(&text).map_err(|reason| RenderError::LutLoad {
            path: path.display().to_string(),
            reason,
        })?;
        Ok(Self::from_grid(lut, size))
    }

    /// Trilinearly samples the grid at `(r, g, b)`, each in `[0, 1]`. The mapping
    /// (`v * (size - 1)`) and corner order match `lut.wgsl` exactly.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::many_single_char_names
    )]
    fn sample(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let n = self.size as usize;
        let last = (n - 1) as f32;
        // Returns (lo index, hi index, fraction) for one axis.
        let axis = |v: f32| {
            let c = v.clamp(0.0, 1.0) * last;
            let lo = c.floor();
            let li = (lo as usize).min(n - 1);
            (li, (li + 1).min(n - 1), c - lo)
        };
        let (r0, r1, fr) = axis(r);
        let (g0, g1, fg) = axis(g);
        let (b0, b1, fb) = axis(b);
        let at = |ri: usize, gi: usize, bi: usize| self.lut[ri + n * (gi + n * bi)];
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        };
        let c00 = lerp(at(r0, g0, b0), at(r1, g0, b0), fr);
        let c10 = lerp(at(r0, g1, b0), at(r1, g1, b0), fr);
        let c01 = lerp(at(r0, g0, b1), at(r1, g0, b1), fr);
        let c11 = lerp(at(r0, g1, b1), at(r1, g1, b1), fr);
        let c0 = lerp(c00, c10, fg);
        let c1 = lerp(c01, c11, fg);
        lerp(c0, c1, fb)
    }
}

/// Reads a LUT file to a string, mapping IO failure to [`RenderError::LutLoad`].
fn read_lut_file(path: &Path) -> Result<String, RenderError> {
    std::fs::read_to_string(path).map_err(|e| RenderError::LutLoad {
        path: path.display().to_string(),
        reason: e.to_string(),
    })
}

/// Parses an Adobe `.cube` file into a red-fastest grid.
///
/// `.cube` data lines list `R G B` with the **red** component varying fastest
/// (verified against `FFmpeg` `vf_lut3d.c`), which is exactly the red-fastest
/// storage order, so entries fill the grid in file order.
#[allow(clippy::cast_possible_truncation)] // size is validated <= 256
fn parse_cube(text: &str) -> Result<(Vec<[f32; 3]>, u32), String> {
    let mut size: Option<usize> = None;
    let mut entries: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
            let n: usize = rest
                .trim()
                .parse()
                .map_err(|_| format!("invalid LUT_3D_SIZE: {rest}"))?;
            if !(2..=256).contains(&n) {
                return Err(format!("LUT_3D_SIZE out of range: {n}"));
            }
            size = Some(n);
            continue;
        }
        // Skip other keyword lines (DOMAIN_MIN/MAX, TITLE, ...); a data line
        // begins with a digit, '-', or '.'.
        if line
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            continue;
        }
        let vals: Vec<f32> = line
            .split_whitespace()
            .map(str::parse::<f32>)
            .collect::<Result<_, _>>()
            .map_err(|_| format!("invalid data line: {line}"))?;
        if vals.len() != 3 {
            return Err(format!("expected 3 floats, got {}: {line}", vals.len()));
        }
        if !vals.iter().all(|v| v.is_finite()) {
            return Err(format!("non-finite value: {line}"));
        }
        entries.push([vals[0], vals[1], vals[2]]);
    }
    let size = size.ok_or("missing LUT_3D_SIZE")?;
    let expected = size * size * size;
    if entries.len() != expected {
        return Err(format!(
            "expected {expected} entries for size {size}, got {}",
            entries.len()
        ));
    }
    Ok((entries, size as u32))
}

/// Parses a Resolve `.3dl` file into a red-fastest grid.
///
/// The first non-comment line is the input mesh; its token count is the grid
/// size. The `size^3` data lines that follow list `R G B` integers with the
/// **blue** component varying fastest (verified against `FFmpeg` `vf_lut3d.c`),
/// so each entry is placed at its `(r, g, b)` position, transposing into the
/// red-fastest store. Integers are normalised by the smallest `2^k - 1` at least
/// as large as the maximum seen (e.g. 1023 for 10-bit, 4095 for 12-bit).
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)] // size is validated <= 256
fn parse_3dl(text: &str) -> Result<(Vec<[f32; 3]>, u32), String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let header = lines.next().ok_or("empty .3dl")?;
    let size = header.split_whitespace().count();
    if !(2..=256).contains(&size) {
        return Err(format!("invalid .3dl mesh size: {size}"));
    }
    let mut ints: Vec<[u32; 3]> = Vec::new();
    let mut max_seen: u32 = 0;
    for line in lines {
        let v: Vec<u32> = line
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<Result<_, _>>()
            .map_err(|_| format!("invalid .3dl data line: {line}"))?;
        if v.len() != 3 {
            return Err(format!("expected an R G B triple: {line}"));
        }
        max_seen = max_seen.max(v[0]).max(v[1]).max(v[2]);
        ints.push([v[0], v[1], v[2]]);
    }
    let expected = size * size * size;
    if ints.len() != expected {
        return Err(format!(
            "expected {expected} entries for size {size}, got {}",
            ints.len()
        ));
    }
    let denom = normalise_denominator(max_seen) as f32;
    let mut lut = vec![[0.0f32; 3]; expected];
    for (n, e) in ints.iter().enumerate() {
        // Blue fastest in the file; place at the red-fastest store index.
        let b = n % size;
        let g = (n / size) % size;
        let r = n / (size * size);
        lut[r + size * (g + size * b)] = [
            e[0] as f32 / denom,
            e[1] as f32 / denom,
            e[2] as f32 / denom,
        ];
    }
    Ok((lut, size as u32))
}

/// The smallest `2^k - 1` (for `k` in `8..=16`) that is at least `max`, used to
/// normalise `.3dl` integer entries to `[0, 1]`.
fn normalise_denominator(max: u32) -> u32 {
    (8..=16)
        .map(|bits| (1u32 << bits) - 1)
        .find(|&d| d >= max)
        .unwrap_or((1u32 << 16) - 1)
}

// CPU path

impl RenderNodeCpu for LutNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn process_cpu(&self, rgba: &mut [u8], _w: u32, _h: u32) {
        for px in rgba.as_chunks_mut::<4>().0 {
            let [r, g, b] = self.sample(
                f32::from(px[0]) / 255.0,
                f32::from(px[1]) / 255.0,
                f32::from(px[2]) / 255.0,
            );
            px[0] = (r * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            px[1] = (g * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            px[2] = (b * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            // alpha unchanged
        }
    }
}

// GPU path

#[cfg(feature = "wgpu")]
struct LutPipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    lut_texture: wgpu::Texture,
}

#[cfg(feature = "wgpu")]
fn lut_texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    // Unfilterable float: the shader reads the LUT with textureLoad (no sampler),
    // so an Rgba32Float 3D texture binds without the float32-filterable feature.
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    }
}

#[cfg(feature = "wgpu")]
impl LutNode {
    fn get_or_create_pipeline(&self, ctx: &crate::context::RenderContext) -> &LutPipeline {
        self.pipeline.get_or_init(|| {
            use super::blur::{fullscreen_pipeline, texture_entry};
            let device = &ctx.device;
            let size = self.size;

            let mut texels: Vec<u8> = Vec::with_capacity(self.lut.len() * 16);
            for px in &self.lut {
                for c in [px[0], px[1], px[2], 1.0] {
                    texels.extend_from_slice(&c.to_le_bytes());
                }
            }
            let lut_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Lut 3D"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: size,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &lut_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 16),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: size,
                },
            );

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Lut shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/lut.wgsl").into()),
            });
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Lut BGL"),
                entries: &[texture_entry(0), lut_texture_entry(1)],
            });
            let render_pipeline = fullscreen_pipeline(device, &shader, &bgl, "Lut");

            LutPipeline {
                render_pipeline,
                bind_group_layout: bgl,
                lut_texture,
            }
        })
    }
}

#[cfg(feature = "wgpu")]
impl super::RenderNode for LutNode {
    fn process(
        &self,
        inputs: &[&wgpu::Texture],
        outputs: &[&wgpu::Texture],
        ctx: &crate::context::RenderContext,
    ) {
        let Some(input) = inputs.first() else {
            log::warn!("LutNode::process called with no inputs");
            return;
        };
        let Some(output) = outputs.first() else {
            log::warn!("LutNode::process called with no outputs");
            return;
        };
        let pd = self.get_or_create_pipeline(ctx);
        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let lut_view = pd
            .lut_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lut BG"),
            layout: &pd.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lut_view),
                },
            ],
        });
        super::blur::run_fullscreen(
            ctx,
            &pd.render_pipeline,
            &bind_group,
            &output_view,
            "Lut pass",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an identity `.cube` of the given size (red-fastest order).
    fn identity_cube(size: usize) -> String {
        let mut s = format!("# test\nLUT_3D_SIZE {size}\n");
        let last = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let (rf, gf, bf) = (r as f32 / last, g as f32 / last, b as f32 / last);
                    s.push_str(&format!("{rf} {gf} {bf}\n"));
                }
            }
        }
        s
    }

    #[test]
    fn parse_cube_should_read_identity_grid() {
        let (lut, size) = parse_cube(&identity_cube(2)).expect("parse");
        assert_eq!(size, 2);
        assert_eq!(lut.len(), 8);
        // Grid point (r=1, g=0, b=0) -> idx 1 -> [1,0,0].
        assert_eq!(lut[1], [1.0, 0.0, 0.0]);
        // Grid point (r=0, g=0, b=1) -> idx 4 -> [0,0,1].
        assert_eq!(lut[4], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn parse_cube_missing_size_should_error() {
        assert!(parse_cube("0.0 0.0 0.0\n").is_err());
    }

    #[test]
    fn lut_identity_should_be_noop() {
        let (lut, size) = parse_cube(&identity_cube(17)).expect("parse");
        let node = LutNode::from_grid(lut, size);
        let original = vec![10u8, 128, 220, 255, 60, 90, 200, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 2, 1);
        for (a, b) in rgba.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "identity LUT must preserve the pixel; got {a} vs {b}"
            );
        }
    }

    #[test]
    fn lut_known_shift_should_match_reference() {
        // A size-2 LUT that halves every channel: every grid output = input/2.
        let mut lut = vec![[0.0f32; 3]; 8];
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    lut[r + 2 * (g + 2 * b)] = [r as f32 / 2.0, g as f32 / 2.0, b as f32 / 2.0];
                }
            }
        }
        let node = LutNode::from_grid(lut, 2);
        let mut rgba = vec![200u8, 100, 40, 255];
        node.process_cpu(&mut rgba, 1, 1);
        // Linear LUT: out = in/2 (trilinear over the unit cube is exact here).
        for (got, inp) in rgba[..3].iter().zip([200u8, 100, 40]) {
            let expected = (f32::from(inp) / 2.0).round() as i32;
            assert!(
                (i32::from(*got) - expected).abs() <= 1,
                "halving LUT: got {got}, expected ~{expected}"
            );
        }
    }

    #[test]
    fn from_cube_missing_file_should_return_lut_load() {
        assert!(matches!(
            LutNode::from_cube(Path::new("does-not-exist-9f3a.cube")),
            Err(RenderError::LutLoad { .. })
        ));
    }

    #[test]
    fn from_3dl_missing_file_should_return_lut_load() {
        assert!(matches!(
            LutNode::from_3dl(Path::new("does-not-exist-3a9f.3dl")),
            Err(RenderError::LutLoad { .. })
        ));
    }

    #[test]
    fn from_cube_should_load_a_valid_file() {
        // Exercises the public happy path (read_lut_file -> parse_cube -> node),
        // which the parse-level tests bypass, via a real temp file on disk.
        let path = std::env::temp_dir().join(format!("avio_lut_{}.cube", std::process::id()));
        std::fs::write(&path, identity_cube(9)).expect("write temp cube");
        let node = LutNode::from_cube(&path).expect("load cube");
        let _ = std::fs::remove_file(&path);
        let original = vec![10u8, 128, 220, 255];
        let mut rgba = original.clone();
        node.process_cpu(&mut rgba, 1, 1);
        for (a, b) in rgba.iter().zip(original.iter()) {
            assert!(
                (i32::from(*a) - i32::from(*b)).abs() <= 1,
                "a loaded identity .cube must preserve the pixel; got {a} vs {b}"
            );
        }
    }

    #[test]
    fn parse_3dl_should_read_grid() {
        // Identity size-2 .3dl: a 2-token mesh header, then blue-fastest data
        // triples in the 0..1023 range.
        let mut s = String::from("# mesh\n0 1023\n");
        for r in 0..2 {
            for g in 0..2 {
                for b in 0..2 {
                    let q = |v: usize| v * 1023;
                    s.push_str(&format!("{} {} {}\n", q(r), q(g), q(b)));
                }
            }
        }
        let (lut, size) = parse_3dl(&s).expect("parse 3dl");
        assert_eq!(size, 2);
        // (r=1,g=0,b=0) -> idx 1 -> ~[1,0,0].
        assert!((lut[1][0] - 1.0).abs() < 1e-3 && lut[1][1].abs() < 1e-3);
        // (r=0,g=0,b=1) -> idx 4 -> ~[0,0,1].
        assert!((lut[4][2] - 1.0).abs() < 1e-3 && lut[4][0].abs() < 1e-3);
    }
}

#[cfg(all(test, feature = "wgpu"))]
mod gpu_tests {
    use super::*;
    use crate::context::RenderContext;
    use crate::graph::RenderGraph;
    use std::sync::Arc;

    fn ctx() -> Option<Arc<RenderContext>> {
        match futures::executor::block_on(RenderContext::init()) {
            Ok(ctx) => Some(Arc::new(ctx)),
            Err(_) => None,
        }
    }

    fn identity_node(size: usize) -> LutNode {
        let last = (size - 1) as f32;
        let mut lut = vec![[0.0f32; 3]; size * size * size];
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    lut[r + size * (g + size * b)] =
                        [r as f32 / last, g as f32 / last, b as f32 / last];
                }
            }
        }
        LutNode::from_grid(lut, size as u32)
    }

    #[test]
    fn lut_gpu_identity_should_preserve_input() {
        let Some(ctx) = ctx() else {
            return;
        };
        let frame = vec![10u8, 128, 220, 255, 60, 90, 200, 255];
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(identity_node(17))
            .process_gpu(&frame, 2, 1)
            .expect("gpu lut");
        for i in 0..frame.len() {
            assert!(
                (i32::from(gpu[i]) - i32::from(frame[i])).abs() <= 2,
                "identity LUT must preserve the input on the GPU path at {i}"
            );
        }
    }

    #[test]
    fn lut_gpu_halving_should_match_cpu() {
        let Some(ctx) = ctx() else {
            return;
        };
        // size-2 halving LUT.
        let mut lut = vec![[0.0f32; 3]; 8];
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    lut[r + 2 * (g + 2 * b)] = [r as f32 / 2.0, g as f32 / 2.0, b as f32 / 2.0];
                }
            }
        }
        let frame = vec![200u8, 100, 40, 255];
        let gpu = RenderGraph::new(Arc::clone(&ctx))
            .push(LutNode::from_grid(lut.clone(), 2))
            .process_gpu(&frame, 1, 1)
            .expect("gpu lut");
        let mut cpu = frame.clone();
        LutNode::from_grid(lut, 2).process_cpu(&mut cpu, 1, 1);
        for i in 0..3 {
            assert!(
                (i32::from(gpu[i]) - i32::from(cpu[i])).abs() <= 2,
                "GPU and CPU LUT must agree at channel {i}: gpu={} cpu={}",
                gpu[i],
                cpu[i]
            );
        }
    }
}
