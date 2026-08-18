//! Video scope analysis tools.
//!
//! Provides frame-level pixel analysis for video quality and colour monitoring.
//! All functions operate directly on [`ff_format::VideoFrame`] data — no `FFmpeg`
//! dependency; pure Rust pixel arithmetic.
//!
//! Currently implemented:
//! - [`ScopeAnalyzer::waveform`] — luminance waveform monitor (Y values per column)
//! - [`ScopeAnalyzer::vectorscope`] — Cb/Cr chroma scatter data
//! - [`ScopeAnalyzer::rgb_parade`] — per-channel RGB waveform (parade)
//! - [`ScopeAnalyzer::histogram`] — 256-bin luminance and per-channel RGB histogram
//!

use ff_format::{PixelFormat, VideoFrame};

/// Scope analysis utilities for decoded video frames.
///
/// All methods are associated functions (no instance state).
pub struct ScopeAnalyzer;

/// 256-bin luminance and per-channel RGB histogram.
pub struct Histogram {
    /// Red channel bin counts (8-bit value → bin index).
    pub r: [u32; 256],
    /// Green channel bin counts (8-bit value → bin index).
    pub g: [u32; 256],
    /// Blue channel bin counts (8-bit value → bin index).
    pub b: [u32; 256],
    /// Luminance bin counts (Y plane for YUV frames; BT.601-derived for others).
    pub luma: [u32; 256],
}

/// Per-channel waveform monitor data (RGB parade).
///
/// Each channel has the same shape as [`ScopeAnalyzer::waveform`]:
/// outer index = column (x), inner values are normalised channel values `[0.0, 1.0]`.
pub struct RgbParade {
    /// Red channel: column-major waveform values in `[0.0, 1.0]`.
    pub r: Vec<Vec<f32>>,
    /// Green channel: column-major waveform values in `[0.0, 1.0]`.
    pub g: Vec<Vec<f32>>,
    /// Blue channel: column-major waveform values in `[0.0, 1.0]`.
    pub b: Vec<Vec<f32>>,
}

impl ScopeAnalyzer {
    /// Compute waveform monitor data for `frame`.
    ///
    /// Returns a [`Vec`] of length `frame.width()`. Each inner [`Vec`] contains
    /// the normalised Y (luma) values `[0.0, 1.0]` of every pixel in that column,
    /// ordered top-to-bottom.
    ///
    /// Only `yuv420p`, `yuv422p`, and `yuv444p` pixel formats are supported.
    /// Returns an empty [`Vec`] for unsupported formats or if Y-plane data is
    /// unavailable.
    #[must_use]
    pub fn waveform(frame: &VideoFrame) -> Vec<Vec<f32>> {
        match frame.format() {
            PixelFormat::Yuv420p | PixelFormat::Yuv422p | PixelFormat::Yuv444p => {}
            _ => return Vec::new(),
        }

        let Some(y_data) = frame.plane(0) else {
            return Vec::new();
        };
        let Some(stride) = frame.stride(0) else {
            return Vec::new();
        };

        let w = frame.width() as usize;
        let h = frame.height() as usize;
        let mut result = vec![Vec::with_capacity(h); w];

        for row in 0..h {
            for col in 0..w {
                let luma = f32::from(y_data[row * stride + col]) / 255.0;
                result[col].push(luma);
            }
        }

        result
    }

    /// Compute vectorscope data for `frame`.
    ///
    /// Returns a [`Vec`] of `(cb, cr)` pairs, one per chroma sample, with both
    /// values normalised to `[-0.5, 0.5]`.
    ///
    /// Chroma dimensions vary by format:
    /// - `yuv420p` — `(width/2) × (height/2)` samples
    /// - `yuv422p` — `(width/2) × height` samples
    /// - `yuv444p` — `width × height` samples
    ///
    /// Returns an empty [`Vec`] for unsupported formats or if chroma plane data
    /// is unavailable.
    #[must_use]
    pub fn vectorscope(frame: &VideoFrame) -> Vec<(f32, f32)> {
        let w = frame.width() as usize;
        let h = frame.height() as usize;

        let (cb_w, cb_h) = match frame.format() {
            PixelFormat::Yuv420p => (w.div_ceil(2), h.div_ceil(2)),
            PixelFormat::Yuv422p => (w.div_ceil(2), h),
            PixelFormat::Yuv444p => (w, h),
            _ => return Vec::new(),
        };

        let Some(u_plane) = frame.plane(1) else {
            return Vec::new();
        };
        let Some(v_plane) = frame.plane(2) else {
            return Vec::new();
        };
        let Some(u_stride) = frame.stride(1) else {
            return Vec::new();
        };
        let Some(v_stride) = frame.stride(2) else {
            return Vec::new();
        };

        let mut result = Vec::with_capacity(cb_w * cb_h);
        for row in 0..cb_h {
            for col in 0..cb_w {
                let cb = f32::from(u_plane[row * u_stride + col]) / 255.0 - 0.5;
                let cr = f32::from(v_plane[row * v_stride + col]) / 255.0 - 0.5;
                result.push((cb, cr));
            }
        }
        result
    }

    /// Compute RGB parade data for `frame`.
    ///
    /// Each pixel is converted from YUV to RGB using the BT.601 full-range matrix
    /// before sampling. Returns an [`RgbParade`] whose `r`, `g`, and `b` fields
    /// each have the same column-major shape as [`ScopeAnalyzer::waveform`].
    ///
    /// Only `yuv420p`, `yuv422p`, and `yuv444p` pixel formats are supported.
    /// Returns `RgbParade { r: vec![], g: vec![], b: vec![] }` for unsupported
    /// formats or if plane data is unavailable.
    #[must_use]
    pub fn rgb_parade(frame: &VideoFrame) -> RgbParade {
        let width = frame.width() as usize;
        let height = frame.height() as usize;
        let fmt = frame.format();

        match fmt {
            PixelFormat::Yuv420p | PixelFormat::Yuv422p | PixelFormat::Yuv444p => {}
            _ => {
                return RgbParade {
                    r: vec![],
                    g: vec![],
                    b: vec![],
                };
            }
        }

        let Some(luma) = frame.plane(0) else {
            return RgbParade {
                r: vec![],
                g: vec![],
                b: vec![],
            };
        };
        let Some(u_plane) = frame.plane(1) else {
            return RgbParade {
                r: vec![],
                g: vec![],
                b: vec![],
            };
        };
        let Some(v_plane) = frame.plane(2) else {
            return RgbParade {
                r: vec![],
                g: vec![],
                b: vec![],
            };
        };
        let Some(luma_stride) = frame.stride(0) else {
            return RgbParade {
                r: vec![],
                g: vec![],
                b: vec![],
            };
        };
        let Some(u_stride) = frame.stride(1) else {
            return RgbParade {
                r: vec![],
                g: vec![],
                b: vec![],
            };
        };
        let Some(v_stride) = frame.stride(2) else {
            return RgbParade {
                r: vec![],
                g: vec![],
                b: vec![],
            };
        };

        let mut red_cols = vec![Vec::with_capacity(height); width];
        let mut grn_cols = vec![Vec::with_capacity(height); width];
        let mut blu_cols = vec![Vec::with_capacity(height); width];

        for row in 0..height {
            for col in 0..width {
                let (chr_row, chr_col) = match fmt {
                    PixelFormat::Yuv420p => (row / 2, col / 2),
                    PixelFormat::Yuv422p => (row, col / 2),
                    _ => (row, col),
                };

                let yy = f32::from(luma[row * luma_stride + col]);
                let uu = f32::from(u_plane[chr_row * u_stride + chr_col]) - 128.0;
                let vv = f32::from(v_plane[chr_row * v_stride + chr_col]) - 128.0;

                let r = (yy + 1.402 * vv).clamp(0.0, 255.0) / 255.0;
                let g = (yy - 0.344 * uu - 0.714 * vv).clamp(0.0, 255.0) / 255.0;
                let b = (yy + 1.772 * uu).clamp(0.0, 255.0) / 255.0;

                red_cols[col].push(r);
                grn_cols[col].push(g);
                blu_cols[col].push(b);
            }
        }

        RgbParade {
            r: red_cols,
            g: grn_cols,
            b: blu_cols,
        }
    }

    /// Compute a 256-bin histogram for each channel and for luminance.
    ///
    /// For YUV frames luma is read directly from the Y plane; R, G, and B are
    /// computed via BT.601 full-range conversion. Bins are indexed by the raw
    /// 8-bit value `[0, 255]`.
    ///
    /// Only `yuv420p`, `yuv422p`, and `yuv444p` pixel formats are supported.
    /// Returns a zeroed [`Histogram`] for unsupported formats or if plane data
    /// is unavailable.
    #[must_use]
    pub fn histogram(frame: &VideoFrame) -> Histogram {
        let mut hist = Histogram {
            r: [0; 256],
            g: [0; 256],
            b: [0; 256],
            luma: [0; 256],
        };

        let width = frame.width() as usize;
        let height = frame.height() as usize;
        let fmt = frame.format();

        match fmt {
            PixelFormat::Yuv420p | PixelFormat::Yuv422p | PixelFormat::Yuv444p => {}
            _ => return hist,
        }

        let Some(luma_plane) = frame.plane(0) else {
            return hist;
        };
        let Some(u_plane) = frame.plane(1) else {
            return hist;
        };
        let Some(v_plane) = frame.plane(2) else {
            return hist;
        };
        let Some(luma_stride) = frame.stride(0) else {
            return hist;
        };
        let Some(u_stride) = frame.stride(1) else {
            return hist;
        };
        let Some(v_stride) = frame.stride(2) else {
            return hist;
        };

        for row in 0..height {
            for col in 0..width {
                let (chr_row, chr_col) = match fmt {
                    PixelFormat::Yuv420p => (row / 2, col / 2),
                    PixelFormat::Yuv422p => (row, col / 2),
                    _ => (row, col),
                };

                let y_px = luma_plane[row * luma_stride + col];
                let u_px = u_plane[chr_row * u_stride + chr_col];
                let v_px = v_plane[chr_row * v_stride + chr_col];

                hist.luma[usize::from(y_px)] += 1;

                // BT.601 full-range integer approximation (10-bit scaling).
                let yy_int = i32::from(y_px);
                let u_diff = i32::from(u_px) - 128;
                let v_diff = i32::from(v_px) - 128;

                let red_bin =
                    usize::try_from((yy_int + ((1436 * v_diff) >> 10)).clamp(0, 255)).unwrap_or(0);
                let grn_bin = usize::try_from(
                    (yy_int - ((352 * u_diff) >> 10) - ((731 * v_diff) >> 10)).clamp(0, 255),
                )
                .unwrap_or(0);
                let blu_bin =
                    usize::try_from((yy_int + ((1815 * u_diff) >> 10)).clamp(0, 255)).unwrap_or(0);

                hist.r[red_bin] += 1;
                hist.g[grn_bin] += 1;
                hist.b[blu_bin] += 1;
            }
        }

        hist
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

    fn make_yuv420p_frame(w: u32, h: u32, fill_y: u8) -> VideoFrame {
        let stride = w as usize;
        let uv_stride = (w as usize + 1) / 2;
        let uv_h = (h as usize + 1) / 2;
        VideoFrame::new(
            vec![
                PooledBuffer::standalone(vec![fill_y; stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; uv_stride * uv_h]),
                PooledBuffer::standalone(vec![128u8; uv_stride * uv_h]),
            ],
            vec![stride, uv_stride, uv_stride],
            w,
            h,
            PixelFormat::Yuv420p,
            Timestamp::default(),
            true,
        )
        .unwrap()
    }

    #[test]
    fn waveform_grey_frame_should_return_half_luma_values() {
        let frame = make_yuv420p_frame(4, 4, 128);
        let wf = ScopeAnalyzer::waveform(&frame);
        assert_eq!(wf.len(), 4, "result must have one inner Vec per column");
        for col in &wf {
            assert_eq!(col.len(), 4, "each column must have one value per row");
            for &v in col {
                let expected = 128.0 / 255.0;
                assert!(
                    (v - expected).abs() < 1e-6,
                    "grey Y=128 must map to {expected:.6}, got {v}"
                );
            }
        }
    }

    #[test]
    fn waveform_gradient_frame_should_have_increasing_column_means() {
        // Build a 4×4 frame where column c has Y = c * 64 (0, 64, 128, 192).
        let w = 4u32;
        let h = 4u32;
        let stride = w as usize;
        let uv_stride = (w as usize + 1) / 2;
        let uv_h = (h as usize + 1) / 2;
        let mut y_plane = vec![0u8; stride * h as usize];
        for row in 0..h as usize {
            for col in 0..w as usize {
                y_plane[row * stride + col] = (col as u8) * 64;
            }
        }
        let frame = VideoFrame::new(
            vec![
                PooledBuffer::standalone(y_plane),
                PooledBuffer::standalone(vec![128u8; uv_stride * uv_h]),
                PooledBuffer::standalone(vec![128u8; uv_stride * uv_h]),
            ],
            vec![stride, uv_stride, uv_stride],
            w,
            h,
            PixelFormat::Yuv420p,
            Timestamp::default(),
            true,
        )
        .unwrap();

        let wf = ScopeAnalyzer::waveform(&frame);
        assert_eq!(wf.len(), 4);
        let means: Vec<f32> = wf
            .iter()
            .map(|col| col.iter().sum::<f32>() / col.len() as f32)
            .collect();
        for i in 1..means.len() {
            assert!(
                means[i] > means[i - 1],
                "column means must increase left to right: {means:?}"
            );
        }
    }

    #[test]
    fn waveform_dimensions_should_match_frame_resolution() {
        let frame = make_yuv420p_frame(16, 8, 100);
        let wf = ScopeAnalyzer::waveform(&frame);
        assert_eq!(wf.len(), 16, "must have one Vec per column (width)");
        for col in &wf {
            assert_eq!(
                col.len(),
                8,
                "each column must have one value per row (height)"
            );
        }
    }

    #[test]
    fn waveform_unsupported_format_should_return_empty() {
        let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
        let wf = ScopeAnalyzer::waveform(&frame);
        assert!(
            wf.is_empty(),
            "unsupported pixel format must return empty Vec, got len={}",
            wf.len()
        );
    }

    #[test]
    fn waveform_yuv422p_should_be_supported() {
        let w = 4u32;
        let h = 4u32;
        let y_stride = w as usize;
        let uv_stride = (w as usize + 1) / 2;
        let frame = VideoFrame::new(
            vec![
                PooledBuffer::standalone(vec![200u8; y_stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; uv_stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; uv_stride * h as usize]),
            ],
            vec![y_stride, uv_stride, uv_stride],
            w,
            h,
            PixelFormat::Yuv422p,
            Timestamp::default(),
            true,
        )
        .unwrap();
        let wf = ScopeAnalyzer::waveform(&frame);
        assert_eq!(wf.len(), 4, "yuv422p must return result of length=width");
    }

    #[test]
    fn vectorscope_grey_frame_should_return_near_zero_pairs() {
        // U=V=128 → (128/255-0.5, 128/255-0.5) ≈ (0.00196, 0.00196)
        let frame = make_yuv420p_frame(4, 4, 128);
        let vs = ScopeAnalyzer::vectorscope(&frame);
        assert_eq!(vs.len(), 4, "yuv420p 4×4 → 2×2 chroma = 4 pairs");
        for &(cb, cr) in &vs {
            let expected = 128.0_f32 / 255.0 - 0.5;
            assert!(
                (cb - expected).abs() < 1e-6,
                "cb must be ≈{expected:.6}, got {cb}"
            );
            assert!(
                (cr - expected).abs() < 1e-6,
                "cr must be ≈{expected:.6}, got {cr}"
            );
        }
    }

    #[test]
    fn vectorscope_yuv420p_should_have_quarter_sample_count() {
        let frame = make_yuv420p_frame(8, 6, 100);
        let vs = ScopeAnalyzer::vectorscope(&frame);
        // chroma: (8+1)/2=4 × (6+1)/2=3 = 12
        assert_eq!(vs.len(), 12, "yuv420p 8×6 must produce 4×3=12 chroma pairs");
    }

    #[test]
    fn vectorscope_yuv422p_should_have_half_width_sample_count() {
        let w = 4u32;
        let h = 4u32;
        let y_stride = w as usize;
        let uv_stride = (w as usize + 1) / 2;
        let frame = VideoFrame::new(
            vec![
                PooledBuffer::standalone(vec![200u8; y_stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; uv_stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; uv_stride * h as usize]),
            ],
            vec![y_stride, uv_stride, uv_stride],
            w,
            h,
            PixelFormat::Yuv422p,
            Timestamp::default(),
            true,
        )
        .unwrap();
        let vs = ScopeAnalyzer::vectorscope(&frame);
        // chroma: 2×4 = 8 pairs
        assert_eq!(vs.len(), 8, "yuv422p 4×4 must produce 2×4=8 chroma pairs");
    }

    #[test]
    fn vectorscope_yuv444p_should_have_full_sample_count() {
        let w = 4u32;
        let h = 4u32;
        let stride = w as usize;
        let frame = VideoFrame::new(
            vec![
                PooledBuffer::standalone(vec![50u8; stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; stride * h as usize]),
            ],
            vec![stride, stride, stride],
            w,
            h,
            PixelFormat::Yuv444p,
            Timestamp::default(),
            true,
        )
        .unwrap();
        let vs = ScopeAnalyzer::vectorscope(&frame);
        assert_eq!(vs.len(), 16, "yuv444p 4×4 must produce 4×4=16 chroma pairs");
    }

    #[test]
    fn vectorscope_unsupported_format_should_return_empty() {
        let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
        let vs = ScopeAnalyzer::vectorscope(&frame);
        assert!(
            vs.is_empty(),
            "unsupported pixel format must return empty Vec, got len={}",
            vs.len()
        );
    }

    // YUV (full-range BT.601) values for a pure red frame (R=255,G=0,B=0):
    //   Y=76, Cb=85, Cr=255
    // Decoded: r≈(76+1.402*(255-128))/255≈0.996, g≈0.0, b≈0.0
    fn make_red_yuv420p_frame(w: u32, h: u32) -> VideoFrame {
        let stride = w as usize;
        let uv_stride = w.div_ceil(2) as usize;
        let uv_h = h.div_ceil(2) as usize;
        VideoFrame::new(
            vec![
                PooledBuffer::standalone(vec![76u8; stride * h as usize]),
                PooledBuffer::standalone(vec![85u8; uv_stride * uv_h]),
                PooledBuffer::standalone(vec![255u8; uv_stride * uv_h]),
            ],
            vec![stride, uv_stride, uv_stride],
            w,
            h,
            PixelFormat::Yuv420p,
            Timestamp::default(),
            true,
        )
        .unwrap()
    }

    #[test]
    fn rgb_parade_red_frame_should_have_high_r_and_low_g_b() {
        let frame = make_red_yuv420p_frame(4, 4);
        let parade = ScopeAnalyzer::rgb_parade(&frame);
        assert_eq!(parade.r.len(), 4, "r must have one Vec per column");
        assert_eq!(parade.g.len(), 4, "g must have one Vec per column");
        assert_eq!(parade.b.len(), 4, "b must have one Vec per column");
        for col in 0..4 {
            for &rv in &parade.r[col] {
                assert!(
                    rv > 0.9,
                    "red channel must be near 1.0 for red frame, got {rv}"
                );
            }
            for &gv in &parade.g[col] {
                assert!(
                    gv < 0.1,
                    "green channel must be near 0.0 for red frame, got {gv}"
                );
            }
            for &bv in &parade.b[col] {
                assert!(
                    bv < 0.1,
                    "blue channel must be near 0.0 for red frame, got {bv}"
                );
            }
        }
    }

    #[test]
    fn rgb_parade_white_frame_should_have_all_channels_at_one() {
        // Y=255, Cb=128, Cr=128 → R=G=B=1.0
        let frame = make_yuv420p_frame(4, 4, 255);
        let parade = ScopeAnalyzer::rgb_parade(&frame);
        for col in 0..4 {
            for (&rv, (&gv, &bv)) in parade.r[col]
                .iter()
                .zip(parade.g[col].iter().zip(parade.b[col].iter()))
            {
                assert!(
                    (rv - 1.0).abs() < 1e-5,
                    "r must be 1.0 for white frame, got {rv}"
                );
                assert!(
                    (gv - 1.0).abs() < 1e-5,
                    "g must be 1.0 for white frame, got {gv}"
                );
                assert!(
                    (bv - 1.0).abs() < 1e-5,
                    "b must be 1.0 for white frame, got {bv}"
                );
            }
        }
    }

    #[test]
    fn rgb_parade_unsupported_format_should_return_empty() {
        let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
        let parade = ScopeAnalyzer::rgb_parade(&frame);
        assert!(
            parade.r.is_empty() && parade.g.is_empty() && parade.b.is_empty(),
            "unsupported format must return empty parade"
        );
    }

    #[test]
    fn rgb_parade_dimensions_should_match_frame_resolution() {
        let frame = make_yuv420p_frame(8, 6, 100);
        let parade = ScopeAnalyzer::rgb_parade(&frame);
        assert_eq!(parade.r.len(), 8, "r must have width columns");
        for col in &parade.r {
            assert_eq!(col.len(), 6, "each column must have height rows");
        }
    }

    #[test]
    fn waveform_yuv444p_should_be_supported() {
        let w = 4u32;
        let h = 4u32;
        let stride = w as usize;
        let frame = VideoFrame::new(
            vec![
                PooledBuffer::standalone(vec![50u8; stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; stride * h as usize]),
                PooledBuffer::standalone(vec![128u8; stride * h as usize]),
            ],
            vec![stride, stride, stride],
            w,
            h,
            PixelFormat::Yuv444p,
            Timestamp::default(),
            true,
        )
        .unwrap();
        let wf = ScopeAnalyzer::waveform(&frame);
        assert_eq!(wf.len(), 4, "yuv444p must return result of length=width");
    }

    #[test]
    fn histogram_uniform_luma_should_concentrate_in_one_bin() {
        // Y=128, Cb=Cr=128 (grey) — luma bin 128 must hold all pixels.
        let frame = make_yuv420p_frame(4, 4, 128);
        let hist = ScopeAnalyzer::histogram(&frame);
        assert_eq!(
            hist.luma[128], 16,
            "all 16 pixels must land in luma bin 128"
        );
        let non_128: u32 = hist
            .luma
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != 128)
            .map(|(_, &v)| v)
            .sum();
        assert_eq!(non_128, 0, "all other luma bins must be zero");
    }

    #[test]
    fn histogram_total_luma_count_should_equal_pixel_count() {
        let frame = make_yuv420p_frame(8, 6, 200);
        let hist = ScopeAnalyzer::histogram(&frame);
        let total: u32 = hist.luma.iter().sum();
        assert_eq!(total, 8 * 6, "total luma bin counts must equal pixel count");
    }

    #[test]
    fn histogram_total_rgb_counts_should_equal_pixel_count() {
        let frame = make_yuv420p_frame(4, 4, 100);
        let hist = ScopeAnalyzer::histogram(&frame);
        let r_total: u32 = hist.r.iter().sum();
        let g_total: u32 = hist.g.iter().sum();
        let b_total: u32 = hist.b.iter().sum();
        assert_eq!(r_total, 16, "r bin counts must equal pixel count");
        assert_eq!(g_total, 16, "g bin counts must equal pixel count");
        assert_eq!(b_total, 16, "b bin counts must equal pixel count");
    }

    #[test]
    fn histogram_unsupported_format_should_return_zeroed() {
        let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).unwrap();
        let hist = ScopeAnalyzer::histogram(&frame);
        let all_zero = hist.luma.iter().all(|&v| v == 0)
            && hist.r.iter().all(|&v| v == 0)
            && hist.g.iter().all(|&v| v == 0)
            && hist.b.iter().all(|&v| v == 0);
        assert!(all_zero, "unsupported format must return zeroed histogram");
    }
}
