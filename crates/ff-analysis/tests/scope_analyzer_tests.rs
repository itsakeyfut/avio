//! Integration tests for `ScopeAnalyzer` — vectorscope and RGB parade.
//!
//! These are pure-Rust pixel arithmetic functions, so no FFmpeg call is
//! required.  Tests use synthetic `VideoFrame` objects to verify measurable,
//! predictable output values.

use ff_analysis::ScopeAnalyzer;
use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

// Helpers

fn yuv420p_frame(w: u32, h: u32, y: u8, u: u8, v: u8) -> VideoFrame {
    let yp = PooledBuffer::standalone(vec![y; (w * h) as usize]);
    let up = PooledBuffer::standalone(vec![u; ((w / 2) * (h / 2)) as usize]);
    let vp = PooledBuffer::standalone(vec![v; ((w / 2) * (h / 2)) as usize]);
    VideoFrame::new(
        vec![yp, up, vp],
        vec![w as usize, (w / 2) as usize, (w / 2) as usize],
        w,
        h,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("test yuv420p frame")
}

fn yuv444p_frame(w: u32, h: u32, y: u8, u: u8, v: u8) -> VideoFrame {
    let yp = PooledBuffer::standalone(vec![y; (w * h) as usize]);
    let up = PooledBuffer::standalone(vec![u; (w * h) as usize]);
    let vp = PooledBuffer::standalone(vec![v; (w * h) as usize]);
    VideoFrame::new(
        vec![yp, up, vp],
        vec![w as usize, w as usize, w as usize],
        w,
        h,
        PixelFormat::Yuv444p,
        Timestamp::default(),
        true,
    )
    .expect("test yuv444p frame")
}

// vectorscope

#[test]
fn vectorscope_neutral_yuv_frame_should_cluster_near_origin() {
    // U=128, V=128 → Cb=Cr=0 in centred coordinates → scatter near (0, 0).
    let frame = yuv420p_frame(8, 8, 128, 128, 128);
    let scatter = ScopeAnalyzer::vectorscope(&frame);
    assert!(
        !scatter.is_empty(),
        "vectorscope must return at least one point"
    );
    for (cb, cr) in &scatter {
        assert!(
            cb.abs() < 0.02,
            "neutral chroma Cb must be near 0; got {cb:.4}"
        );
        assert!(
            cr.abs() < 0.02,
            "neutral chroma Cr must be near 0; got {cr:.4}"
        );
    }
}

#[test]
fn vectorscope_yuv420p_sample_count_should_equal_quarter_of_pixel_count() {
    // 4:2:0 sub-sampling → chroma is one sample per 2×2 luma block.
    let frame = yuv420p_frame(8, 8, 128, 128, 128);
    let scatter = ScopeAnalyzer::vectorscope(&frame);
    assert_eq!(
        scatter.len(),
        8 * 8 / 4,
        "4:2:0 vectorscope must have w*h/4 samples"
    );
}

#[test]
fn vectorscope_yuv444p_sample_count_should_equal_pixel_count() {
    let frame = yuv444p_frame(6, 4, 128, 128, 128);
    let scatter = ScopeAnalyzer::vectorscope(&frame);
    assert_eq!(
        scatter.len(),
        6 * 4,
        "4:4:4 vectorscope must have exactly w*h samples"
    );
}

#[test]
fn vectorscope_all_values_should_be_in_normalised_range() {
    // Saturated blue: U=255 (max Cb), V=128 (neutral Cr)
    let frame = yuv444p_frame(4, 4, 100, 255, 128);
    let scatter = ScopeAnalyzer::vectorscope(&frame);
    for (cb, cr) in &scatter {
        assert!(
            *cb >= -1.0 && *cb <= 1.0,
            "Cb must be in [-1.0, 1.0]; got {cb:.4}"
        );
        assert!(
            *cr >= -1.0 && *cr <= 1.0,
            "Cr must be in [-1.0, 1.0]; got {cr:.4}"
        );
    }
}

#[test]
fn vectorscope_unsupported_format_should_return_empty() {
    let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).expect("test rgba frame");
    let scatter = ScopeAnalyzer::vectorscope(&frame);
    assert!(
        scatter.is_empty(),
        "vectorscope must return empty for RGBA input"
    );
}

// rgb_parade

/// `rgb_parade` only supports YUV formats. A neutral grey YUV frame
/// (U=128, V=128) should produce equal R, G, B averages per column.
#[test]
fn rgb_parade_neutral_yuv_frame_should_have_equal_r_g_b_columns() {
    // U=128, V=128 = neutral chroma → R≈G≈B
    let frame = yuv420p_frame(8, 8, 128, 128, 128);
    let parade = ScopeAnalyzer::rgb_parade(&frame);

    assert_eq!(parade.r.len(), 8, "parade.r must have one vec per column");
    assert_eq!(parade.g.len(), 8, "parade.g must have one vec per column");
    assert_eq!(parade.b.len(), 8, "parade.b must have one vec per column");

    for col in 0..8 {
        let avg_r: f32 = parade.r[col].iter().sum::<f32>() / parade.r[col].len() as f32;
        let avg_g: f32 = parade.g[col].iter().sum::<f32>() / parade.g[col].len() as f32;
        let avg_b: f32 = parade.b[col].iter().sum::<f32>() / parade.b[col].len() as f32;
        let diff_rg = (avg_r - avg_g).abs();
        let diff_gb = (avg_g - avg_b).abs();
        assert!(
            diff_rg < 0.05,
            "neutral chroma column {col}: R and G must be close; diff={diff_rg:.4}"
        );
        assert!(
            diff_gb < 0.05,
            "neutral chroma column {col}: G and B must be close; diff={diff_gb:.4}"
        );
    }
}

/// A very bright YUV frame (Y near 255) should produce high values in all
/// channels because neutral chroma means R≈G≈B≈Y.
#[test]
fn rgb_parade_bright_yuv_frame_should_have_high_luma_across_channels() {
    let frame = yuv420p_frame(4, 4, 235, 128, 128); // BT.601 white
    let parade = ScopeAnalyzer::rgb_parade(&frame);

    assert!(
        !parade.r.is_empty(),
        "bright frame must produce non-empty R parade"
    );
    let avg_r: f32 = parade.r[0].iter().sum::<f32>() / parade.r[0].len() as f32;
    assert!(
        avg_r > 0.8,
        "Y=235, neutral chroma must produce R≈1.0; got {avg_r:.4}"
    );
}

/// Unsupported formats (e.g. RGBA) must return empty parade.
#[test]
fn rgb_parade_unsupported_format_should_return_empty() {
    let frame = VideoFrame::empty(4, 4, PixelFormat::Rgba).expect("rgba frame");
    let parade = ScopeAnalyzer::rgb_parade(&frame);
    assert!(
        parade.r.is_empty() && parade.g.is_empty() && parade.b.is_empty(),
        "rgb_parade must return empty vecs for unsupported RGBA input"
    );
}

#[test]
fn rgb_parade_yuv420p_frame_should_return_nonempty_vectors() {
    let frame = yuv420p_frame(8, 8, 128, 128, 128);
    let parade = ScopeAnalyzer::rgb_parade(&frame);
    assert_eq!(parade.r.len(), 8, "YUV parade must have one vec per column");
    for col in 0..8 {
        assert!(
            !parade.r[col].is_empty(),
            "column {col}: R must have at least one sample"
        );
    }
}

#[test]
fn rgb_parade_column_values_should_be_in_normalised_range() {
    let frame = yuv420p_frame(4, 4, 200, 100, 150);
    let parade = ScopeAnalyzer::rgb_parade(&frame);
    for col in 0..4 {
        for &v in &parade.r[col] {
            assert!(v >= 0.0 && v <= 1.0, "R value must be in [0,1]; got {v:.4}");
        }
        for &v in &parade.g[col] {
            assert!(v >= 0.0 && v <= 1.0, "G value must be in [0,1]; got {v:.4}");
        }
        for &v in &parade.b[col] {
            assert!(v >= 0.0 && v <= 1.0, "B value must be in [0,1]; got {v:.4}");
        }
    }
}
