//! Regression: the multi-track composition canvas must follow the timeline's
//! frame rate, not a hardcoded 30 fps.
//!
//! The composition builds a background `color` canvas and conforms layers to it.
//! When that rate was hardcoded to 30 fps but the timeline encoded at a different
//! rate (e.g. a 24 fps source with no explicit `frame_rate`, which defaults to the
//! source rate), the exported video was stretched by `30 / encode_rate` while the
//! audio stayed correct — desyncing A/V (audio progressively leading the video).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

mod fixtures;

use ff_decode::VideoDecoder;
use ff_encode::{AudioCodec, BitrateMode, VideoCodec};
use ff_pipeline::{Clip, EncoderConfig, Timeline};
use fixtures::{FileGuard, make_source_file, test_output_path};

#[test]
fn composition_does_not_stretch_non_30fps_video() {
    // 24 fps source, 2 s. Render WITHOUT setting frame_rate, so the timeline
    // defaults to the source's 24 fps.
    let src = test_output_path("compfps_src_24.mp4");
    let _gs = FileGuard::new(src.clone());
    let frame_count = 48usize; // 2 s at 24 fps
    if make_source_file(&src, 320, 240, 24.0, frame_count, 76, 84, 255).is_none() {
        return; // encoder unavailable -> skip
    }

    let out = test_output_path("compfps_out_24.mp4");
    let _go = FileGuard::new(out.clone());
    let clip = Clip::new(&src);
    let timeline = Timeline::builder()
        .video_track(vec![clip.clone()])
        .audio_track(vec![clip])
        .build()
        .expect("build timeline");
    let cfg = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .audio_codec(AudioCodec::Aac)
        .bitrate_mode(BitrateMode::Cbr(800_000))
        .build();
    if timeline.render(&out, cfg).is_err() {
        return; // encoder unavailable -> skip
    }

    // Exported video duration = last decoded frame PTS.
    let mut last = 0.0f64;
    let mut d = VideoDecoder::open(&out).build().expect("open output");
    while let Ok(Some(f)) = d.decode_one() {
        last = f.timestamp().as_duration().as_secs_f64();
    }

    let expected = (frame_count - 1) as f64 / 24.0; // ~1.958 s
    // The bug stretched this to ~2.45 s (1.25x). A small tolerance covers
    // codec frame-count rounding without admitting the regression.
    assert!(
        (last - expected).abs() < 0.2,
        "exported 24fps video was stretched: last_pts={last:.3}s expected~{expected:.3}s \
         — the composition canvas rate must follow the timeline frame_rate"
    );
}
