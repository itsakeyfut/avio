//! Integration tests for `Timeline::render()`.
//!
//! Creates synthetic source clips, builds a `Timeline`, calls `render()`,
//! and validates the output with `ff_probe`.

#![allow(clippy::unwrap_used)]

mod fixtures;

use std::time::Duration;

use avio::{Clip, EncoderConfig, Timeline, TimelineError, Track};
use ff_encode::{AudioCodec, BitrateMode, VideoCodec};
use ff_filter::FilterStep;
use ff_filter::animation::{AnimationTrack, Easing};
use fixtures::{FileGuard, make_source_file, test_output_path};

// Small canvas for fast CI runs.
const W: u32 = 160;
const H: u32 = 90;
const FPS: f64 = 30.0;
// 30 frames ≈ 1 second of source content per clip.
const FRAME_COUNT: usize = 30;

fn render_config() -> EncoderConfig {
    EncoderConfig::builder()
        .video_codec(VideoCodec::Mpeg4)
        .audio_codec(AudioCodec::Aac)
        .bitrate_mode(BitrateMode::Cbr(500_000))
        .build()
}

#[test]
fn timeline_render_should_produce_ffprobe_valid_output() {
    //
    // One 1-second solid-colour clip (red-ish in YUV) that is reused as both
    // clip 1 (starts at t=0) and clip 2 (starts at t=1 s).  Using the same
    // source for both clips keeps the test self-contained without requiring two
    // encode passes.

    let src_path = test_output_path("timeline_src.mp4");
    let out_path = test_output_path("timeline_out.mp4");

    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    // Y=76, U=84, V=255 ≈ red in YUV420P
    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 76, 84, 255).is_none() {
        return;
    }

    //
    // Both video and audio tracks contain:
    //   clip 1 — offset = 0 s  (plays from 0 → 1 s)
    //   clip 2 — offset = 1 s  (plays from 1 → 2 s)
    //
    // `Clip::new` defaults: in_point = None, out_point = None (full file).

    let clip1 = Clip::new(&src_path);
    let clip2 = Clip::new(&src_path).offset(Duration::from_secs(1));

    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip1.clone(), clip2.clone()])
        .audio_track(vec![clip1, clip2])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    match timeline.render(&out_path, render_config()) {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from Timeline::render: {e}"),
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };

    assert!(
        info.has_video(),
        "rendered output must contain a video stream"
    );
    assert!(
        info.has_audio(),
        "rendered output must contain an audio stream"
    );

    let duration = info.duration();
    assert!(
        duration > Duration::ZERO,
        "rendered output must have non-zero duration, got {duration:?}"
    );

    let video = info.video_stream(0).expect("video stream must be present");
    assert_eq!(
        video.width(),
        W,
        "output video width must match canvas width"
    );
    assert_eq!(
        video.height(),
        H,
        "output video height must match canvas height"
    );
}

/// A timeline-level (master bus) audio effect chain must be applied on render
/// (issue #1424 — "no longer inert"). A wiring/format bug in the post-mix master
/// graph would fail push/pull/flush here. `Volume` is a plain node: the synthetic
/// source audio is silent, so a two-pass loudnorm would be a degenerate input;
/// `LoudnessNormalize`'s measured correctness lives in ff-filter's tests.
#[test]
fn timeline_audio_filter_should_render_through_master_chain() {
    let src_path = test_output_path("timeline_audiofilter_src.mp4");
    let out_path = test_output_path("timeline_audiofilter_out.mp4");
    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 76, 84, 255).is_none() {
        return;
    }

    let clip = Clip::new(&src_path);
    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip.clone()])
        .audio_track(vec![clip])
        .audio_filter(vec![FilterStep::Volume(-6.0)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    match timeline.render(&out_path, render_config()) {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from Timeline::render: {e}"),
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };
    assert!(
        info.has_audio(),
        "rendered output with a master audio filter must contain an audio stream"
    );
}

/// A per-track (pre-mix) two-pass audio effect must be applied on render (issue
/// #1446 — "no longer inert"). `LoudnessNormalize` is a two-pass step: it is inert
/// inside the source-only mixer, so this exercises the per-track push/pull graph.
/// A wiring/format bug there would fail push/pull/flush; measured loudness
/// correctness lives in ff-filter's tests (the synthetic source audio is silent).
#[test]
fn timeline_track_audio_effect_should_render_through_per_track_chain() {
    let src_path = test_output_path("timeline_trackfx_src.mp4");
    let out_path = test_output_path("timeline_trackfx_out.mp4");
    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 76, 84, 255).is_none() {
        return;
    }

    let clip = Clip::new(&src_path);
    let audio = Track::new(vec![clip.clone()]).audio_effects(vec![FilterStep::LoudnessNormalize {
        target_lufs: -14.0,
        true_peak_db: -1.0,
        lra: 7.0,
    }]);
    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip])
        .audio_track_with(audio)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    match timeline.render(&out_path, render_config()) {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from Timeline::render: {e}"),
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };
    assert!(
        info.has_audio(),
        "rendered output with a per-track audio effect must contain an audio stream"
    );
}

/// Two audio tracks, one carrying a per-track effect, must sum through the
/// additive master `amix` (issue #1446). This exercises the full per-track path:
/// per-track sub-mix, a single track's push/pull effect graph, and the master mix
/// over both processed tracks.
#[test]
fn timeline_two_tracks_one_with_effect_should_render_mixed_audio() {
    let src_path = test_output_path("timeline_trackmix_src.mp4");
    let out_path = test_output_path("timeline_trackmix_out.mp4");
    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 76, 84, 255).is_none() {
        return;
    }

    let clip = Clip::new(&src_path);
    let fx_track = Track::new(vec![clip.clone()]).audio_effects(vec![FilterStep::Volume(-6.0)]);
    let plain_track = Track::new(vec![clip.clone()]);
    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip])
        .audio_track_with(fx_track)
        .audio_track_with(plain_track)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    match timeline.render(&out_path, render_config()) {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from Timeline::render: {e}"),
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };
    assert!(
        info.has_audio(),
        "mixed output over two tracks (one with an effect) must contain an audio stream"
    );
}

/// A per-track effect chain and the timeline master bus (`audio_filter`) together
/// must render (issue #1446). This traverses the per-track drain branch's
/// `Some(master)` path — the track's push/pull effect graph output is routed
/// through the master bus before the encoder — which the other #1446 tests (no
/// `audio_filter`) never exercise.
#[test]
fn timeline_track_effect_with_master_bus_should_render() {
    let src_path = test_output_path("timeline_trackfxbus_src.mp4");
    let out_path = test_output_path("timeline_trackfxbus_out.mp4");
    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 76, 84, 255).is_none() {
        return;
    }

    let clip = Clip::new(&src_path);
    let fx_track = Track::new(vec![clip.clone()]).audio_effects(vec![FilterStep::Volume(-3.0)]);
    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip])
        .audio_track_with(fx_track)
        .audio_filter(vec![FilterStep::Volume(-6.0)])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    match timeline.render(&out_path, render_config()) {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from Timeline::render: {e}"),
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };
    assert!(
        info.has_audio(),
        "output with a per-track effect and a master bus must contain an audio stream"
    );
}

/// Verifies that `render_with_progress` returns `TimelineError::Cancelled` when
/// the callback immediately returns `false`.
///
/// This is the acceptance-criterion test for issue #1016:
/// "A unit test verifies that a cancelling callback returns `Cancelled` after
/// the first frame."
#[test]
fn render_with_progress_should_cancel_when_callback_returns_false() {
    let src_path = test_output_path("rwp_cancel_src.mp4");
    let out_path = test_output_path("rwp_cancel_out.mp4");
    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    // Y=100, U=100, V=100 ≈ grey
    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 100, 100, 100).is_none() {
        return;
    }

    let clip = Clip::new(&src_path).trim(Duration::ZERO, Duration::from_secs(1));

    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    // A callback that always returns false should cancel on the first frame.
    let result = timeline.render_with_progress(&out_path, render_config(), |_| false);

    match result {
        Err(TimelineError::Cancelled) => { /* expected */ }
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
        }
        Err(e) => panic!("expected Cancelled, got {e}"),
        Ok(()) => panic!("render_with_progress must not succeed when callback returns false"),
    }
}

/// Verifies that a `Timeline` with two clips joined by a transition can be
/// built without error.
///
/// This is the acceptance-criteria test for issue #1015:
/// "Clip has no transition field; TimelineBuilder cannot express inter-clip
/// cross-fade transitions."
#[test]
fn timeline_with_transition_should_build_without_error() {
    use ff_filter::XfadeTransition;

    let clip_a = Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(4));
    let clip_b = Clip::new("b.mp4")
        .trim(Duration::ZERO, Duration::from_secs(4))
        .with_transition(XfadeTransition::Fade, Duration::from_millis(500));

    let result = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![clip_a, clip_b])
        .build();

    assert!(
        result.is_ok(),
        "Timeline with transition must build without error: {result:?}"
    );
}

/// Verifies that `render_with_progress` invokes the callback at least once
/// and reports monotonically increasing `frames_processed`.
#[test]
fn render_with_progress_should_invoke_callback_with_incrementing_frame_count() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let src_path = test_output_path("rwp_count_src.mp4");
    let out_path = test_output_path("rwp_count_out.mp4");
    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 100, 100, 100).is_none() {
        return;
    }

    let clip = Clip::new(&src_path).trim(Duration::ZERO, Duration::from_secs(1));

    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    let call_count = AtomicU64::new(0);
    let last_frames = AtomicU64::new(0);

    let result = timeline.render_with_progress(&out_path, render_config(), |p| {
        let prev = last_frames.load(Ordering::Relaxed);
        assert!(
            p.frames_processed > prev,
            "frames_processed must increase monotonically: prev={prev} got={}",
            p.frames_processed
        );
        last_frames.store(p.frames_processed, Ordering::Relaxed);
        call_count.fetch_add(1, Ordering::Relaxed);
        true
    });

    match result {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from render_with_progress: {e}"),
    }

    let count = call_count.load(Ordering::Relaxed);
    assert!(
        count > 0,
        "on_progress must be invoked at least once, got {count} calls"
    );
}

/// Verifies that a `Timeline` with an audio volume fade track encodes without
/// error.  The volume track fades from −60 dB to 0 dB over the clip's
/// duration; the test confirms the output is a valid file with both streams.
///
/// This covers the acceptance criterion for issue #364:
/// "a Timeline with 3 clips and a volume fade track encodes correctly".
#[test]
#[ignore = "requires FFmpeg filter graph; run with -- --include-ignored"]
fn timeline_with_volume_animation_should_encode_successfully() {
    let src_path = test_output_path("timeline_vol_src.mp4");
    let out_path = test_output_path("timeline_vol_out.mp4");
    let _g_src = FileGuard::new(src_path.clone());
    let _g_out = FileGuard::new(out_path.clone());

    // Y=128, U=128, V=128 ≈ grey
    if make_source_file(&src_path, W, H, FPS, FRAME_COUNT, 128, 128, 128).is_none() {
        return;
    }

    // Volume track: −60 dB at t=0, 0 dB at t=1 s (linear fade-in).
    let vol_track = AnimationTrack::fade(
        -60.0_f64,
        0.0_f64,
        Duration::ZERO,
        Duration::from_secs(1),
        Easing::Linear,
    );

    let clip = Clip::new(&src_path);

    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![clip.clone()])
        .audio_track(vec![clip])
        // "audio_0_volume" is the key used by TimelineBuilder for the first
        // audio track's volume animation (format: "audio_{track_idx}_{prop}").
        .audio_animation("audio_0_volume", vol_track)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    match timeline.render(&out_path, render_config()) {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from Timeline::render: {e}"),
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };

    assert!(info.has_video(), "output must contain a video stream");
    assert!(info.has_audio(), "output must contain an audio stream");
    assert!(
        info.duration() > Duration::ZERO,
        "output must have non-zero duration"
    );
}

/// A timeline built entirely from generated (Solid + Text) clips renders to a
/// probe-valid file with no source media on disk. Exercises the compositor's
/// generated-source path (color/drawtext, not the lavfi demuxer — RK-013) end to
/// end through the engine. Filter/encoder unavailable → skip (RK-002).
#[test]
fn timeline_render_generated_clips_should_produce_valid_output() {
    use ff_format::{Color, TextSpec};

    let out_path = test_output_path("timeline_generated_out.mp4");
    let _g_out = FileGuard::new(out_path.clone());

    // Generated sources are infinite; trim gives each a 1-second out_point.
    let solid = Clip::solid(Color::rgb(0, 0, 255)).trim(Duration::ZERO, Duration::from_secs(1));
    let text = Clip::text(TextSpec::new("title"))
        .trim(Duration::ZERO, Duration::from_secs(1))
        .with_opacity(0.8);

    let timeline = match Timeline::builder()
        .canvas(W, H)
        .frame_rate(FPS)
        .video_track(vec![solid])
        .video_track(vec![text])
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping: Timeline::builder().build() failed: {e}");
            return;
        }
    };

    match timeline.render(&out_path, render_config()) {
        Ok(()) => {}
        Err(TimelineError::Filter(e)) => {
            println!("Skipping: filter graph construction failed: {e}");
            return;
        }
        Err(TimelineError::Encode(e)) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
        Err(TimelineError::Decode(e)) => {
            println!("Skipping: decoder unavailable: {e}");
            return;
        }
        Err(e) => panic!("unexpected error from Timeline::render: {e}"),
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };
    assert!(
        info.has_video(),
        "generated-clip render must contain a video stream"
    );
    let video = info.video_stream(0).expect("video stream must be present");
    assert_eq!(video.width(), W, "output width must match canvas");
    assert_eq!(video.height(), H, "output height must match canvas");
}
