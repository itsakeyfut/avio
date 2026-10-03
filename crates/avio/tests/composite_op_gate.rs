//! The engine refuses `In`/`Out`/`Atop`/`Xor` wherever only the CPU compositor could
//! render them (#1753, ADR-0014).
//!
//! The filter path cannot carry the backdrop's alpha, so for these four operators it
//! would compute per-channel arithmetic under the operator's name. Rather than ship
//! that silently, `ff-filter` refuses to build them, and the engine surfaces the
//! refusal in both places a user meets it:
//!
//! - **export** on the CPU route propagates the composer's build error as
//!   `TimelineError::Filter(FilterError::UnsupportedCompositeOp { .. })`;
//! - **preview** is refused at `TimelinePlayer::open` with
//!   `PreviewError::NeedsGpuCompositor` whenever no GPU compositor could be attached,
//!   because the runner's own reaction to a compositor build failure is to show the
//!   base frame with the layer missing, which would still be silent.
//!
//! Probe-gated on the environment only: the fixture encode and the source probe may
//! be unavailable on a minimal `FFmpeg`, and the GPU-attached case needs an adapter.
//! The refusals themselves are matched by **variant**, never through a catch-all
//! "filter error means skip" predicate, so a missing gate fails loudly.

#![cfg(feature = "preview")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::path::{Path, PathBuf};

use avio::{
    Clip, CompositeOp, EncoderConfig, FilterError, PreviewError, Timeline, TimelineError,
    TimelinePlayer, Track,
};
use ff_encode::{BitrateMode, VideoCodec};
use ff_format::Rational;
use fixtures::{FileGuard, make_source_file, test_output_path};

const CANVAS: u32 = 64;
const FPS: Rational = Rational::new(30, 1);
const FRAMES: usize = 10;

/// Two encoded sources for a base track and an overlay track, or `None` when this
/// environment cannot encode (skip). Distinct names per test keep the suite safe at
/// default parallelism.
fn sources(tag: &str) -> Option<(PathBuf, PathBuf, FileGuard, FileGuard)> {
    let base = test_output_path(&format!("composite_gate_{tag}_base.mp4"));
    let over = test_output_path(&format!("composite_gate_{tag}_over.mp4"));
    let gb = FileGuard::new(base.clone());
    let go = FileGuard::new(over.clone());
    make_source_file(&base, CANVAS, CANVAS, FPS.as_f64(), FRAMES, 200, 128, 128)?;
    make_source_file(&over, CANVAS, CANVAS, FPS.as_f64(), FRAMES, 60, 128, 128)?;
    Some((base, over, gb, go))
}

/// A base track under an overlay track whose clip carries `op`.
fn stacked(base: &Path, over: &Path, op: CompositeOp) -> Timeline {
    Timeline::builder()
        .canvas(CANVAS, CANVAS)
        .frame_rate(FPS)
        .video_track(vec![Clip::new(base)])
        .video_track(vec![Clip::new(over).with_composite_op(op)])
        .build()
        .expect("timeline build failed")
}

fn export_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Cbr(400_000))
        .build()
}

#[test]
fn export_forcing_cpu_should_refuse_an_atop_overlay() {
    let Some((base, over, _gb, _go)) = sources("export_atop") else {
        return; // no encoder
    };
    let out = test_output_path("composite_gate_export_atop_out.mp4");
    let _gout = FileGuard::new(out.clone());
    let timeline = stacked(&base, &over, CompositeOp::Atop);

    match timeline.render_forcing_cpu(&out, export_config()) {
        Err(TimelineError::Filter(FilterError::UnsupportedCompositeOp { op })) => {
            assert_eq!(op, CompositeOp::Atop, "the refusal must name the operator");
        }
        // The composer is built after the decoders open, so a decode or encode
        // failure means this environment never reached the gate.
        Err(TimelineError::Decode(e)) => println!("skipping: decoder unavailable: {e}"),
        Err(TimelineError::Encode(e)) => println!("skipping: encoder unavailable: {e}"),
        Err(e) => panic!("an Atop overlay must be refused as UnsupportedCompositeOp, got {e}"),
        Ok(()) => panic!("an Atop overlay must not render on the CPU route"),
    }
}

#[test]
fn export_forcing_cpu_should_still_accept_over_and_under() {
    // The gate is selective: the two operators the filter path implements correctly
    // must not be caught by it. `Ok` and an environment error are both
    // acceptable here; only the refusal variant is a failure.
    let Some((base, over, _gb, _go)) = sources("export_under") else {
        return;
    };
    for (tag, op) in [("over", CompositeOp::Over), ("under", CompositeOp::Under)] {
        let out = test_output_path(&format!("composite_gate_export_{tag}_out.mp4"));
        let _gout = FileGuard::new(out.clone());
        let result = stacked(&base, &over, op).render_forcing_cpu(&out, export_config());
        assert!(
            !matches!(
                result,
                Err(TimelineError::Filter(
                    FilterError::UnsupportedCompositeOp { .. }
                ))
            ),
            "{op:?} is implemented on the filter path and must not be refused"
        );
    }
}

#[test]
fn preview_open_forcing_cpu_should_refuse_an_atop_overlay() {
    let Some((base, over, _gb, _go)) = sources("preview_cpu_atop") else {
        return;
    };
    let timeline = stacked(&base, &over, CompositeOp::Atop);

    match TimelinePlayer::open_forcing_cpu(&timeline) {
        Err(PreviewError::NeedsGpuCompositor { reason }) => {
            assert!(
                reason.contains("Atop"),
                "the refusal must name the operator, got: {reason}"
            );
            assert!(
                reason.contains("track 1"),
                "the refusal must locate the clip, got: {reason}"
            );
        }
        // `ScenePlayer::open` probes the sources before the engine's check runs.
        Err(PreviewError::Probe(e)) => println!("skipping: probe unavailable: {e}"),
        Err(PreviewError::Decode(e)) => println!("skipping: decoder unavailable: {e}"),
        Err(e) => {
            panic!("forced-CPU preview of an Atop overlay must be NeedsGpuCompositor, got {e}")
        }
        Ok(_) => panic!("forced-CPU preview of an Atop overlay must not open"),
    }
}

#[cfg(feature = "gpu")]
#[test]
fn preview_open_should_accept_an_atop_overlay_when_a_gpu_is_attached() {
    // With an adapter the GPU compositor renders the operator correctly, so the
    // open-time refusal must not fire. Adapter-gated.
    if avio::GpuCompositor::new().is_none() {
        return; // no adapter
    }
    let Some((base, over, _gb, _go)) = sources("preview_gpu_atop") else {
        return;
    };
    let timeline = stacked(&base, &over, CompositeOp::Atop);

    match TimelinePlayer::open(&timeline) {
        Ok(_) => {}
        Err(PreviewError::NeedsGpuCompositor { reason }) => {
            panic!("an attached GPU must lift the open-time refusal, got: {reason}")
        }
        Err(PreviewError::Probe(e)) => println!("skipping: probe unavailable: {e}"),
        Err(PreviewError::Decode(e)) => println!("skipping: decoder unavailable: {e}"),
        Err(e) => panic!("unexpected open error: {e}"),
    }
}

#[test]
fn preview_open_forcing_cpu_should_ignore_an_atop_clip_on_an_inactive_track() {
    // The derivation drops a disabled, muted or solo-shadowed track, so the runner
    // never builds that layer and the open-time check must apply the same rule.
    // Each shape builds a fresh timeline: the base track is soloed for the third,
    // which shadows the overlay track carrying the operator.
    let Some((base, over, _gb, _go)) = sources("preview_cpu_inactive") else {
        return;
    };
    let shapes: [(&str, fn(Track) -> Track, fn(Track) -> Track); 3] = [
        ("disabled", |b| b, |o| o.enabled(false)),
        ("muted", |b| b, |o| o.muted(true)),
        ("solo-shadowed", |b| b.soloed(true), |o| o),
    ];
    for (shape, base_track, over_track) in shapes {
        let timeline = Timeline::builder()
            .canvas(CANVAS, CANVAS)
            .frame_rate(FPS)
            .video_track_with(base_track(Track::new(vec![Clip::new(&base)])))
            .video_track_with(over_track(Track::new(vec![
                Clip::new(&over).with_composite_op(CompositeOp::Atop),
            ])))
            .build()
            .expect("timeline build failed");
        let result = TimelinePlayer::open_forcing_cpu(&timeline);
        assert!(
            !matches!(result, Err(PreviewError::NeedsGpuCompositor { .. })),
            "a {shape} track contributes no layer, so its operator must not be refused"
        );
    }
}

#[test]
fn preview_open_forcing_cpu_should_still_accept_under() {
    let Some((base, over, _gb, _go)) = sources("preview_cpu_under") else {
        return;
    };
    let result = TimelinePlayer::open_forcing_cpu(&stacked(&base, &over, CompositeOp::Under));
    assert!(
        !matches!(result, Err(PreviewError::NeedsGpuCompositor { .. })),
        "Under is implemented on the filter path and must not be refused at open"
    );
}
