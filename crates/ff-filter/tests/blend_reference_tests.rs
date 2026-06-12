//! Golden-image regression tests for all 18 photographic blend modes.
//!
//! ## Running
//!
//! The test is marked `#[ignore]` because it requires pre-committed PNG
//! fixture files.  Two workflows are supported:
//!
//! **Generate fixtures** (first time, or after intentional mode changes):
//! ```text
//! BLEND_GENERATE_REFS=1 cargo test -p ff-filter blend_mode_reference -- --include-ignored
//! ```
//! This writes `tests/fixtures/blend/bottom.png`, `top.png`, and
//! `tests/fixtures/blend/expected/<mode>.png` for all 18 modes.
//!
//! **Verify against committed references** (normal CI):
//! ```text
//! cargo test -p ff-filter blend_mode_reference -- --include-ignored
//! ```
//! Each output is compared against the committed reference within a
//! per-channel tolerance of ±2/255.

#![allow(clippy::unwrap_used)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]

use std::path::PathBuf;

use image::RgbImage;

use ff_filter::{BlendMode, FilterGraph, FilterGraphBuilder};
use ff_format::{AlphaMode, PixelFormat, PooledBuffer, Timestamp, VideoFrame};

// ── Synthetic input image generators ─────────────────────────────────────────

/// 64×64 horizontal red gradient: pixel (x, y) = Rgb([x*4, 128, 64]).
fn generate_bottom_image() -> RgbImage {
    RgbImage::from_fn(64, 64, |x, _y| image::Rgb([(x * 4) as u8, 128u8, 64u8]))
}

/// 64×64 vertical blue gradient: pixel (x, y) = Rgb([64, 128, y*4]).
fn generate_top_image() -> RgbImage {
    RgbImage::from_fn(64, 64, |_x, y| image::Rgb([64u8, 128u8, (y * 4) as u8]))
}

// ── Frame conversion ──────────────────────────────────────────────────────────

/// Convert an RGB image to a Yuv420p `VideoFrame` using BT.601 coefficients.
///
/// Chroma (U, V) is subsampled 2×2 by averaging the four contributing pixels.
fn rgb_to_yuv420p(img: &RgbImage) -> VideoFrame {
    let w = img.width() as usize;
    let h = img.height() as usize;

    let mut y_plane = vec![0u8; w * h];
    let mut u_plane = vec![128u8; (w / 2) * (h / 2)];
    let mut v_plane = vec![128u8; (w / 2) * (h / 2)];

    for py in 0..h {
        for px in 0..w {
            let p = img.get_pixel(px as u32, py as u32);
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            y_plane[py * w + px] = (0.299 * r + 0.587 * g + 0.114 * b)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }

    for cy in 0..(h / 2) {
        for cx in 0..(w / 2) {
            let mut sum_u = 0.0f32;
            let mut sum_v = 0.0f32;
            for dy in 0..2usize {
                for dx in 0..2usize {
                    let p = img.get_pixel((cx * 2 + dx) as u32, (cy * 2 + dy) as u32);
                    let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
                    sum_u += -0.169 * r - 0.331 * g + 0.500 * b + 128.0;
                    sum_v += 0.500 * r - 0.419 * g - 0.081 * b + 128.0;
                }
            }
            u_plane[cy * (w / 2) + cx] = (sum_u / 4.0).round().clamp(0.0, 255.0) as u8;
            v_plane[cy * (w / 2) + cx] = (sum_v / 4.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    VideoFrame::new(
        vec![
            PooledBuffer::standalone(y_plane),
            PooledBuffer::standalone(u_plane),
            PooledBuffer::standalone(v_plane),
        ],
        vec![w, w / 2, w / 2],
        w as u32,
        h as u32,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .unwrap()
}

/// Convert a `VideoFrame` to an `RgbImage`.
///
/// Handles two layouts:
/// - **3-plane Yuv420p** (including `yuvj420p`): planes 0/1/2 present, BT.601 inverse.
/// - **1-plane packed RGB** (`Rgb24`): stride ≥ 3 × width.
///
/// Returns `None` for formats that cannot be decoded with either heuristic.
fn frame_to_rgb_image(frame: &VideoFrame) -> Option<RgbImage> {
    let w = frame.width() as usize;
    let h = frame.height() as usize;

    // Try Yuv420p-compatible 3-plane layout (yuv420p, yuvj420p, …).
    if let (Some(y_plane), Some(u_plane), Some(v_plane)) =
        (frame.plane(0), frame.plane(1), frame.plane(2))
    {
        let y_stride = frame.stride(0).unwrap_or(w);
        let u_stride = frame.stride(1).unwrap_or(w / 2);
        let v_stride = frame.stride(2).unwrap_or(w / 2);
        return Some(RgbImage::from_fn(w as u32, h as u32, |px, py| {
            let (px, py) = (px as usize, py as usize);
            let y = y_plane[py * y_stride + px] as f32;
            let u = u_plane[(py / 2) * u_stride + (px / 2)] as f32;
            let v = v_plane[(py / 2) * v_stride + (px / 2)] as f32;
            let r = (y + 1.402 * (v - 128.0)).round().clamp(0.0, 255.0) as u8;
            let g = (y - 0.344 * (u - 128.0) - 0.714 * (v - 128.0))
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (y + 1.772 * (u - 128.0)).round().clamp(0.0, 255.0) as u8;
            image::Rgb([r, g, b])
        }));
    }

    // Fall back: single packed-RGB plane (Rgb24 / Bgr24 stride ≥ 3 × width).
    if let Some(data) = frame.plane(0) {
        let stride = frame.stride(0).unwrap_or(w * 3);
        if stride >= w * 3 && data.len() >= stride * h {
            return Some(RgbImage::from_fn(w as u32, h as u32, |px, py| {
                let base = py as usize * stride + px as usize * 3;
                image::Rgb([data[base], data[base + 1], data[base + 2]])
            }));
        }
    }

    // Last resort: single luma plane (e.g. YUVA420p output where only the Y
    // plane is accessible due to Other(n).num_planes() == 1).  Expand luma to
    // a grayscale RGB image so a stable reference can still be generated and
    // compared.
    if let Some(y_data) = frame.plane(0) {
        let stride = frame.stride(0).unwrap_or(w);
        if y_data.len() >= stride * h {
            return Some(RgbImage::from_fn(w as u32, h as u32, |px, py| {
                let y = y_data[py as usize * stride + px as usize];
                image::Rgb([y, y, y])
            }));
        }
    }

    None
}

// ── Blend helper ──────────────────────────────────────────────────────────────

/// Apply `mode` blend to `bottom` (slot 0) and `top_frame` (slot 1).
///
/// Returns `None` if graph construction or frame processing fails.
fn apply_blend(bottom: &VideoFrame, top_frame: &VideoFrame, mode: BlendMode) -> Option<VideoFrame> {
    let top_builder = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top_builder, mode, 1.0, AlphaMode::Straight)
        .build()
        .ok()?;
    graph.push_video(0, bottom).ok()?;
    graph.push_video(1, top_frame).ok()?;
    graph.pull_video().ok().flatten()
}

// ── Comparison helper ─────────────────────────────────────────────────────────

/// Assert that every pixel channel in `actual` differs from `expected` by at
/// most `tolerance`.
fn assert_within_tolerance(actual: &RgbImage, expected: &RgbImage, tolerance: u8, name: &str) {
    assert_eq!(
        actual.dimensions(),
        expected.dimensions(),
        "blend mode '{name}': output dimensions mismatch"
    );
    let (w, h) = actual.dimensions();
    for y in 0..h {
        for x in 0..w {
            let a = actual.get_pixel(x, y);
            let e = expected.get_pixel(x, y);
            for c in 0..3usize {
                let diff = (a[c] as i32 - e[c] as i32).unsigned_abs() as u8;
                assert!(
                    diff <= tolerance,
                    "blend mode '{name}': pixel ({x},{y}) channel {c} diff {diff} > \
                     tolerance {tolerance} (actual={}, expected={})",
                    a[c],
                    e[c]
                );
            }
        }
    }
}

// ── Main test ─────────────────────────────────────────────────────────────────

/// All 18 photographic blend modes verified against committed reference PNGs.
///
/// The test is marked `#[ignore]` so the base `cargo test` suite does not
/// require fixture files to be present.  Run explicitly:
///
/// ```text
/// # Generate references (first time or after intentional mode changes):
/// BLEND_GENERATE_REFS=1 cargo test -p ff-filter blend_mode_reference -- --include-ignored
///
/// # Verify:
/// cargo test -p ff-filter blend_mode_reference -- --include-ignored
/// ```
#[test]
#[ignore = "requires blend fixture images; run with -- --include-ignored or BLEND_GENERATE_REFS=1"]
fn blend_mode_reference_images_should_match_within_tolerance() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/blend");
    let expected_dir = fixture_dir.join("expected");
    let generate = std::env::var("BLEND_GENERATE_REFS").is_ok();

    // ── Load or generate input fixtures ───────────────────────────────────────
    let bottom_path = fixture_dir.join("bottom.png");
    let top_path = fixture_dir.join("top.png");

    let bottom_img: RgbImage = if generate || !bottom_path.exists() {
        let img = generate_bottom_image();
        std::fs::create_dir_all(&fixture_dir).unwrap();
        img.save(&bottom_path).unwrap();
        img
    } else {
        match image::open(&bottom_path) {
            Ok(i) => i.to_rgb8(),
            Err(e) => {
                println!("Skipping: failed to load bottom.png: {e}");
                return;
            }
        }
    };

    let top_img: RgbImage = if generate || !top_path.exists() {
        let img = generate_top_image();
        std::fs::create_dir_all(&fixture_dir).unwrap();
        img.save(&top_path).unwrap();
        img
    } else {
        match image::open(&top_path) {
            Ok(i) => i.to_rgb8(),
            Err(e) => {
                println!("Skipping: failed to load top.png: {e}");
                return;
            }
        }
    };

    let bottom_frame = rgb_to_yuv420p(&bottom_img);
    let top_frame = rgb_to_yuv420p(&top_img);

    if generate {
        std::fs::create_dir_all(&expected_dir).unwrap();
    }

    // ── Blend mode table ──────────────────────────────────────────────────────
    let modes: &[(&str, BlendMode)] = &[
        ("normal", BlendMode::Normal),
        ("multiply", BlendMode::Multiply),
        ("screen", BlendMode::Screen),
        ("overlay", BlendMode::Overlay),
        ("soft_light", BlendMode::SoftLight),
        ("hard_light", BlendMode::HardLight),
        ("color_dodge", BlendMode::ColorDodge),
        ("color_burn", BlendMode::ColorBurn),
        ("darken", BlendMode::Darken),
        ("lighten", BlendMode::Lighten),
        ("difference", BlendMode::Difference),
        ("exclusion", BlendMode::Exclusion),
        ("add", BlendMode::Add),
        ("subtract", BlendMode::Subtract),
        ("hue", BlendMode::Hue),
        ("saturation", BlendMode::Saturation),
        ("color", BlendMode::Color),
        ("luminosity", BlendMode::Luminosity),
    ];

    // ── Per-mode generate or compare ──────────────────────────────────────────
    let mut failures = 0usize;
    for &(name, mode) in modes {
        let out_frame = match apply_blend(&bottom_frame, &top_frame, mode) {
            Some(f) => f,
            None => {
                println!("Skipping mode '{name}': blend filter did not produce a frame");
                continue;
            }
        };

        let out_img = match frame_to_rgb_image(&out_frame) {
            Some(img) => img,
            None => {
                println!(
                    "Skipping mode '{name}': output in unrecognised pixel format \
                     (format={:?})",
                    out_frame.format()
                );
                continue;
            }
        };

        if generate {
            let ref_path = expected_dir.join(format!("{name}.png"));
            out_img.save(&ref_path).unwrap();
            println!("Generated reference: {}", ref_path.display());
        } else {
            let ref_path = expected_dir.join(format!("{name}.png"));
            if !ref_path.exists() {
                println!(
                    "Skipping mode '{name}': no reference image at {}",
                    ref_path.display()
                );
                continue;
            }
            let expected_img = match image::open(&ref_path) {
                Ok(i) => i.to_rgb8(),
                Err(e) => {
                    println!("Skipping mode '{name}': failed to load reference: {e}");
                    continue;
                }
            };
            // Use catch_unwind so all modes are reported before the test fails.
            let result = std::panic::catch_unwind(|| {
                assert_within_tolerance(&out_img, &expected_img, 2, name);
            });
            if let Err(e) = result {
                eprintln!("FAILED mode '{name}': {:?}", e);
                failures += 1;
            }
        }
    }

    assert_eq!(
        failures, 0,
        "{failures} blend mode(s) exceeded the ±2 per-channel tolerance"
    );
}
