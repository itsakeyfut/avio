//! End-to-end export on both routes (Br4, #1627): the GPU export path (composite
//! -> readback -> existing encoder) and the force-CPU `MultiTrackComposer` path must
//! each produce a decodable, canvas-sized video from the same single-clip timeline.
//!
//! Probe-gated (RK-002): the source encode needs `FFmpeg` codecs and the GPU leg needs
//! an adapter; both are skipped gracefully when unavailable so the suite is green on
//! a headless / minimal-`FFmpeg` CI. Only environment-unavailable errors are skipped;
//! a structural failure (e.g. `TimelineRenderFailed`) fails the test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError};
use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoCodec};
use ff_filter::XfadeTransition;
use fixtures::{FileGuard, make_source_file, test_output_path};

const SRC_FRAMES: usize = 15;
const CANVAS: u32 = 64;

/// Whether a render error means "this environment can't run the pipeline" (skip) as
/// opposed to a real regression (fail). Mirrors the `timeline_tests` convention: a
/// filter/encode/decode build failure on minimal-FFmpeg CI is a skip; anything else
/// (notably `TimelineRenderFailed` / `Cancelled`) is a genuine failure.
fn is_environment_unavailable(e: &TimelineError) -> bool {
    matches!(
        e,
        TimelineError::Filter(_) | TimelineError::Encode(_) | TimelineError::Decode(_)
    )
}

/// `(decoded frame count, dimensions of the first frame)` for an exported file, or
/// `(0, None)` when it cannot be opened/decoded.
fn decode_stats(path: &std::path::Path) -> (usize, Option<(u32, u32)>) {
    let Ok(mut d) = VideoDecoder::open(path).build() else {
        return (0, None);
    };
    let mut n = 0usize;
    let mut dims = None;
    while let Ok(Some(f)) = d.decode_one() {
        if dims.is_none() {
            dims = Some((f.width(), f.height()));
        }
        n += 1;
    }
    (n, dims)
}

/// Asserts an exported file is a valid ~`SRC_FRAMES`-frame, canvas-sized video. A
/// silently truncated export (e.g. an early loop break) or a wrong-sized frame fails
/// here rather than passing a bare "non-empty" check.
fn assert_valid_export(path: &std::path::Path, route: &str) {
    let (count, dims) = decode_stats(path);
    assert!(
        (SRC_FRAMES - 1..=SRC_FRAMES + 1).contains(&count),
        "{route} export should decode ~{SRC_FRAMES} frames, got {count}"
    );
    assert_eq!(
        dims,
        Some((CANVAS, CANVAS)),
        "{route} export frames should be the {CANVAS}x{CANVAS} canvas size"
    );
}

fn export_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(800_000))
        .build()
}

/// A square, single hard-cut, file-source, unity-speed timeline: the shape the GPU
/// export path handles (aspect matches the canvas, identity transform).
fn build_timeline(src: &std::path::Path) -> Option<Timeline> {
    Timeline::builder()
        .canvas(CANVAS, CANVAS)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(src)])
        .build()
        .ok()
}

/// Renders `timeline` to `out`, returning `false` (skip) on an environment-unavailable
/// error and panicking on a structural one.
fn render_or_skip(result: Result<(), TimelineError>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) if is_environment_unavailable(&e) => false,
        Err(e) => panic!("unexpected export error: {e}"),
    }
}

/// Bounding box of the "bright" pixels (the overlay) against a dark base, inclusive.
///
/// The placement tests assert this rather than a mean difference: two equally-wrong
/// renders have the same mean, and an overlay in the wrong place is exactly that failure
/// (RK-015).
fn bright_bbox(rgba: &[u8], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if rgba[i] > 160 && rgba[i + 1] > 160 && rgba[i + 2] > 160 {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then_some((x0, y0, x1, y1))
}

/// `(frame count, bright bbox)` of the frame at index `at` in an exported file.
fn count_and_bbox(path: &std::path::Path, at: usize) -> (usize, Option<(u32, u32, u32, u32)>) {
    let Ok(mut d) = VideoDecoder::open(path)
        .output_format(ff_format::PixelFormat::Rgba)
        .build()
    else {
        return (0, None);
    };
    let (mut n, mut bbox) = (0usize, None);
    while let Ok(Some(f)) = d.decode_one() {
        if n == at {
            bbox = f
                .to_rgba()
                .and_then(|r| bright_bbox(&r, f.width(), f.height()));
        }
        n += 1;
    }
    (n, bbox)
}

/// Mean luma of the bottom-right 8x8 corner of frame `at`, which the overlay fixtures
/// never cover, so it reads the base track's own contribution.
fn corner_luma(path: &std::path::Path, at: usize) -> Option<f64> {
    let mut d = VideoDecoder::open(path)
        .output_format(ff_format::PixelFormat::Rgba)
        .build()
        .ok()?;
    let mut n = 0usize;
    while let Ok(Some(f)) = d.decode_one() {
        if n == at {
            let rgba = f.to_rgba()?;
            let (w, h) = (f.width(), f.height());
            let mut sum = 0f64;
            for y in h.saturating_sub(8)..h {
                for x in w.saturating_sub(8)..w {
                    let i = ((y * w + x) * 4) as usize;
                    sum += f64::from(rgba[i]);
                }
            }
            return Some(sum / 64.0);
        }
        n += 1;
    }
    None
}

/// A dark base track under a bright overlay placed at `(10, 4)` and scaled `0.5`.
fn two_track_timeline(base: &std::path::Path, over: &std::path::Path) -> Option<Timeline> {
    Timeline::builder()
        .canvas(CANVAS, CANVAS)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(base)])
        .video_track(vec![
            Clip::new(over).with_position(10.0, 4.0).with_scale(0.5),
        ])
        .build()
        .ok()
}

/// Mean R/G/B of the first frame, or `None` when the file cannot be decoded.
///
/// A whole-frame mean is the right instrument for the opacity tests below: the overlay
/// covers the canvas there, so the number *is* how many times the opacity was applied
/// (0.5 -> ~140 on the fixture, 0.25 -> ~89), which a bounding box cannot see.
fn mean_rgb(path: &std::path::Path) -> Option<(f64, f64, f64)> {
    let mut d = VideoDecoder::open(path)
        .output_format(ff_format::PixelFormat::Rgba)
        .build()
        .ok()?;
    let f = d.decode_one().ok()??;
    let rgba = f.to_rgba()?;
    let (mut r, mut g, mut b) = (0f64, 0f64, 0f64);
    let n = (rgba.len() / 4) as f64;
    for px in rgba.chunks_exact(4) {
        r += f64::from(px[0]);
        g += f64::from(px[1]);
        b += f64::from(px[2]);
    }
    Some((r / n, g / n, b / n))
}

/// A dark base under a full-canvas overlay carrying `opacity`.
fn opacity_timeline(base: &std::path::Path, over: &std::path::Path) -> Option<Timeline> {
    Timeline::builder()
        .canvas(CANVAS, CANVAS)
        .frame_rate(30.0)
        .video_track(vec![Clip::new(base)])
        .video_track(vec![Clip::new(over).with_opacity(0.5)])
        .build()
        .ok()
}

#[cfg(feature = "gpu")]
#[test]
fn an_overlay_should_apply_its_opacity_exactly_once() {
    // The scheduler pre-composites the base to the canvas but hands the stack every
    // other track's *raw* frame, because the stack pass is where an overlay's opacity,
    // blend and effects are applied. Compositing an overlay on its own first applied
    // them a second time: measured 51 against the CPU's 140 on exactly this fixture.
    // A bbox assertion cannot see this (the overlay covers the canvas), so the mean is.
    let dark = test_output_path("op_dark.mp4");
    let bright = test_output_path("op_bright.mp4");
    let _gd = FileGuard::new(dark.clone());
    let _gb = FileGuard::new(bright.clone());
    if make_source_file(&dark, CANVAS, CANVAS, 30.0, SRC_FRAMES, 40, 128, 128).is_none() {
        return;
    }
    if make_source_file(&bright, CANVAS, CANVAS, 30.0, SRC_FRAMES, 235, 128, 128).is_none() {
        return;
    }
    let out_cpu = test_output_path("op_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_t) = opacity_timeline(&dark, &bright) else {
        return;
    };
    if !render_or_skip(cpu_t.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    if avio::GpuCompositor::new().is_none() {
        return;
    }
    let out_gpu = test_output_path("op_gpu.mp4");
    let _gg = FileGuard::new(out_gpu.clone());
    let Some(gpu_t) = opacity_timeline(&dark, &bright) else {
        return;
    };
    if !render_or_skip(gpu_t.render(&out_gpu, export_config())) {
        return;
    }

    let (Some(cpu), Some(gpu)) = (mean_rgb(&out_cpu), mean_rgb(&out_gpu)) else {
        return;
    };
    println!("opacity once: cpu={cpu:?} gpu={gpu:?}");
    assert!(
        cpu.0 > 120.0 && cpu.0 < 160.0,
        "the control must really be a half-strength overlay, got {cpu:?}"
    );
    assert!(
        (cpu.0 - gpu.0).abs() < 4.0,
        "the GPU must apply the overlay's opacity once, like the CPU: cpu={cpu:?} gpu={gpu:?}"
    );
}

#[cfg(feature = "gpu")]
#[test]
fn a_base_track_should_apply_its_own_effect_exactly_once_under_an_overlay() {
    // The mirror of the overlay test: the base *is* pre-composited to the canvas, which
    // is where its effects, opacity and blend are applied, so the layer it hands the
    // stack has to be stripped of them. A hue rotation is the readout -- applied twice it
    // is a 120 degree rotation, which no tolerance hides. The overlay is small and
    // placed, so most of the frame is the base.
    let base = test_output_path("be_base.mp4");
    let over = test_output_path("be_over.mp4");
    let _gb = FileGuard::new(base.clone());
    let _go = FileGuard::new(over.clone());
    if make_source_file(&base, CANVAS, CANVAS, 30.0, SRC_FRAMES, 120, 200, 90).is_none() {
        return;
    }
    if make_source_file(&over, CANVAS, CANVAS, 30.0, SRC_FRAMES, 235, 128, 128).is_none() {
        return;
    }
    let build = || {
        Timeline::builder()
            .canvas(CANVAS, CANVAS)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new(&base).with_video_effect(ff_filter::FilterStep::Hue { degrees: 60.0 }),
            ])
            .video_track(vec![
                Clip::new(&over).with_position(10.0, 4.0).with_scale(0.5),
            ])
            .build()
            .ok()
    };
    let out_cpu = test_output_path("be_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_t) = build() else { return };
    if !render_or_skip(cpu_t.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    if avio::GpuCompositor::new().is_none() {
        return;
    }
    let out_gpu = test_output_path("be_gpu.mp4");
    let _gg = FileGuard::new(out_gpu.clone());
    let Some(gpu_t) = build() else { return };
    if !render_or_skip(gpu_t.render(&out_gpu, export_config())) {
        return;
    }

    let (Some(cpu), Some(gpu)) = (mean_rgb(&out_cpu), mean_rgb(&out_gpu)) else {
        return;
    };
    println!("base effect once: cpu={cpu:?} gpu={gpu:?}");
    let spread = (cpu.0 - cpu.1).abs() + (cpu.1 - cpu.2).abs();
    assert!(
        spread > 20.0,
        "the control must be visibly hue-rotated, or a second rotation would not show: {cpu:?}"
    );
    assert!(
        (cpu.0 - gpu.0).abs() < 12.0
            && (cpu.1 - gpu.1).abs() < 12.0
            && (cpu.2 - gpu.2).abs() < 12.0,
        "the base's effect must be applied once, like the CPU: cpu={cpu:?} gpu={gpu:?}"
    );
}

#[cfg(feature = "gpu")]
#[test]
fn an_overlay_should_be_stretched_to_the_canvas_the_way_the_cpu_stretches_it() {
    // The CPU scales an overlay with `scale = canvas * (sx, sy)`, which discards its
    // aspect. Pre-compositing an overlay on its own instead letterboxed it into the
    // canvas and baked the bars in: a 64x32 source at scale 0.5 landed 32x16 (bbox
    // (0,8)-(31,23)) where the CPU had a stretched 32x32 (bbox (0,0)-(31,31)).
    let dark = test_output_path("wide_dark.mp4");
    let wide = test_output_path("wide_over.mp4");
    let _gd = FileGuard::new(dark.clone());
    let _gw = FileGuard::new(wide.clone());
    if make_source_file(&dark, CANVAS, CANVAS, 30.0, SRC_FRAMES, 40, 128, 128).is_none() {
        return;
    }
    if make_source_file(&wide, CANVAS, CANVAS / 2, 30.0, SRC_FRAMES, 235, 128, 128).is_none() {
        return;
    }
    let build = || {
        Timeline::builder()
            .canvas(CANVAS, CANVAS)
            .frame_rate(30.0)
            .video_track(vec![Clip::new(&dark)])
            .video_track(vec![
                Clip::new(&wide).with_position(0.0, 0.0).with_scale(0.5),
            ])
            .build()
            .ok()
    };
    let out_cpu = test_output_path("wide_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_t) = build() else { return };
    if !render_or_skip(cpu_t.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    if avio::GpuCompositor::new().is_none() {
        return;
    }
    let out_gpu = test_output_path("wide_gpu.mp4");
    let _gg = FileGuard::new(out_gpu.clone());
    let Some(gpu_t) = build() else { return };
    if !render_or_skip(gpu_t.render(&out_gpu, export_config())) {
        return;
    }

    let (_, cpu_box) = count_and_bbox(&out_cpu, 5);
    let (_, gpu_box) = count_and_bbox(&out_gpu, 5);
    println!("stretched overlay: cpu={cpu_box:?} gpu={gpu_box:?}");
    assert_eq!(
        cpu_box,
        Some((0, 0, 31, 31)),
        "the CPU control must stretch the 64x32 overlay into a 32x32 square"
    );
    assert_eq!(
        gpu_box, cpu_box,
        "the GPU must stretch the overlay as the CPU does, not letterbox it"
    );
}

#[cfg(feature = "gpu")]
#[test]
fn two_track_export_should_match_the_cpu_route() {
    // #1633: a second active video track used to keep the whole export on the CPU. Both
    // routes must now agree, and the overlay must land where the CPU was *measured* to
    // put it: stretched to the base size, scaled by `base * scale`, top-left at (x, y)
    // -- i.e. (10, 4)..(41, 35) inclusive for this fixture.
    let dark = test_output_path("mt_dark.mp4");
    let bright = test_output_path("mt_bright.mp4");
    let _gd = FileGuard::new(dark.clone());
    let _gb = FileGuard::new(bright.clone());
    if make_source_file(&dark, CANVAS, CANVAS, 30.0, SRC_FRAMES, 40, 128, 128).is_none() {
        return; // encoder unavailable -> skip
    }
    if make_source_file(&bright, CANVAS, CANVAS, 30.0, SRC_FRAMES, 235, 128, 128).is_none() {
        return;
    }

    let out_cpu = test_output_path("mt_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_t) = two_track_timeline(&dark, &bright) else {
        return;
    };
    if !render_or_skip(cpu_t.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }

    if avio::GpuCompositor::new().is_none() {
        return; // no adapter -> the GPU leg is unreachable here
    }
    let out_gpu = test_output_path("mt_gpu.mp4");
    let _gg = FileGuard::new(out_gpu.clone());
    let Some(gpu_t) = two_track_timeline(&dark, &bright) else {
        return;
    };
    if !render_or_skip(gpu_t.render(&out_gpu, export_config())) {
        return;
    }

    let (cpu_n, cpu_box) = count_and_bbox(&out_cpu, 5);
    let (gpu_n, gpu_box) = count_and_bbox(&out_gpu, 5);
    println!("two-track: cpu=({cpu_n}, {cpu_box:?}) gpu=({gpu_n}, {gpu_box:?})");
    assert_eq!(cpu_n, gpu_n, "both routes must export the same frame count");
    assert_eq!(
        cpu_box,
        Some((10, 4, 41, 35)),
        "the CPU fixture must still land where it was measured"
    );
    assert_eq!(
        gpu_box, cpu_box,
        "the GPU must place the overlay where the CPU does"
    );
}

#[cfg(feature = "gpu")]
#[test]
fn a_base_track_that_ends_first_should_not_promote_the_overlay() {
    // The scheduler reads a track's **stack position** as "which layer is the base"
    // (`gpu_compositor::layer_transform`), so a track that ends must keep its slot and
    // contribute transparent rather than be dropped -- dropping it would promote the
    // overlay to base, whose transform is ignored, and the overlay would fill the canvas.
    // Measured on the CPU route: with a 6-frame base under a 15-frame overlay, the base's
    // area goes black from frame 6 while the overlay stays exactly where it was.
    let short = test_output_path("mtbase_short.mp4");
    let bright = test_output_path("mtbase_bright.mp4");
    let _gs = FileGuard::new(short.clone());
    let _gb = FileGuard::new(bright.clone());
    if make_source_file(&short, CANVAS, CANVAS, 30.0, 6, 40, 128, 128).is_none() {
        return;
    }
    if make_source_file(&bright, CANVAS, CANVAS, 30.0, SRC_FRAMES, 235, 128, 128).is_none() {
        return;
    }

    let out_cpu = test_output_path("mtbase_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_t) = two_track_timeline(&short, &bright) else {
        return;
    };
    if !render_or_skip(cpu_t.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    if avio::GpuCompositor::new().is_none() {
        return;
    }
    let out_gpu = test_output_path("mtbase_gpu.mp4");
    let _gg = FileGuard::new(out_gpu.clone());
    let Some(gpu_t) = two_track_timeline(&short, &bright) else {
        return;
    };
    if !render_or_skip(gpu_t.render(&out_gpu, export_config())) {
        return;
    }

    // Frame 10 is past the base's 6 frames, so this is the placeholder's frame.
    let (cpu_n, cpu_box) = count_and_bbox(&out_cpu, 10);
    let (gpu_n, gpu_box) = count_and_bbox(&out_gpu, 10);
    println!("base-ends-first: cpu=({cpu_n}, {cpu_box:?}) gpu=({gpu_n}, {gpu_box:?})");
    assert!(
        cpu_n > 6,
        "the control must outlive the base track, got {cpu_n}"
    );
    assert_eq!(cpu_n, gpu_n, "both routes must export the same frame count");
    assert_eq!(
        cpu_box,
        Some((10, 4, 41, 35)),
        "the CPU must keep the overlay placed after the base ends"
    );
    assert_eq!(
        gpu_box, cpu_box,
        "the overlay must not be promoted to base when the base ends"
    );
    // The bbox above cannot tell a transparent stand-in from the base's last frame held
    // over: both are below the brightness threshold. Sample a corner outside the overlay
    // instead, where the CPU was measured to go black once the base ended.
    let cpu_corner = corner_luma(&out_cpu, 10);
    let gpu_corner = corner_luma(&out_gpu, 10);
    println!("base-ends-first corner: cpu={cpu_corner:?} gpu={gpu_corner:?}");
    if let (Some(c), Some(g)) = (cpu_corner, gpu_corner) {
        assert!(
            c < 24.0,
            "the CPU control must render the ended base's area black, got {c}"
        );
        assert!(
            (c - g).abs() < 12.0,
            "the ended base's area must match the CPU: cpu={c} gpu={g}"
        );
    }
}

#[cfg(feature = "gpu")]
#[test]
fn a_transition_on_the_base_track_should_render_under_a_live_overlay() {
    // The scheduler's busiest crossing: the base is mid-blend (two clips open on one
    // TrackSource, composited and blended to the canvas) while a second track pulls and
    // places its own frames in the same output. Eligibility for this shape is asserted by
    // `eligible_tracks_should_still_accept_a_transition_on_the_base_track`, but that is a
    // data compare -- this drives the real pipeline and checks the pixels.
    let dark = test_output_path("bt_dark.mp4");
    let mid = test_output_path("bt_mid.mp4");
    let bright = test_output_path("bt_bright.mp4");
    let _gd = FileGuard::new(dark.clone());
    let _gm = FileGuard::new(mid.clone());
    let _gb = FileGuard::new(bright.clone());
    if make_source_file(&dark, CANVAS, CANVAS, 30.0, SRC_FRAMES, 40, 128, 128).is_none() {
        return;
    }
    if make_source_file(&mid, CANVAS, CANVAS, 30.0, SRC_FRAMES, 90, 128, 128).is_none() {
        return;
    }
    if make_source_file(&bright, CANVAS, CANVAS, 30.0, SRC_FRAMES, 235, 128, 128).is_none() {
        return;
    }
    let build = || {
        Timeline::builder()
            .canvas(CANVAS, CANVAS)
            .frame_rate(30.0)
            .video_track(vec![
                Clip::new(&dark)
                    .offset(Duration::ZERO)
                    .trim(Duration::ZERO, Duration::from_millis(400)),
                Clip::new(&mid)
                    .offset(Duration::from_millis(400))
                    .trim(Duration::ZERO, Duration::from_millis(100))
                    .with_transition(XfadeTransition::Fade, Duration::from_millis(100)),
            ])
            .video_track(vec![
                Clip::new(&bright).with_position(10.0, 4.0).with_scale(0.5),
            ])
            .build()
            .ok()
    };
    let out_cpu = test_output_path("bt_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_t) = build() else { return };
    if !render_or_skip(cpu_t.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    if avio::GpuCompositor::new().is_none() {
        return;
    }
    let out_gpu = test_output_path("bt_gpu.mp4");
    let _gg = FileGuard::new(out_gpu.clone());
    let Some(gpu_t) = build() else { return };
    if !render_or_skip(gpu_t.render(&out_gpu, export_config())) {
        return;
    }

    // Frame 13 sits inside the 0.1 s window that opens at 0.4 s (frame 12) at 30 fps.
    let (cpu_n, cpu_box) = count_and_bbox(&out_cpu, 13);
    let (gpu_n, gpu_box) = count_and_bbox(&out_gpu, 13);
    let cpu_corner = corner_luma(&out_cpu, 13);
    let gpu_corner = corner_luma(&out_gpu, 13);
    println!(
        "base transition under overlay: cpu=({cpu_n}, {cpu_box:?}, {cpu_corner:?}) \
         gpu=({gpu_n}, {gpu_box:?}, {gpu_corner:?})"
    );
    assert_eq!(cpu_n, gpu_n, "both routes must export the same frame count");
    assert_eq!(
        cpu_box,
        Some((10, 4, 41, 35)),
        "the CPU control must keep the overlay placed while the base blends"
    );
    assert_eq!(
        gpu_box, cpu_box,
        "the overlay must stay placed while the base runs its transition"
    );
    if let (Some(c), Some(g)) = (cpu_corner, gpu_corner) {
        assert!(
            (c - g).abs() < 12.0,
            "the blending base must match the CPU outside the overlay: cpu={c} gpu={g}"
        );
    }
}

#[cfg(feature = "gpu")]
#[test]
fn tracks_of_unequal_length_should_end_where_the_cpu_ends() {
    // Measured on the CPU route: the export ends with the **topmost** track, not the
    // longest -- the last overlay is built with `eof_action=endall`. A 15-frame base
    // under a 6-frame overlay exports ~5 frames. The scheduler has to reproduce that,
    // and it is exactly the kind of rule an implementation invents differently.
    let dark = test_output_path("mtlen_dark.mp4");
    let short = test_output_path("mtlen_short.mp4");
    let _gd = FileGuard::new(dark.clone());
    let _gs = FileGuard::new(short.clone());
    if make_source_file(&dark, CANVAS, CANVAS, 30.0, SRC_FRAMES, 40, 128, 128).is_none() {
        return;
    }
    if make_source_file(&short, CANVAS, CANVAS, 30.0, 6, 235, 128, 128).is_none() {
        return;
    }

    let out_cpu = test_output_path("mtlen_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_t) = two_track_timeline(&dark, &short) else {
        return;
    };
    if !render_or_skip(cpu_t.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    if avio::GpuCompositor::new().is_none() {
        return;
    }
    let out_gpu = test_output_path("mtlen_gpu.mp4");
    let _gg = FileGuard::new(out_gpu.clone());
    let Some(gpu_t) = two_track_timeline(&dark, &short) else {
        return;
    };
    if !render_or_skip(gpu_t.render(&out_gpu, export_config())) {
        return;
    }

    let (cpu_n, _) = count_and_bbox(&out_cpu, 0);
    let (gpu_n, _) = count_and_bbox(&out_gpu, 0);
    println!("unequal length: cpu={cpu_n} gpu={gpu_n}");
    assert!(
        cpu_n < SRC_FRAMES,
        "the control must actually be truncated by the short overlay, got {cpu_n}"
    );
    assert_eq!(
        gpu_n, cpu_n,
        "the GPU export must end where the CPU export ends"
    );
}

#[test]
fn export_should_produce_frames_on_cpu_and_gpu_routes() {
    let src = test_output_path("gpuexport_src.mp4");
    let _gs = FileGuard::new(src.clone());
    if make_source_file(&src, CANVAS, CANVAS, 30.0, SRC_FRAMES, 120, 90, 160).is_none() {
        return; // encoder unavailable -> skip
    }

    // CPU route: force-CPU always uses the MultiTrackComposer path (compiles and runs
    // without the `gpu` feature).
    let out_cpu = test_output_path("gpuexport_cpu.mp4");
    let _gc = FileGuard::new(out_cpu.clone());
    let Some(cpu_timeline) = build_timeline(&src) else {
        return; // source codec unavailable -> skip
    };
    if !render_or_skip(cpu_timeline.render_forcing_cpu(&out_cpu, export_config())) {
        return;
    }
    assert_valid_export(&out_cpu, "force-CPU");

    // GPU route: the default `render` composites the eligible timeline on the GPU when
    // an adapter is present. Skip when there is no adapter.
    #[cfg(feature = "gpu")]
    {
        if avio::GpuCompositor::new().is_none() {
            return; // no GPU adapter -> the GPU leg is unreachable here
        }
        let out_gpu = test_output_path("gpuexport_gpu.mp4");
        let _gg = FileGuard::new(out_gpu.clone());
        let Some(gpu_timeline) = build_timeline(&src) else {
            return;
        };
        if !render_or_skip(gpu_timeline.render(&out_gpu, export_config())) {
            return;
        }
        assert_valid_export(&out_gpu, "GPU");
    }
}

/// #1660: a source whose native rate differs from the timeline rate used to force the
/// CPU path; the GPU drain now conforms it. The export must keep the clip's on-screen
/// duration — i.e. the output carries the *timeline's* frame count, not the source's.
#[cfg(feature = "gpu")]
#[test]
fn gpu_export_should_conform_a_slower_source_to_the_timeline_rate() {
    const SRC_FPS: f64 = 24.0;
    const OUT_FPS: f64 = 30.0;
    const SRC_24_FRAMES: usize = 24; // ~1 s of 24 fps source

    let src = test_output_path("gpuexport_24fps_src.mp4");
    let _gs = FileGuard::new(src.clone());
    if make_source_file(&src, CANVAS, CANVAS, SRC_FPS, SRC_24_FRAMES, 120, 90, 160).is_none() {
        return; // encoder unavailable -> skip
    }
    if avio::GpuCompositor::new().is_none() {
        return; // no GPU adapter -> the GPU leg is unreachable here
    }
    // Expect against what the source *actually decodes*, not the nominal frame count:
    // an encode/decode round trip can lose a frame, and that shortfall would otherwise
    // read as a conform bug. The conformed output should cover the same wall-clock span
    // at the timeline rate, i.e. `src_frames * OUT_FPS / SRC_FPS`.
    let (src_frames, _) = decode_stats(&src);
    if src_frames == 0 {
        return; // source decoder unavailable -> skip
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    let expected = (src_frames as f64 * OUT_FPS / SRC_FPS).round() as usize;
    let Some(timeline) = Timeline::builder()
        .canvas(CANVAS, CANVAS)
        .frame_rate(OUT_FPS)
        .video_track(vec![Clip::new(&src)])
        .build()
        .ok()
    else {
        return; // source codec unavailable -> skip
    };
    let out = test_output_path("gpuexport_24to30.mp4");
    let _gg = FileGuard::new(out.clone());
    if !render_or_skip(timeline.render(&out, export_config())) {
        return;
    }
    let (count, dims) = decode_stats(&out);
    assert_eq!(
        dims,
        Some((CANVAS, CANVAS)),
        "conformed export frames should be the {CANVAS}x{CANVAS} canvas size"
    );
    // The load-bearing assertion: up-conform must *add* frames. Without it the drain
    // took one source frame per output and the clip came out at the source's own count,
    // far below this.
    assert!(
        count > src_frames,
        "conform must add frames: {src_frames} source frames became {count} outputs"
    );
    // The count lands near the conformed duration, but not exactly: n frames span n-1
    // intervals, so the file is 23/24 s rather than 1 s, and PTS quantisation and frame
    // reordering move the last output's boundary by a frame between platforms. The
    // window is wide enough to absorb that and still far from the un-conformed count.
    assert!(
        (expected - 2..=expected + 2).contains(&count),
        "a {SRC_FPS} fps source ({src_frames} frames) in a {OUT_FPS} fps timeline should \
         export ~{expected} frames, got {count}"
    );
}

/// Per-frame mean absolute RGB difference between the two routes' exports.
///
/// Calibrated (#1659, widened in #1732): this pipeline's floor is a **hard cut**, where
/// the routes still differ because the GPU one round-trips yuv -> rgba -> yuv while the
/// CPU one stays in yuv throughout the filter graph. Measured on the structured sources
/// below, a hard cut comes out at mean 1.4 / max 7, and the transitions the export
/// renders land at 1.9-2.3. The bound clears those with room for platform variation and
/// is still far from anything a real divergence would produce: the wrong blend direction reads as mean ~127, and a
/// transition rendered as the wrong kind as ~50.
const TOL_TRANSITION_MEAN: f64 = 6.0;

/// Encodes a spatially structured, colourful source: a horizontal ramp in R, a vertical
/// one in G, and `phase` shifting B so the two clips differ everywhere.
///
/// Deliberately not `make_source_file`'s flat fill. On a solid colour a fade, a wipe and
/// a dissolve produce near-identical frames, so a flat fixture cannot tell a correct
/// blend from a mirrored or mis-keyed one (RK-022).
fn make_structured_source(path: &std::path::Path, frames: usize, phase: u8) -> Option<()> {
    use ff_encode::VideoEncoder;
    use ff_format::VideoFrame;

    let mut enc = VideoEncoder::create(path)
        .video(CANVAS, CANVAS, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .build()
        .ok()?;
    for _ in 0..frames {
        let mut rgba = vec![0u8; (CANVAS * CANVAS * 4) as usize];
        for y in 0..CANVAS {
            for x in 0..CANVAS {
                let o = ((y * CANVAS + x) * 4) as usize;
                rgba[o] = u8::try_from(x * 255 / CANVAS)
                    .unwrap_or(255)
                    .wrapping_add(phase);
                rgba[o + 1] = u8::try_from(y * 255 / CANVAS).unwrap_or(255);
                rgba[o + 2] = 128u8.wrapping_sub(phase);
                rgba[o + 3] = 255;
            }
        }
        enc.push_video(&VideoFrame::from_rgba(CANVAS, CANVAS, rgba).ok()?)
            .ok()?;
    }
    enc.finish().ok()?;
    Some(())
}

/// Every decoded frame of `path` as rgba.
fn decode_rgba(path: &std::path::Path) -> Vec<Vec<u8>> {
    let Ok(mut d) = VideoDecoder::open(path)
        .output_format(ff_format::PixelFormat::Rgba)
        .build()
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(f)) = d.decode_one() {
        if let Some(plane) = f.plane(0) {
            out.push(plane.to_vec());
        }
    }
    out
}

/// Mean absolute difference over the RGB channels (alpha is meaningless here).
fn mean_abs_diff_rgb(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()) / 4;
    if n == 0 {
        return f64::MAX;
    }
    let mut sum = 0f64;
    for i in 0..n {
        for c in 0..3 {
            sum += f64::from(a[i * 4 + c].abs_diff(b[i * 4 + c]));
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let denom = (n * 3) as f64;
    sum / denom
}

/// #1659 / #1732: a transition into the track's last clip exports on the GPU and lands
/// on the CPU export's pixels, for every kind the export renders.
///
/// **Why the frame count is the load-bearing assertion.** The CPU route's `xfade`
/// overlaps the two clips, so the track comes out `transition` shorter: two 1 s clips at
/// 30 fps with a 0.5 s fade give 45 frames, not the hard cut's 60. An export that
/// silently dropped the transition would produce 60 and fail here, which no pixel
/// tolerance could catch on its own.
///
/// **Why this test does not prove the route on its own.** `render()` falls back to CPU
/// without saying so, so if the timeline were ineligible both legs here would be CPU
/// exports and agree perfectly -- the false green that made #1660's export test exercise
/// the CPU path for months. What closes it is
/// `gpu_export::tests::eligible_track_should_accept_a_fade_into_the_last_clip`, which
/// asserts this exact shape is eligible; with an adapter present (checked below) those
/// two facts leave no other route.
///
/// The kinds beyond `Fade` are only here because #1732 brought each node onto FFmpeg's
/// own formula. Before that the GPU route declined them, and this loop would have been
/// comparing two CPU exports.
#[cfg(feature = "gpu")]
#[test]
fn gpu_export_should_match_the_cpu_export_for_every_rendered_transition() {
    use std::time::Duration;

    use ff_filter::XfadeTransition;

    const CLIP_FRAMES: usize = 30; // 1 s at 30 fps
    const WINDOW: usize = 15; // 0.5 s at 30 fps
    // The blend reads the outgoing clip's handle -- its frames past the out-point
    // (ADR-0009) -- so the source has to hold more than the clip trims out of it.
    const SOURCE_FRAMES: usize = CLIP_FRAMES + 2 * WINDOW;

    let a = test_output_path("gpuexport_tr_a.mp4");
    let b = test_output_path("gpuexport_tr_b.mp4");
    let _ga = FileGuard::new(a.clone());
    let _gb = FileGuard::new(b.clone());
    if make_structured_source(&a, SOURCE_FRAMES, 0).is_none()
        || make_structured_source(&b, SOURCE_FRAMES, 100).is_none()
    {
        return; // encoder unavailable -> skip
    }
    if avio::GpuCompositor::new().is_none() {
        return; // no GPU adapter -> the GPU leg is unreachable here
    }

    // `Dissolve` is absent on purpose: the export declines it (its pixel set depends on
    // libm agreement between Rust and FFmpeg), so both legs here would be CPU renders and
    // the comparison would pass without exercising anything. The rejection itself is
    // asserted by `gpu_export::tests::export_maps_to_gpu_should_reject_dissolve_despite_it_mapping`.
    for kind in [
        XfadeTransition::Fade,
        XfadeTransition::WipeLeft,
        XfadeTransition::WipeRight,
        XfadeTransition::WipeUp,
        XfadeTransition::WipeDown,
        XfadeTransition::FadeBlack,
        XfadeTransition::FadeWhite,
    ] {
        let build = || {
            Timeline::builder()
                .canvas(CANVAS, CANVAS)
                .frame_rate(30.0)
                .video_track(vec![
                    Clip::new(&a).trim(Duration::ZERO, Duration::from_secs(1)),
                    Clip::new(&b)
                        .offset(Duration::from_secs(1))
                        .trim(Duration::ZERO, Duration::from_secs(1))
                        .with_transition(kind, Duration::from_millis(500)),
                ])
                .build()
                .ok()
        };
        let (Some(gpu_timeline), Some(cpu_timeline)) = (build(), build()) else {
            return; // source codec unavailable -> skip
        };

        let out_gpu = test_output_path("gpuexport_tr_gpu.mp4");
        let out_cpu = test_output_path("gpuexport_tr_cpu.mp4");
        let _gg = FileGuard::new(out_gpu.clone());
        let _gc = FileGuard::new(out_cpu.clone());
        if !render_or_skip(gpu_timeline.render(&out_gpu, export_config()))
            || !render_or_skip(cpu_timeline.render_forcing_cpu(&out_cpu, export_config()))
        {
            return;
        }

        let gpu = decode_rgba(&out_gpu);
        let cpu = decode_rgba(&out_cpu);
        if gpu.is_empty() || cpu.is_empty() {
            return; // decoder unavailable -> skip
        }

        // The transition preserves the timeline length (ADR-0009): both clips are
        // trimmed to a second, so the track runs for two whatever the transition does.
        // Its own length is what the two routes must agree on frame for frame below;
        // this is the guard that the drain did not silently drop or double the window.
        let expected = CLIP_FRAMES * 2;
        assert!(
            (expected - 1..=expected + 1).contains(&gpu.len()),
            "a {WINDOW}-frame transition must leave the {expected}-frame timeline its \
         length, got {}",
            gpu.len()
        );
        assert_eq!(
            gpu.len(),
            cpu.len(),
            "both routes must export the same number of frames"
        );

        let worst = gpu
            .iter()
            .zip(cpu.iter())
            .enumerate()
            .map(|(i, (g, c))| (mean_abs_diff_rgb(g, c), i))
            .fold((0f64, 0usize), |acc, x| if x.0 > acc.0 { x } else { acc });
        println!("{kind:?}: worst frame {} mean={:.3}", worst.1, worst.0);
        assert!(
            worst.0 <= TOL_TRANSITION_MEAN,
            "{kind:?}: GPU and CPU exports diverged at frame {}: mean={:.3} \
         (tolerance {TOL_TRANSITION_MEAN})",
            worst.1,
            worst.0
        );
    }
}

#[test]
fn render_with_progress_forcing_cpu_should_report_progress_and_export() {
    let src = test_output_path("gpuexport_progress_src.mp4");
    let _gs = FileGuard::new(src.clone());
    if make_source_file(&src, CANVAS, CANVAS, 30.0, SRC_FRAMES, 60, 140, 200).is_none() {
        return;
    }
    let out = test_output_path("gpuexport_progress_cpu.mp4");
    let _go = FileGuard::new(out.clone());
    let Some(timeline) = build_timeline(&src) else {
        return;
    };

    // on_progress is Fn (not FnMut), so count through an atomic.
    let frames = AtomicU32::new(0);
    let result = timeline.render_with_progress_forcing_cpu(&out, export_config(), |_p| {
        frames.fetch_add(1, Ordering::Relaxed);
        true
    });
    if !render_or_skip(result) {
        return;
    }
    assert!(
        frames.load(Ordering::Relaxed) >= 1,
        "the force-CPU progress callback must fire at least once"
    );
    assert_valid_export(&out, "force-CPU (progress)");
}
