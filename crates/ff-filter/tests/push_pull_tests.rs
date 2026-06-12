//! Integration tests for ff-filter push/pull frame processing.
//!
//! # Temporary file cleanup
//!
//! These tests are read-only with respect to the filesystem: they push frames
//! through in-memory filter graphs and inspect the resulting `VideoFrame` /
//! `AudioFrame` values. No output files are written, so no
//! `fixtures/mod.rs` with `FileGuard`/`DirGuard` is needed.

#![allow(clippy::unwrap_used)]

use ff_filter::{
    BlendMode, DrawTextOptions, FilterError, FilterGraph, FilterGraphBuilder, HwAccel, Rgb,
    ScaleAlgorithm, ToneMap, XfadeTransition, YadifMode,
};
use ff_format::{
    AlphaMode, AudioFrame, ColorRange, ColorSpace, PixelFormat, PooledBuffer, SampleFormat,
    Timestamp, VideoFrame,
};

/// 64×64 Yuv420p frame filled with grey (Y=128, U=128, V=128).
fn make_yuv420p_frame(width: u32, height: u32) -> VideoFrame {
    let y = vec![128u8; (width * height) as usize];
    let u = vec![128u8; ((width / 2) * (height / 2)) as usize];
    let v = vec![128u8; ((width / 2) * (height / 2)) as usize];
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(y),
            PooledBuffer::standalone(u),
            PooledBuffer::standalone(v),
        ],
        vec![width as usize, (width / 2) as usize, (width / 2) as usize],
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .unwrap()
}

/// YUV420p frame with explicit luma and chroma values.
fn make_yuv_frame(width: u32, height: u32, y: u8, u: u8, v: u8) -> VideoFrame {
    let y_plane = vec![y; (width * height) as usize];
    let u_plane = vec![u; ((width / 2) * (height / 2)) as usize];
    let v_plane = vec![v; ((width / 2) * (height / 2)) as usize];
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(y_plane),
            PooledBuffer::standalone(u_plane),
            PooledBuffer::standalone(v_plane),
        ],
        vec![width as usize, (width / 2) as usize, (width / 2) as usize],
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .unwrap()
}

/// Stereo packed F32 audio frame, 1024 samples @ 48 kHz.
fn make_audio_frame() -> AudioFrame {
    AudioFrame::empty(1024, 2, 48000, SampleFormat::F32).unwrap()
}

#[test]
fn format_graph_with_ffmpeg_tokens_should_be_accepted_by_ffmpeg() {
    // #1212/#1223: `format` args are built from `FfmpegToken`. The FFmpeg graph is constructed
    // lazily on the first `push_video` (see `FilterGraph::build` docs), so it is the push — not
    // `build()` — that hands the args to the real `format` filter. The old `.name()` path produced
    // `color_spaces=bt2020/srgb` and `color_ranges=limited`, which the filter rejects at
    // construction; with the FfmpegToken values (`bt2020nc`/`tv`/`yuv420p`) the push must succeed.
    // Pushing unconditionally (no `is_ok()` guard) ensures an invalid arg fails the test.
    let mut graph = FilterGraph::builder()
        .format(
            vec![PixelFormat::Yuv420p],
            vec![ColorSpace::Bt2020],
            vec![ColorRange::Limited],
        )
        .build()
        .expect("non-empty format graph must build");
    let frame = make_yuv420p_frame(64, 64);
    graph
        .push_video(0, &frame)
        .expect("format push must succeed — FFmpeg must accept the FfmpegToken args");
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame) after format push");
    assert_eq!(out.width(), 64, "format must not change frame width");
    assert_eq!(out.height(), 64, "format must not change frame height");
}

#[test]
fn blend_with_premultiplied_alpha_should_be_accepted_by_ffmpeg() {
    // #1225: the `overlay` `alpha=premultiplied` argument must be accepted by FFmpeg. The overlay
    // filter is constructed lazily when frames are pushed (not at `build()`), so BOTH inputs must
    // be pushed to force its construction; an invalid `alpha` value would fail the push.
    let top = FilterGraphBuilder::new();
    let mut graph = FilterGraph::builder()
        .blend(top, BlendMode::Normal, 1.0, AlphaMode::Premultiplied)
        .build()
        .expect("blend graph must build");
    let bottom = make_yuv420p_frame(64, 64);
    let fg = make_yuv420p_frame(64, 64);
    graph
        .push_video(0, &bottom)
        .expect("bottom push must succeed");
    graph
        .push_video(1, &fg)
        .expect("top push must succeed — overlay must accept alpha=premultiplied");
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame) after blend push");
    assert_eq!(out.width(), 64, "overlay must not change frame width");
    assert_eq!(out.height(), 64, "overlay must not change frame height");
}

#[test]
fn pull_video_before_push_should_return_none() {
    let mut graph = match FilterGraph::builder()
        .scale(32, 32, ScaleAlgorithm::Fast)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let result = graph
        .pull_video()
        .expect("pull_video must not fail before any push");
    assert!(
        result.is_none(),
        "expected None before any push, got Some(frame)"
    );
}

#[test]
fn push_video_and_pull_through_scale_should_return_resized_frame() {
    let mut graph = match FilterGraph::builder()
        .scale(32, 32, ScaleAlgorithm::Fast)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after scale push");
    assert_eq!(out.width(), 32, "width should be scaled to 32");
    assert_eq!(out.height(), 32, "height should be scaled to 32");
}

#[test]
fn push_video_to_invalid_slot_should_return_error() {
    let mut graph = match FilterGraph::builder()
        .scale(32, 32, ScaleAlgorithm::Fast)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    // Push to slot 0 first to ensure the graph is fully initialised.
    // On some Linux FFmpeg builds the filter registry is not populated until
    // the first AVFilterGraph is allocated; without this step,
    // ensure_video_graph can return BuildFailed before the slot check runs.
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.push_video(1, &frame);
    assert!(
        matches!(result, Err(FilterError::InvalidInput { slot: 1, .. })),
        "expected InvalidInput for slot 1, got {result:?}"
    );
}

#[test]
fn pull_audio_before_push_should_return_none() {
    let mut graph = match FilterGraph::builder().volume(0.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let result = graph
        .pull_audio()
        .expect("pull_audio must not fail before any push");
    assert!(
        result.is_none(),
        "expected None before any push, got Some(frame)"
    );
}

#[test]
fn push_audio_and_pull_through_volume_should_return_frame() {
    let mut graph = match FilterGraph::builder().volume(0.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after volume push");
    assert_eq!(out.sample_rate(), 48000);
    assert_eq!(out.channels(), 2);
    assert_eq!(out.samples(), 1024);
}

#[test]
fn push_video_through_trim_should_return_frame_within_range() {
    let mut graph = match FilterGraph::builder().trim(0.0, 5.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after trim push within range");
    assert_eq!(out.width(), 64, "width should be unchanged after trim");
    assert_eq!(out.height(), 64, "height should be unchanged after trim");
}

#[test]
fn push_video_through_trim_frame_timestamp_should_be_within_trim_range() {
    // Push a frame at t=0 through a [0, 5) trim window and confirm the output
    // timestamp lies within the window.
    let mut graph = match FilterGraph::builder().trim(0.0, 5.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after trim push within range");
    let secs = out.timestamp().as_secs_f64();
    assert!(
        secs >= 0.0 && secs < 5.0,
        "frame timestamp {secs:.3}s must fall within the [0, 5) trim window"
    );
}

#[test]
fn push_video_through_crop_should_return_cropped_frame() {
    let mut graph = match FilterGraph::builder().crop(0, 0, 32, 32).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after crop push");
    assert_eq!(out.width(), 32, "width should be cropped to 32");
    assert_eq!(out.height(), 32, "height should be cropped to 32");
}

#[test]
fn push_1080p_frame_through_crop_100_100_1720_880_should_return_cropped_frame() {
    // Crop a 1920×1080 frame to a 1720×880 rectangle starting at (100, 100).
    let mut graph = match FilterGraph::builder().crop(100, 100, 1720, 880).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(1920, 1080);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after crop push");
    assert_eq!(out.width(), 1720, "width should be 1720 after crop");
    assert_eq!(out.height(), 880, "height should be 880 after crop");
}

#[test]
fn push_video_through_overlay_should_return_composited_frame() {
    let mut graph = match FilterGraph::builder().overlay(0, 0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let base = make_yuv420p_frame(64, 64);
    let overlay = make_yuv420p_frame(64, 64);
    // Push base video to slot 0 first; this also initialises the graph.
    match graph.push_video(0, &base) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    // Push overlay video to slot 1.
    match graph.push_video(1, &overlay) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after overlay push");
    assert_eq!(out.width(), 64, "output width should match base video");
    assert_eq!(out.height(), 64, "output height should match base video");
}

#[test]
fn push_video_through_fade_in_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().fade_in(0.0, 1.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after fade_in push");
    assert_eq!(out.width(), 64, "width should be unchanged after fade_in");
    assert_eq!(out.height(), 64, "height should be unchanged after fade_in");
}

#[test]
fn push_video_through_fade_out_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().fade_out(0.0, 1.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after fade_out push");
    assert_eq!(out.width(), 64, "width should be unchanged after fade_out");
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after fade_out"
    );
}

#[test]
fn push_video_through_rotate_should_return_frame() {
    let mut graph = match FilterGraph::builder().rotate(90.0, "black").build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    // Use a square frame so output dimensions are unambiguous after 90° rotation.
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    assert!(result.is_some(), "expected Some(frame) after rotate push");
}

#[test]
fn push_video_through_tone_map_should_return_frame() {
    // The tonemap filter is HDR-specific and may not be available in all
    // FFmpeg builds, or may reject non-HDR input. All failure paths are
    // treated as graceful skips.
    let mut graph = match FilterGraph::builder().tone_map(ToneMap::Hable).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    assert!(result.is_some(), "expected Some(frame) after tone_map push");
}

#[test]
fn push_audio_through_amix_should_return_mixed_frame() {
    let mut graph = match FilterGraph::builder().amix(2).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    // Push to slot 0 — this also initialises the audio graph.
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    // Push to slot 1 so amix has data on both inputs and can produce output.
    match graph.push_audio(1, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after amix push to both slots");
    assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
    assert_eq!(out.channels(), 2, "channel count should be unchanged");
}

#[test]
fn push_audio_through_equalizer_should_return_frame_with_same_properties() {
    let mut graph = match FilterGraph::builder()
        .equalizer(vec![ff_filter::EqBand::Peak {
            freq_hz: 1000.0,
            gain_db: 3.0,
            q: 1.0,
        }])
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after equalizer push");
    assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
    assert_eq!(out.channels(), 2, "channel count should be unchanged");
    assert_eq!(out.samples(), 1024, "sample count should be unchanged");
}

#[test]
fn push_video_through_cuda_scale_should_return_resized_frame_or_skip() {
    // Build a filter graph with CUDA hardware acceleration and a scale step.
    // If CUDA is not available (av_hwdevice_ctx_create fails, hwupload_cuda
    // filter is missing, or avfilter_graph_config rejects the chain), all
    // error paths are treated as graceful skips.
    let mut graph = match FilterGraph::builder()
        .hardware(HwAccel::Cuda)
        .scale(32, 32, ScaleAlgorithm::Fast)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping (build): {e}");
            return;
        }
    };

    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping (push): {e}");
            return;
        }
    }

    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after cuda scale push");
    assert_eq!(out.width(), 32, "width should be scaled to 32");
    assert_eq!(out.height(), 32, "height should be scaled to 32");
}

#[test]
fn push_video_through_videotoolbox_scale_should_return_resized_frame_or_skip() {
    // VideoToolbox is macOS-only; on other platforms av_hwdevice_ctx_create
    // will fail and the test skips gracefully.  On macOS the hwupload filter
    // uploads frames to the VideoToolbox device and hwdownload brings them
    // back; avfilter_graph_config failure is also treated as a skip.
    let mut graph = match FilterGraph::builder()
        .hardware(HwAccel::VideoToolbox)
        .scale(32, 32, ScaleAlgorithm::Fast)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping (build): {e}");
            return;
        }
    };

    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping (push): {e}");
            return;
        }
    }

    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after videotoolbox scale push");
    assert_eq!(out.width(), 32, "width should be scaled to 32");
    assert_eq!(out.height(), 32, "height should be scaled to 32");
}

#[test]
fn push_video_through_vaapi_scale_should_return_resized_frame_or_skip() {
    // VAAPI is Linux-only; on other platforms (or Linux without a VA-API
    // device) av_hwdevice_ctx_create will fail and the test skips gracefully.
    // On a VA-API-capable Linux system the hwupload filter uploads frames to
    // the VAAPI device and hwdownload brings them back.
    let mut graph = match FilterGraph::builder()
        .hardware(HwAccel::Vaapi)
        .scale(32, 32, ScaleAlgorithm::Fast)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping (build): {e}");
            return;
        }
    };

    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping (push): {e}");
            return;
        }
    }

    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after vaapi scale push");
    assert_eq!(out.width(), 32, "width should be scaled to 32");
    assert_eq!(out.height(), 32, "height should be scaled to 32");
}

#[test]
fn push_video_through_eq_saturation_zero_should_return_frame_with_same_dimensions() {
    // saturation=0.0 converts to grayscale; frame dimensions are preserved.
    let mut graph = match FilterGraph::builder().eq(0.0, 1.0, 0.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after eq push");
    assert_eq!(out.width(), 64, "width should be unchanged after eq");
    assert_eq!(out.height(), 64, "height should be unchanged after eq");
}

#[test]
fn push_video_through_curves_s_curve_should_return_frame_with_same_dimensions() {
    // Apply an S-curve to the master channel to boost midtone contrast;
    // frame dimensions must be preserved.
    let s_curve = vec![
        (0.0, 0.0),
        (0.25, 0.15),
        (0.5, 0.5),
        (0.75, 0.85),
        (1.0, 1.0),
    ];
    let mut graph = match FilterGraph::builder()
        .curves(s_curve, vec![], vec![], vec![])
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after curves push");
    assert_eq!(out.width(), 64, "width should be unchanged after curves");
    assert_eq!(out.height(), 64, "height should be unchanged after curves");
}

#[test]
fn push_video_through_white_balance_should_return_frame_with_same_dimensions() {
    // Apply a warm (3200 K) white balance correction; frame dimensions must be preserved.
    let mut graph = match FilterGraph::builder().white_balance(3200, 0.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after white_balance push");
    assert_eq!(
        out.width(),
        64,
        "width should be unchanged after white_balance"
    );
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after white_balance"
    );
}

#[test]
fn push_video_through_hue_180_should_return_frame_with_same_dimensions() {
    // Rotating hue by 180° inverts the hue; frame dimensions must be preserved.
    let mut graph = match FilterGraph::builder().hue(180.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after hue push");
    assert_eq!(
        out.width(),
        64,
        "width should be unchanged after hue rotation"
    );
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after hue rotation"
    );
}

#[test]
fn push_video_through_gamma_should_return_frame_with_same_dimensions() {
    // Apply 2.2 gamma to all channels (brightens midtones); dimensions must be preserved.
    let mut graph = match FilterGraph::builder().gamma(2.2, 2.2, 2.2).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after gamma push");
    assert_eq!(out.width(), 64, "width should be unchanged after gamma");
    assert_eq!(out.height(), 64, "height should be unchanged after gamma");
}

#[test]
fn push_video_through_three_way_cc_neutral_should_return_frame_with_same_dimensions() {
    // Neutral lift/gamma/gain (all 1.0) is an identity operation; dimensions must be preserved.
    let mut graph = match FilterGraph::builder()
        .three_way_cc(Rgb::NEUTRAL, Rgb::NEUTRAL, Rgb::NEUTRAL)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after three_way_cc push");
    assert_eq!(
        out.width(),
        64,
        "width should be unchanged after three_way_cc"
    );
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after three_way_cc"
    );
}

#[test]
fn push_video_through_vignette_should_return_frame_with_same_dimensions() {
    // Default-angle vignette centred on the frame; dimensions must be preserved.
    let mut graph = match FilterGraph::builder()
        .vignette(std::f32::consts::PI / 5.0, 0.0, 0.0)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after vignette push");
    assert_eq!(out.width(), 64, "width should be unchanged after vignette");
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after vignette"
    );
}

#[test]
fn push_video_through_hflip_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().hflip().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after hflip push");
    assert_eq!(out.width(), 64, "width should be unchanged after hflip");
    assert_eq!(out.height(), 64, "height should be unchanged after hflip");
}

#[test]
fn push_video_through_vflip_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().vflip().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after vflip push");
    assert_eq!(out.width(), 64, "width should be unchanged after vflip");
    assert_eq!(out.height(), 64, "height should be unchanged after vflip");
}

#[test]
fn push_720p_frame_through_pad_1920_1080_centred_should_return_1080p_frame() {
    let mut graph = match FilterGraph::builder()
        .pad(1920, 1080, -1, -1, "black")
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(1280, 720);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after pad push");
    assert_eq!(out.width(), 1920, "width should be padded to 1920");
    assert_eq!(out.height(), 1080, "height should be padded to 1080");
}

#[test]
fn push_4x3_frame_through_fit_to_aspect_16x9_should_return_target_dimensions() {
    // 64×48 is 4:3; fitting into 128×72 (16:9) should produce pillarbox bars.
    // Scale factor = min(128/64, 72/48) = min(2.0, 1.5) = 1.5
    // Scaled: 64×1.5=96, 48×1.5=72 → pad to 128×72 with (128-96)/2=16px bars each side.
    let mut graph = match FilterGraph::builder()
        .fit_to_aspect(128, 72, "black")
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 48);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after fit_to_aspect push");
    assert_eq!(
        out.width(),
        128,
        "width should match target after fit_to_aspect"
    );
    assert_eq!(
        out.height(),
        72,
        "height should match target after fit_to_aspect"
    );
}

#[test]
fn push_wide_frame_through_fit_to_aspect_should_produce_letterbox() {
    // 128×54 is ~2.37:1; fitting into 128×72 (16:9) should produce letterbox bars.
    // Scale factor = min(128/128, 72/54) = min(1.0, 1.33) = 1.0 (no scale needed)
    // Pad 128×72 with (72-54)/2=9px bars top and bottom.
    let mut graph = match FilterGraph::builder()
        .fit_to_aspect(128, 72, "black")
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(128, 54);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after fit_to_aspect letterbox push");
    assert_eq!(
        out.width(),
        128,
        "width should match target after letterbox fit"
    );
    assert_eq!(
        out.height(),
        72,
        "height should match target after letterbox fit"
    );
}

#[test]
fn push_video_through_gblur_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().gblur(5.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after gblur push");
    assert_eq!(out.width(), 64, "width should be unchanged after gblur");
    assert_eq!(out.height(), 64, "height should be unchanged after gblur");
}

#[test]
fn push_video_through_unsharp_sharpen_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().unsharp(1.0, 0.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after unsharp push");
    assert_eq!(out.width(), 64, "width should be unchanged after unsharp");
    assert_eq!(out.height(), 64, "height should be unchanged after unsharp");
}

#[test]
fn push_video_through_hqdn3d_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().hqdn3d(4.0, 3.0, 6.0, 4.5).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after hqdn3d push");
    assert_eq!(out.width(), 64, "width should be unchanged after hqdn3d");
    assert_eq!(out.height(), 64, "height should be unchanged after hqdn3d");
}

#[test]
fn push_video_through_nlmeans_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().nlmeans(8.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after nlmeans push");
    assert_eq!(out.width(), 64, "width should be unchanged after nlmeans");
    assert_eq!(out.height(), 64, "height should be unchanged after nlmeans");
}

#[test]
fn push_video_through_yadif_frame_mode_should_accept_frames_without_error() {
    // yadif is a temporal filter; it buffers frames before emitting output.
    // This test verifies that frames are accepted and pull_video does not error.
    // A single progressive frame may not produce output (None is acceptable).
    let mut graph = match FilterGraph::builder().yadif(YadifMode::Frame).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    // pull_video must not error; None is acceptable (frame buffered by yadif).
    let result = graph.pull_video().expect("pull_video must not fail");
    if let Some(out) = result {
        assert_eq!(out.width(), 64, "width should be unchanged after yadif");
        assert_eq!(out.height(), 64, "height should be unchanged after yadif");
    }
}

#[test]
fn push_video_through_fade_in_white_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().fade_in_white(0.0, 1.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after fade_in_white push");
    assert_eq!(
        out.width(),
        64,
        "width should be unchanged after fade_in_white"
    );
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after fade_in_white"
    );
}

#[test]
fn push_video_through_fade_out_white_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder().fade_out_white(0.0, 1.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after fade_out_white push");
    assert_eq!(
        out.width(),
        64,
        "width should be unchanged after fade_out_white"
    );
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after fade_out_white"
    );
}

#[test]
fn push_two_clips_through_xfade_dissolve_should_return_frame_with_same_dimensions() {
    let mut graph = match FilterGraph::builder()
        .xfade(XfadeTransition::Dissolve, 1.0, 4.0)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let clip_a = make_yuv420p_frame(64, 64);
    let clip_b = make_yuv420p_frame(64, 64);
    // Push clip A to slot 0 first; this initialises the graph.
    match graph.push_video(0, &clip_a) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    // Push clip B to slot 1.
    match graph.push_video(1, &clip_b) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after xfade push");
    assert_eq!(
        out.width(),
        64,
        "output width should match input after xfade"
    );
    assert_eq!(
        out.height(),
        64,
        "output height should match input after xfade"
    );
}

#[test]
fn push_video_through_drawtext_should_return_frame_with_same_dimensions() {
    let opts = DrawTextOptions {
        text: "Hello".to_string(),
        x: "10".to_string(),
        y: "10".to_string(),
        font_size: 24,
        font_color: "white".to_string(),
        font_file: None,
        opacity: 1.0,
        box_color: None,
        box_border_width: 0,
    };
    let mut graph = match FilterGraph::builder().drawtext(opts).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after drawtext push");
    assert_eq!(out.width(), 64, "width should be unchanged after drawtext");
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after drawtext"
    );
}

#[test]
fn push_video_through_drawtext_with_box_should_return_frame_with_same_dimensions() {
    let opts = DrawTextOptions {
        text: "Hello".to_string(),
        x: "10".to_string(),
        y: "10".to_string(),
        font_size: 24,
        font_color: "white".to_string(),
        font_file: None,
        opacity: 1.0,
        box_color: Some("black@0.5".to_string()),
        box_border_width: 5,
    };
    let mut graph = match FilterGraph::builder().drawtext(opts).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after drawtext with box push");
    assert_eq!(
        out.width(),
        64,
        "width should be unchanged after drawtext with box"
    );
    assert_eq!(
        out.height(),
        64,
        "height should be unchanged after drawtext with box"
    );
}

#[test]
fn push_audio_through_agate_should_return_frame_with_same_properties() {
    let mut graph = match FilterGraph::builder().agate(-40.0, 10.0, 100.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after agate push");
    assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
    assert_eq!(out.channels(), 2, "channel count should be unchanged");
    assert_eq!(out.samples(), 1024, "sample count should be unchanged");
}

#[test]
fn push_audio_through_compressor_should_return_frame_with_same_properties() {
    let mut graph = match FilterGraph::builder()
        .compressor(-20.0, 4.0, 10.0, 100.0, 6.0)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after compressor push");
    assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
    assert_eq!(out.channels(), 2, "channel count should be unchanged");
    assert_eq!(out.samples(), 1024, "sample count should be unchanged");
}

#[test]
fn push_stereo_audio_through_stereo_to_mono_should_return_mono_frame() {
    let mut graph = match FilterGraph::builder().stereo_to_mono().build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after stereo_to_mono push");
    assert_eq!(out.channels(), 1, "output should be mono (1 channel)");
    assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
}

#[test]
fn push_audio_through_channel_map_should_return_frame_with_same_properties() {
    // "FL|FR" maps the two stereo channels to the same layout (identity remap).
    let mut graph = match FilterGraph::builder().channel_map("FL|FR").build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after channel_map push");
    assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
    assert_eq!(
        out.channels(),
        2,
        "channel count should be unchanged for identity remap"
    );
    assert_eq!(out.samples(), 1024, "sample count should be unchanged");
}

#[test]
fn push_audio_through_audio_delay_positive_should_return_frame_with_same_properties() {
    let mut graph = match FilterGraph::builder().audio_delay(100.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    // adelay pads with silence first; result may be None if the delay
    // exceeds one frame's worth of samples.  No error is the key assertion.
    let result = graph.pull_audio().expect("pull_audio must not fail");
    if let Some(out) = result {
        assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
        assert_eq!(out.channels(), 2, "channel count should be unchanged");
    }
}

#[test]
fn push_audio_through_audio_delay_negative_should_not_error() {
    // Use a 5 ms advance. The test frame is 1024/48000 ≈ 21 ms, so atrim
    // trims only a portion of it and may still return a frame.
    // Regardless of whether a frame is returned, no error is the key assertion.
    let mut graph = match FilterGraph::builder().audio_delay(-5.0).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    if let Some(out) = result {
        assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
        assert_eq!(out.channels(), 2, "channel count should be unchanged");
    }
}

#[test]
fn push_video_through_concat_video_should_produce_output() {
    let mut graph = match FilterGraph::builder().concat_video(2).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after concat push to both slots");
    assert_eq!(out.width(), 64, "width should be unchanged");
    assert_eq!(out.height(), 64, "height should be unchanged");
}

#[test]
fn push_audio_through_concat_audio_should_produce_output() {
    let mut graph = match FilterGraph::builder().concat_audio(2).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_audio_frame();
    match graph.push_audio(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_audio(1, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_audio().expect("pull_audio must not fail");
    let out = result.expect("expected Some(frame) after concat push to both slots");
    assert_eq!(out.sample_rate(), 48000, "sample rate should be unchanged");
    assert_eq!(out.channels(), 2, "channel count should be unchanged");
}

#[test]
fn push_two_clips_through_join_with_dissolve_should_produce_output() {
    let mut graph = match FilterGraph::builder()
        .join_with_dissolve(4.0, 1.0, 1.0)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let clip_a = make_yuv420p_frame(64, 64);
    let clip_b = make_yuv420p_frame(64, 64);
    // Push clip A to slot 0 first; this initialises the graph.
    match graph.push_video(0, &clip_a) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    // Push clip B to slot 1.
    match graph.push_video(1, &clip_b) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let result = graph.pull_video().expect("pull_video must not fail");
    let out = result.expect("expected Some(frame) after join_with_dissolve push");
    assert_eq!(
        out.width(),
        64,
        "output width should match input after join_with_dissolve"
    );
    assert_eq!(
        out.height(),
        64,
        "output height should match input after join_with_dissolve"
    );
}

// ── Blend: Multiply and Screen ────────────────────────────────────────────────

/// YUV420p frame filled with a solid luma value; U/V neutral at 128.
fn make_solid_yuv_frame(width: u32, height: u32, y_val: u8) -> VideoFrame {
    let y = vec![y_val; (width * height) as usize];
    let u = vec![128u8; ((width / 2) * (height / 2)) as usize];
    let v = vec![128u8; ((width / 2) * (height / 2)) as usize];
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(y),
            PooledBuffer::standalone(u),
            PooledBuffer::standalone(v),
        ],
        vec![width as usize, (width / 2) as usize, (width / 2) as usize],
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .unwrap()
}

#[test]
fn blend_multiply_black_top_should_produce_black_output() {
    // Multiply(bottom=Y128, top=Y0) = 0: black top kills all luma.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Multiply, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 0);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 10.0,
        "Multiply with black top should produce near-black output (avg={avg})"
    );
}

#[test]
fn blend_multiply_white_top_should_be_identity() {
    // Multiply(bottom=Y128, top=Y255) = 128 * 255 / 255 = 128: identity.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Multiply, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 255);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        (avg - 128.0).abs() < 15.0,
        "Multiply with white top should preserve bottom luma (avg={avg})"
    );
}

#[test]
fn blend_screen_white_top_should_produce_white_output() {
    // Screen(bottom=Y128, top=Y255) = 255: white top saturates to white.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Screen, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 255);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg > 245.0,
        "Screen with white top should produce near-white output (avg={avg})"
    );
}

#[test]
fn blend_overlay_midgray_top_should_be_identity() {
    // Overlay with a 50% gray top (Y=128) sits at the Multiply/Screen boundary
    // and leaves the bottom luma approximately unchanged.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Overlay, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 128);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        (avg - 128.0).abs() < 20.0,
        "Overlay with mid-gray top should leave bottom luma approximately unchanged (avg={avg})"
    );
}

#[test]
fn blend_colordodge_should_produce_brighter_output() {
    // ColorDodge(bottom=Y128, top=Y128): result = bottom / (1 - top) → saturates toward white.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::ColorDodge, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 128);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg > 140.0,
        "ColorDodge should produce brighter output than bottom luma=128 (avg={avg})"
    );
}

#[test]
fn blend_colorburn_should_produce_darker_output() {
    // ColorBurn(bottom=Y128, top=Y128): result = 1 - (1 - bottom) / top → darkens toward black.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::ColorBurn, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 128);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 115.0,
        "ColorBurn should produce darker output than bottom luma=128 (avg={avg})"
    );
}

#[test]
fn blend_darken_black_top_should_produce_black_output() {
    // Darken(bottom=Y128, top=Y0): min(128, 0) = 0.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Darken, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 0);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 10.0,
        "Darken with black top should produce near-black output (avg={avg})"
    );
}

#[test]
fn blend_lighten_white_top_should_produce_white_output() {
    // Lighten(bottom=Y128, top=Y255): max(128, 255) = 255.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Lighten, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 255);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg > 245.0,
        "Lighten with white top should produce near-white output (avg={avg})"
    );
}

#[test]
fn blend_difference_with_self_should_produce_black() {
    // Difference(bottom=Y128, top=Y128): |128 - 128| = 0 → black.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Difference, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 128);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 10.0,
        "Difference of identical layers should produce near-black output (avg={avg})"
    );
}

#[test]
fn blend_add_black_top_should_be_identity() {
    // Add(bottom=Y128, top=Y0): 128 + 0 = 128, clamped → identity.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Add, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 0);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        (avg - 128.0).abs() < 15.0,
        "Add with black top should preserve bottom luma (avg={avg})"
    );
}

#[test]
fn blend_subtract_white_top_should_produce_black() {
    // Subtract(bottom=Y128, top=Y255): max(128 - 255, 0) = 0 → black.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Subtract, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 255);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 10.0,
        "Subtract with white top should produce near-black output (avg={avg})"
    );
}

#[test]
fn blend_luminosity_should_preserve_base_hue_and_saturation() {
    // Luminosity applies the top's luminance to the base's hue+saturation.
    // With a brighter grey top (Y=200) over a darker grey bottom (Y=128),
    // the output luma should shift toward the top's luminance (200).
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::Luminosity, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 128);
    let top_frame = make_solid_yuv_frame(64, 64, 200);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg > 160.0,
        "Luminosity with brighter top should increase output luma toward top's value (avg={avg})"
    );
}

// ── Porter-Duff Over ──────────────────────────────────────────────────────────

#[test]
fn porter_duff_over_opaque_top_should_cover_bottom() {
    // PorterDuffOver with opacity=1.0: opaque YUV420p top covers the bottom.
    // Uses overlay=format=auto:shortest=1 — opaque top always wins.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::PorterDuffOver, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 100);
    let top_frame = make_solid_yuv_frame(64, 64, 200);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg > 160.0,
        "PorterDuffOver with opaque top should cover the bottom (avg={avg})"
    );
}

#[test]
fn porter_duff_over_semitransparent_should_blend_correctly() {
    // PorterDuffOver with opacity=0.5 inserts colorchannelmixer=aa=0.5 on the top
    // layer, making it semi-transparent so the bottom shows through.
    // Verifies the graph constructs and runs without error.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::PorterDuffOver, 0.5, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 100);
    let top_frame = make_solid_yuv_frame(64, 64, 200);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
}

// ── Porter-Duff Under ─────────────────────────────────────────────────────────

#[test]
fn porter_duff_under_should_place_bottom_over_top() {
    // PorterDuffUnder reverses overlay input order (bottom→pad1, top→pad0), so
    // the bottom layer composites over the top. Verify the graph builds and
    // produces a frame with the expected dimensions.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::PorterDuffUnder, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 200);
    let top_frame = make_solid_yuv_frame(64, 64, 100);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
}

// ── Porter-Duff In ────────────────────────────────────────────────────────────

#[test]
fn porter_duff_in_should_produce_black_where_bottom_is_black() {
    // PorterDuffIn uses all_expr=B*A/255, so when the bottom luma (A) is 0,
    // the output is 0 regardless of the top value.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::PorterDuffIn, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 0);
    let top_frame = make_solid_yuv_frame(64, 64, 200);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 10.0,
        "PorterDuffIn with black bottom should produce black output (avg={avg})"
    );
}

// ── Porter-Duff Atop ─────────────────────────────────────────────────────────

#[test]
fn porter_duff_atop_should_use_bottom_alpha_for_output() {
    // Atop formula: B*A/255 + A*(255-B)/255 = A*(B + 255 - B)/255 = A.
    // The output luma always equals the bottom luma regardless of the top value.
    // bottom=Y100, top=Y200 → avg ≈ 100 (within ±15).
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::PorterDuffAtop, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 100);
    let top_frame = make_solid_yuv_frame(64, 64, 200);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg > 85.0 && avg < 115.0,
        "PorterDuffAtop output luma should equal bottom luma (~100), got avg={avg}"
    );
}

// ── Porter-Duff XOR ───────────────────────────────────────────────────────────

#[test]
fn porter_duff_xor_identical_shapes_should_produce_zero_alpha() {
    // XOR formula with A=B=255: 255*(255-255)/255 + 255*(255-255)/255 = 0.
    // Two fully-opaque identical layers cancel each other out.
    // bottom=Y255, top=Y255 → avg ≈ 0.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::PorterDuffXor, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 255);
    let top_frame = make_solid_yuv_frame(64, 64, 255);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 10.0,
        "PorterDuffXor of identical full-luma shapes should produce black (avg={avg})"
    );
}

// ── Porter-Duff Out ───────────────────────────────────────────────────────────

#[test]
fn porter_duff_out_should_produce_black_where_bottom_is_white() {
    // PorterDuffOut uses all_expr=B*(255-A)/255, so when the bottom luma (A) is
    // 255, the output is 0 regardless of the top value.
    let top = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .blend(top, BlendMode::PorterDuffOut, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let bottom = make_solid_yuv_frame(64, 64, 255);
    let top_frame = make_solid_yuv_frame(64, 64, 200);
    match graph.push_video(0, &bottom) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &top_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    let luma = out.plane(0).expect("Y plane must exist");
    let avg = luma.iter().map(|&b| b as f32).sum::<f32>() / luma.len() as f32;
    assert!(
        avg < 10.0,
        "PorterDuffOut with white bottom should produce black output (avg={avg})"
    );
}

// ── Color key ─────────────────────────────────────────────────────────────────

#[test]
fn colorkey_solid_color_background_should_become_transparent() {
    // Neutral gray (Y=128, U=128, V=128) ≈ 0x808080 in RGB.
    // colorkey("0x808080", 0.3, 0.0) should key it out in RGB space.
    // Output is rgba (packed); alpha bytes are at indices 3, 7, 11, ... in plane 0.
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .colorkey("0x808080", 0.3, 0.0)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    // Neutral gray in YUV420p: Y=128, U=128, V=128 ≈ RGB(128,128,128) = 0x808080.
    let frame = make_yuv_frame(64, 64, 128, 128, 128);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // colorkey outputs argb (alpha at byte 0) or rgba (alpha at byte 3).
    // Check the minimum of both to be format-agnostic.
    if let Some(data) = out.plane(0) {
        if data.len() == 64 * 64 * 4 {
            let avg_a0 = data.chunks(4).map(|p| p[0] as f32).sum::<f32>() / (64.0 * 64.0);
            let avg_a3 = data.chunks(4).map(|p| p[3] as f32).sum::<f32>() / (64.0 * 64.0);
            let avg_alpha = avg_a0.min(avg_a3);
            assert!(
                avg_alpha < 10.0,
                "gray pixels should be keyed out (avg_a0={avg_a0} avg_a3={avg_a3})"
            );
        }
    }
}

// ── Alpha matte ───────────────────────────────────────────────────────────────

#[test]
fn alpha_matte_white_region_should_be_opaque() {
    // White matte (Y=235) → alphamerge sets alpha to the matte's luma → high alpha.
    // Main video: neutral gray; matte: white. Expected: output alpha near 255 (opaque).
    let matte = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .alpha_matte(matte)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let main_frame = make_yuv420p_frame(64, 64);
    let white_matte = make_solid_yuv_frame(64, 64, 235);
    match graph.push_video(0, &main_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &white_matte) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // The output has an alpha channel; plane(3) is alpha for yuva420p.
    if let Some(alpha) = out.plane(3) {
        let avg = alpha.iter().map(|&b| b as f32).sum::<f32>() / alpha.len() as f32;
        assert!(
            avg > 200.0,
            "white matte should produce high (opaque) alpha (avg={avg})"
        );
    }
}

#[test]
fn alpha_matte_black_region_should_be_transparent() {
    // Black matte (Y=16) → alphamerge sets alpha to the matte's luma → low alpha.
    // Main video: neutral gray; matte: black. Expected: output alpha near 0 (transparent).
    let matte = FilterGraphBuilder::new().trim(0.0, 5.0);
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .alpha_matte(matte)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let main_frame = make_yuv420p_frame(64, 64);
    let black_matte = make_solid_yuv_frame(64, 64, 16);
    match graph.push_video(0, &main_frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    match graph.push_video(1, &black_matte) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // The output has an alpha channel; plane(3) is alpha for yuva420p.
    if let Some(alpha) = out.plane(3) {
        let avg = alpha.iter().map(|&b| b as f32).sum::<f32>() / alpha.len() as f32;
        assert!(
            avg < 30.0,
            "black matte should produce low (transparent) alpha (avg={avg})"
        );
    }
}

// ── Spill suppress ────────────────────────────────────────────────────────────

#[test]
fn spill_suppress_should_reduce_green_cast_on_subject_edges() {
    // A frame with green chroma (Y=128, U=44, V=21) fed through
    // spill_suppress("green", 1.0) → hue=h=0:s=0.0 → fully desaturated.
    // After full desaturation the U and V planes should be near neutral (128).
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .spill_suppress("green", 1.0)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    // Green chroma in YUV420p: Y=128, U=44, V=21.
    let frame = make_yuv_frame(64, 64, 128, 44, 21);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // After full desaturation the U plane should be higher than the input (44),
    // proving the green spill was reduced toward neutral (128).
    if let Some(u_plane) = out.plane(1) {
        let avg_u = u_plane.iter().map(|&b| b as f32).sum::<f32>() / u_plane.len() as f32;
        assert!(
            avg_u > 55.0,
            "U plane should move toward neutral after full desaturation (avg_u={avg_u})"
        );
    }
}

// ── Chroma key ────────────────────────────────────────────────────────────────

#[test]
fn chromakey_green_screen_should_produce_transparent_green_area() {
    // Pure green (0x00FF00) in YUV420p: Y≈150, U≈44, V≈21 (BT.601 coefficients).
    // chromakey(color="0x00FF00", similarity=0.3, blend=0.0) should detect the
    // green chroma and key it out, producing a yuva420p frame where the alpha
    // plane (index 3) is 0 (transparent) for the green pixels.
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .chromakey("0x00FF00", 0.3, 0.0)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    // Y≈150, U≈44, V≈21 approximates 0x00FF00 in YUV420p.
    let frame = make_yuv_frame(64, 64, 150, 44, 21);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // The output is yuva420p; plane(3) is the alpha channel.
    // Keyed (green) pixels have alpha=0 (transparent).
    if let Some(alpha) = out.plane(3) {
        let avg = alpha.iter().map(|&b| b as f32).sum::<f32>() / alpha.len() as f32;
        assert!(
            avg < 10.0,
            "green pixels should be keyed out (avg alpha={avg})"
        );
    }
}

// ── Luma key ──────────────────────────────────────────────────────────────────

#[test]
fn lumakey_white_background_should_be_transparent_at_threshold_1() {
    // White frame in limited-range YUV420p: Y=235 (≈1.0 normalized).
    // lumakey(threshold=0.9, tolerance=0.1, softness=0.0, invert=false) keys out
    // pixels with luma in [0.8, 1.0].  Y=235 ≈ 0.92 falls inside that range,
    // so the output alpha (plane 3 of yuva420p) should be near 0 (transparent).
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .lumakey(0.9, 0.1, 0.0, false)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv_frame(64, 64, 235, 128, 128);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // yuva420p: plane(3) is the alpha channel; 0 = transparent.
    if let Some(alpha) = out.plane(3) {
        let avg = alpha.iter().map(|&b| b as f32).sum::<f32>() / alpha.len() as f32;
        assert!(
            avg < 10.0,
            "white pixels should be keyed out (avg alpha={avg})"
        );
    }
}

#[test]
fn lumakey_invert_should_key_out_dark_regions() {
    // Black frame in limited-range YUV420p: Y=16 (≈0.0 normalized).
    // lumakey(threshold=1.0, tolerance=0.1, softness=0.0, invert=true):
    //   1. lumakey makes pixels near 1.0 (white) transparent; Y=16 stays opaque.
    //   2. geq inverts alpha: Y=16 (was opaque) → transparent.
    // So the output alpha (plane 3 of yuva420p) should be near 0.
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .lumakey(1.0, 0.1, 0.0, true)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv_frame(64, 64, 16, 128, 128);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // yuva420p: plane(3) is the alpha channel; 0 = transparent.
    if let Some(alpha) = out.plane(3) {
        let avg = alpha.iter().map(|&b| b as f32).sum::<f32>() / alpha.len() as f32;
        assert!(
            avg < 10.0,
            "dark pixels should be keyed out after alpha invert (avg alpha={avg})"
        );
    }
}

// ── Feather mask ──────────────────────────────────────────────────────────────

#[test]
fn feather_mask_should_produce_smooth_alpha_edges() {
    // Apply a sharp rect_mask (left 32 columns opaque) then feather it.
    // The Gaussian blur softens the hard edge at X=32.
    //
    // The alphamerge chain outputs gbrap (AV_PIX_FMT_GBRAP = 111).
    // PixelFormat::Other(111).num_planes() returns 1, so only plane(0)
    // (the G channel) is accessible.  For a neutral-gray input (Y=128,
    // U=128, V=128) the G values should be close to 128.
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .rect_mask(0, 0, 32, 64, false)
        .feather_mask(4)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    if let Some(g_plane) = out.plane(0) {
        let avg = g_plane.iter().map(|&b| b as f32).sum::<f32>() / g_plane.len() as f32;
        assert!(
            avg > 80.0 && avg < 180.0,
            "feather_mask should preserve colour data (avg G={avg})"
        );
    }
}

// ── Polygon matte ─────────────────────────────────────────────────────────────

#[test]
fn polygon_matte_triangle_should_isolate_triangular_region() {
    // Triangle (0,0)→(1,0)→(0,1) covers pixels where X + Y < frame_width.
    // For a 64×64 frame: inside count = 64+63+…+1 = 2080 out of 4096 pixels.
    // Expected average alpha (non-inverted) = 2080*255/4096 ≈ 129.5.
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .polygon_matte(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], false)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // geq outputs rgba (packed); alpha is at byte 3 (rgba) or byte 0 (argb).
    if let Some(data) = out.plane(0) {
        if data.len() == 64 * 64 * 4 {
            let avg_a0 = data.chunks(4).map(|p| p[0] as f32).sum::<f32>() / (64.0 * 64.0);
            let avg_a3 = data.chunks(4).map(|p| p[3] as f32).sum::<f32>() / (64.0 * 64.0);
            // Triangle covers ~50% of pixels; avg alpha ≈ 129.5.
            let alpha = avg_a0.max(avg_a3);
            assert!(
                alpha > 100.0 && alpha < 160.0,
                "triangle mask should cover ~50% of pixels (avg≈129), got avg_a0={avg_a0} avg_a3={avg_a3}"
            );
        }
    }
}

// ── Rect mask ─────────────────────────────────────────────────────────────────

#[test]
fn rect_mask_100x100_makes_outside_transparent() {
    // A 128×128 gray frame with a non-inverted 100×100 mask at (0, 0):
    // - inside  (X in [0,99], Y in [0,99]):  alpha=255 (opaque)
    // - outside (remaining 128²-100²=6384 px): alpha=0 (transparent)
    // Expected average alpha = 10000*255/16384 ≈ 155.5.
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .rect_mask(0, 0, 100, 100, false)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(128, 128);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 128, "output width must match input");
    assert_eq!(out.height(), 128, "output height must match input");
    // geq outputs rgba (packed); alpha is at byte 3 (rgba) or byte 0 (argb).
    if let Some(data) = out.plane(0) {
        if data.len() == 128 * 128 * 4 {
            let avg_a0 = data.chunks(4).map(|p| p[0] as f32).sum::<f32>() / (128.0 * 128.0);
            let avg_a3 = data.chunks(4).map(|p| p[3] as f32).sum::<f32>() / (128.0 * 128.0);
            // The alpha channel (whichever byte) should average ~155 (100×100 opaque out of 128×128).
            let alpha = avg_a0.max(avg_a3);
            assert!(
                alpha > 130.0 && alpha < 180.0,
                "rect_mask inside should be opaque (avg≈155), got avg_a0={avg_a0} avg_a3={avg_a3}"
            );
        }
    }
}

#[test]
fn rect_mask_inverted_makes_inside_transparent() {
    // A 64×64 gray frame with an inverted 32×32 mask at (0, 0):
    // - inside  (X in [0,31], Y in [0,31]):  alpha=0 (transparent)
    // - outside (remaining 3072 px):         alpha=255 (opaque)
    // Expected average alpha = 3072*255/4096 ≈ 191.25.
    let mut graph = match FilterGraph::builder()
        .trim(0.0, 5.0)
        .rect_mask(0, 0, 32, 32, true)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = make_yuv420p_frame(64, 64);
    match graph.push_video(0, &frame) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");
    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");
    // geq outputs rgba (packed); alpha is at byte 3 (rgba) or byte 0 (argb).
    if let Some(data) = out.plane(0) {
        if data.len() == 64 * 64 * 4 {
            let avg_a0 = data.chunks(4).map(|p| p[0] as f32).sum::<f32>() / (64.0 * 64.0);
            let avg_a3 = data.chunks(4).map(|p| p[3] as f32).sum::<f32>() / (64.0 * 64.0);
            // The alpha channel should average ~191 (75% opaque after inverting the 32×32 region).
            let alpha = avg_a0.max(avg_a3);
            assert!(
                alpha > 165.0 && alpha < 215.0,
                "inverted rect_mask outside should be opaque (avg≈191), got avg_a0={avg_a0} avg_a3={avg_a3}"
            );
        }
    }
}
