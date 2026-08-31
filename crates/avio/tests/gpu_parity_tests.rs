//! GPU/CPU parity and fallback tests for the compositing bridge (Br5, #1628).
//!
//! Parity: for the supported v1 set (identity transform, canvas-aspect frame), the
//! GPU compositor ([`GpuCompositor`]) and the CPU preview compositor
//! (`ff_filter::RealtimeComposer`) must agree within tolerance. Both consume the same
//! `RealtimeLayer`, so they are compared directly in rgba, with no encode/decode
//! noise. Since the GPU *export* drain composites through the same [`GpuCompositor`],
//! this covers the export compositing math too; the end-to-end export-vs-force-CPU
//! smoke lives in `gpu_export_tests.rs`.
//!
//! Fallback: the GPU compositor returns `None` (never panics) for every unsupported
//! input, and the preview runner keeps advancing (never hangs) when it falls back.
//!
//! Double-gated (RK-002 / RK-020): the GPU leg needs an adapter and the CPU leg needs
//! an `FFmpeg` built with filters (`RealtimeComposer` is libavfilter-based). Each leg
//! skips gracefully when unavailable, so the suite is green on headless / minimal CI
//! and the real parity runs on a full dev build (and macOS CI). Tolerances are
//! calibrated on the dev machine (see the constants); exact pixel equality across GPU
//! drivers is explicitly not asserted (docs/rules/test.md).

#![cfg(all(feature = "gpu", feature = "preview"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use avio::{
    AnimatedValue, BlendMode, Clip, Color, CompositeOp, FilterStep, GpuCompositor, PixelFormat,
    PlayerHandle, RealtimeLayer, Timeline, TimelinePlayer, VideoFrame,
};
use ff_filter::RealtimeComposer;
use ff_preview::FrameSink;

// Tolerances (mean absolute per-channel RGB difference, 0..255). Calibrated on the
// dev machine; alpha is excluded (the compositor blends RGB over transparent black,
// an alpha detail orthogonal to colour parity). Measured on this build: passthrough
// is pixel-exact (mean 0.0), colour grade is mean 6.6 (max 33). The thresholds keep a
// margin for GPU-driver rounding while staying tight enough to fail on a real
// divergence (a stretch/letterbox/axis-swap of the gradient is tens of levels).
const TOL_PASSTHROUGH_MEAN: f64 = 2.0; // identity passthrough: pixel-exact here
const TOL_COLOR_GRADE_MEAN: f64 = 20.0; // GPU ColorGradeNode vs FFmpeg `eq`: ~6.6 here
const TOL_BLUR_MEAN: f64 = 20.0; // GPU GaussianBlurNode vs FFmpeg `gblur`: ~9.0 here (different kernels)

/// A deterministic non-uniform pattern: R ramps in x, G in y, B in x+y. A stretch,
/// letterbox, axis-swap, or channel bug shows up here where a flat fill would not
/// (RK-015).
fn gradient_rgba(w: u32, h: u32) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let mut v = Vec::with_capacity(wu * hu * 4);
    for y in 0..hu {
        for x in 0..wu {
            let r = (x * 255 / (wu - 1).max(1)) as u8;
            let g = (y * 255 / (hu - 1).max(1)) as u8;
            let b = ((x + y) * 255 / (wu + hu - 2).max(1)) as u8;
            v.extend_from_slice(&[r, g, b, 255]);
        }
    }
    v
}

/// A high-frequency `block`-sized checkerboard (black/white). A blur visibly smooths
/// it, so a parity test on it is non-vacuous (RK-015): a GPU that failed to blur would
/// keep the sharp squares and diverge far from the CPU-blurred reference.
fn checkerboard_rgba(w: u32, h: u32, block: u32) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let mut v = Vec::with_capacity(wu * hu * 4);
    for y in 0..h {
        for x in 0..w {
            let on = ((x / block) + (y / block)) % 2 == 0;
            let c = if on { 255u8 } else { 0u8 };
            v.extend_from_slice(&[c, c, c, 255]);
        }
    }
    v
}

/// Mean absolute per-channel difference over the RGB channels only (skips alpha).
fn mean_abs_diff_rgb(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "buffers must be the same length");
    let mut sum = 0u64;
    let mut n = 0u64;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            sum += u64::from(pa[c].abs_diff(pb[c]));
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { sum as f64 / n as f64 }
}

fn max_abs_diff_rgb(a: &[u8], b: &[u8]) -> u8 {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .flat_map(|(pa, pb)| (0..3).map(move |c| pa[c].abs_diff(pb[c])))
        .max()
        .unwrap_or(0)
}

/// A canvas-sized, identity, single base layer carrying `effects` (the v1 supported
/// shape the GPU path renders without falling back).
fn base_layer(w: u32, h: u32, effects: Vec<FilterStep>) -> RealtimeLayer {
    RealtimeLayer {
        width: w,
        height: h,
        pixel_format: PixelFormat::Rgba,
        effects,
        opacity: AnimatedValue::Static(1.0),
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        blend_mode: BlendMode::Normal,
        composite_op: CompositeOp::Over,
    }
}

/// Composites one layer + frame on the CPU (`RealtimeComposer`) to rgba, or `None`
/// when `FFmpeg` filters are unavailable (skip).
fn cpu_composite(layer: &RealtimeLayer, frame: &VideoFrame, canvas: (u32, u32)) -> Option<Vec<u8>> {
    let mut composer =
        RealtimeComposer::with_canvas(std::slice::from_ref(layer), Some(canvas)).ok()?;
    composer.push_layer(0, frame).ok()?;
    composer.pull().ok()??.to_rgba()
}

/// Composites one layer + frame on the GPU (`GpuCompositor`) to rgba, or `None` when
/// no adapter is present or the layer is unsupported (fallback).
fn gpu_composite(
    gpu: &mut GpuCompositor,
    layer: &RealtimeLayer,
    frame: &VideoFrame,
    canvas: (u32, u32),
) -> Option<Vec<u8>> {
    let (rgba, _w, _h) = gpu.composite(&[(layer, frame)], canvas, Duration::ZERO)?;
    Some(rgba)
}

#[test]
fn passthrough_gpu_should_match_input_within_tolerance() {
    // The broadest guard (adapter only, no filters): an identity, canvas-sized layer
    // composited on the GPU must reproduce the input. Catches stretch / colour-space /
    // corruption regressions.
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    let layer = base_layer(w, h, vec![]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported passthrough layer must composite on the GPU");
    };
    assert_eq!(out.len(), input.len());
    let mean = mean_abs_diff_rgb(&out, &input);
    println!(
        "passthrough GPU vs input: mean={mean:.3} max={}",
        max_abs_diff_rgb(&out, &input)
    );
    assert!(
        mean <= TOL_PASSTHROUGH_MEAN,
        "GPU passthrough diverged from the input beyond tolerance: mean={mean}"
    );
}

#[test]
fn passthrough_gpu_should_match_cpu_within_tolerance() {
    // The literal preview parity: GPU compositor vs CPU RealtimeComposer for the
    // supported passthrough case. Double-gated (adapter + filters).
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let layer = base_layer(w, h, vec![]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported passthrough layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "passthrough GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_PASSTHROUGH_MEAN,
        "GPU and CPU passthrough diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn color_grade_gpu_should_match_cpu_within_tolerance() {
    // ColorGrade sanity: GPU ColorGradeNode vs FFmpeg `eq`. Different implementations,
    // so a loose tolerance (calibrated). Double-gated.
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Eq {
            brightness: 0.1,
            contrast: 1.2,
            saturation: 1.1,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported colour-graded layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "color-grade GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_COLOR_GRADE_MEAN,
        "GPU and CPU colour grade diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn blur_gpu_should_match_cpu_within_tolerance() {
    // Gaussian blur parity: GPU GaussianBlurNode (map_scene maps `GBlur`) vs the CPU
    // `gblur` filter. Different kernel implementations, so a loose calibrated
    // tolerance. Double-gated (adapter + filters); a high-frequency checkerboard makes
    // it non-vacuous (a GPU that did not blur would diverge far).
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, checkerboard_rgba(w, h, 4)).unwrap();
    let layer = base_layer(w, h, vec![FilterStep::GBlur { sigma: 3.0 }]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported blur layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "blur GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_BLUR_MEAN,
        "GPU and CPU blur diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn gpu_compositor_should_fall_back_not_panic_for_unsupported_inputs() {
    // Every unsupported input must return None (CPU fallback), never panic (AC2).
    // Adapter-gated (the gate lives past GpuCompositor::new).
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };

    // Non-identity transform (RK-020: model pixels are not ff_render UV units).
    let mut positioned = base_layer(w, h, vec![]);
    positioned.x = AnimatedValue::Static(10.0);
    assert!(
        gpu_composite(&mut gpu, &positioned, &frame, (w, h)).is_none(),
        "a positioned layer must fall back"
    );

    // Aspect mismatch: a 64x48 frame on a square canvas would be stretched.
    let square = base_layer(w, h, vec![]);
    assert!(
        gpu_composite(&mut gpu, &square, &frame, (48, 48)).is_none(),
        "an aspect-mismatched frame must fall back"
    );

    // Unsupported blend mode (no ff-render equivalent).
    let mut glow = base_layer(w, h, vec![]);
    glow.blend_mode = BlendMode::Glow;
    assert!(
        gpu_composite(&mut gpu, &glow, &frame, (w, h)).is_none(),
        "an unsupported blend mode must fall back"
    );

    // Unsupported effect (no GPU node).
    let hue = base_layer(w, h, vec![FilterStep::Hue { degrees: 30.0 }]);
    assert!(
        gpu_composite(&mut gpu, &hue, &frame, (w, h)).is_none(),
        "an unsupported effect must fall back"
    );
}

#[test]
#[ignore = "requires the color filter + a GPU adapter; run with -- --include-ignored"]
fn preview_runner_should_fall_back_and_advance_on_unsupported_clip() {
    // A positioned clip makes the GPU compositor return None every frame, so the
    // runner falls back to its CPU compositor. It must keep advancing and terminate
    // (never hang) — the RK-019 hang guard, now over the GPU-attached runner. Skips
    // without an adapter or filters.
    if GpuCompositor::new().is_none() {
        println!("skipping: no GPU adapter");
        return;
    }

    struct PtsSink {
        pts: Arc<Mutex<Vec<Duration>>>,
        handle: PlayerHandle,
    }
    impl FrameSink for PtsSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, pts: Duration) {
            let mut log = self
                .pts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.push(pts);
            if log.len() >= 12 {
                self.handle.stop();
            }
        }
    }

    let timeline = Timeline::builder()
        .canvas(64, 48)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::solid(Color::rgb(20, 120, 200))
                .trim(Duration::ZERO, Duration::from_secs(1))
                .with_position(10.0, 0.0),
        ])
        .build()
        .expect("timeline build failed");

    let (mut runner, handle) = match TimelinePlayer::open(&timeline) {
        Ok(p) => p,
        Err(e) => {
            println!("skipping: open failed (filters unavailable?): {e}");
            return;
        }
    };
    let pts = Arc::new(Mutex::new(Vec::<Duration>::new()));
    runner.set_sink(Box::new(PtsSink {
        pts: Arc::clone(&pts),
        handle: handle.clone(),
    }));
    // If this returns at all, the GPU-fallback runner did not hang.
    let _ = runner.run();

    let pts = pts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pts.is_empty() {
        println!("skipping: color filter unavailable (no frames rendered)");
        return;
    }
    assert!(
        pts.last() > pts.first(),
        "GPU-fallback playback PTS must advance: {:?}",
        &pts[..pts.len().min(6)]
    );
}
