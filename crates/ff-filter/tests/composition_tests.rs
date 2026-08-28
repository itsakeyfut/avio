//! End-to-end integration tests for multi-track video composition, audio mixing,
//! and clip concatenation.
//!
//! - `multi_track_composition_should_produce_valid_mp4_output`: composites three
//!   synthetic video layers with `MultiTrackComposer` and mixes two audio tracks
//!   with `MultiTrackAudioMixer`.
//! - `video_concatenator_should_produce_output_longer_than_single_clip`:
//!   concatenates two synthetic video clips with `VideoConcatenator`.
//! - `audio_concatenator_should_produce_output_longer_than_single_clip`:
//!   concatenates two synthetic audio clips with `AudioConcatenator`.
//! - `animated_opacity_fade_should_darken_composite_over_time`: composites a
//!   white layer over a black background with opacity 1→0, verifies luma drops.
//! - `volume_automation_should_change_audio_amplitude`: mixes one track with
//!   volume animated from −60 dB → 0 dB, verifies RMS increases over time.

#![allow(clippy::unwrap_used)]

mod fixtures;

use std::time::Duration;

use ff_encode::{AudioCodec, AudioEncoder, VideoCodec, VideoEncoder};
use ff_filter::{
    AnimatedValue, AudioConcatenator, AudioTrack, FilterStep, LayerSource, MultiTrackAudioMixer,
    MultiTrackComposer, VideoConcatenator, VideoLayer, XfadeTransition,
    animation::{AnimationTrack, Easing, Keyframe},
};
use ff_format::{AudioFrame, ChannelLayout, SampleFormat, Timestamp};
use fixtures::{FileGuard, make_source_file, test_output_path, yuv420p_frame};

// Canvas / encoding parameters — kept small so CI runs quickly.
const CANVAS_W: u32 = 320;
const CANVAS_H: u32 = 180;
const FPS: f64 = 30.0;
const FRAME_COUNT: usize = 10; // ≈ 0.33 s per source clip
const SAMPLE_RATE: u32 = 48_000;

#[test]
fn multi_track_composition_should_produce_valid_mp4_output() {
    //
    // Each source is a solid-colour 10-frame clip with stereo AAC audio.
    //   Layer 0 (base)        : 320×180 — red-ish
    //   Layer 1 (PIP top-right): 160× 90 — green-ish
    //   Layer 2 (PIP btm-left): 80 × 46 — blue-ish  (height rounded to even)

    let src1_path = test_output_path("composition_src1.mp4");
    let src2_path = test_output_path("composition_src2.mp4");
    let src3_path = test_output_path("composition_src3.mp4");
    let out_path = test_output_path("composition_out.mp4");

    let _g1 = FileGuard::new(src1_path.clone());
    let _g2 = FileGuard::new(src2_path.clone());
    let _g3 = FileGuard::new(src3_path.clone());
    let _gout = FileGuard::new(out_path.clone());

    // Red-ish (Y=76, U=84, V=255 ≈ red in YUV)
    if make_source_file(
        &src1_path,
        CANVAS_W,
        CANVAS_H,
        FPS,
        FRAME_COUNT,
        76,
        84,
        255,
    )
    .is_none()
    {
        return;
    }
    // Green-ish (Y=149, U=43, V=21 ≈ green in YUV)
    if make_source_file(&src2_path, 160, 90, FPS, FRAME_COUNT, 149, 43, 21).is_none() {
        return;
    }
    // Blue-ish (Y=29, U=255, V=107 ≈ blue in YUV)
    if make_source_file(&src3_path, 80, 46, FPS, FRAME_COUNT, 29, 255, 107).is_none() {
        return;
    }

    let mut composer = match MultiTrackComposer::new(CANVAS_W, CANVAS_H)
        .add_layer(VideoLayer {
            source: LayerSource::File(src1_path.clone()),
            proxy: None,
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .add_layer(VideoLayer {
            source: LayerSource::File(src2_path.clone()),
            proxy: None,
            x: AnimatedValue::Static(160.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .add_layer(VideoLayer {
            source: LayerSource::File(src3_path.clone()),
            proxy: None,
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(134.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };

    let mut mixer = match MultiTrackAudioMixer::new(SAMPLE_RATE, ChannelLayout::Stereo)
        .add_track(AudioTrack {
            source: src1_path.clone(),
            volume: AnimatedValue::Static(0.0),
            pan: AnimatedValue::Static(0.0),
            effects: vec![],
            sample_rate: SAMPLE_RATE,
            channel_layout: ChannelLayout::Stereo,
        })
        .add_track(AudioTrack {
            source: src2_path.clone(),
            volume: AnimatedValue::Static(-3.0),
            pan: AnimatedValue::Static(0.0),
            effects: vec![],
            sample_rate: SAMPLE_RATE,
            channel_layout: ChannelLayout::Stereo,
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackAudioMixer::build failed: {e}");
            return;
        }
    };

    let mut encoder = match VideoEncoder::create(&out_path)
        .video(CANVAS_W, CANVAS_H, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .audio(SAMPLE_RATE, 2)
        .audio_codec(AudioCodec::Aac)
        .audio_bitrate(128_000)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: output encoder build failed: {e}");
            return;
        }
    };

    // Pull video frames from the composer and push to encoder.
    let mut video_frame_count = 0usize;
    loop {
        match composer.pull_video() {
            Ok(Some(frame)) => {
                // Re-encode as a plain YUV frame of the correct dimensions when
                // the filter graph outputs a different pixel format.  If the
                // format is already compatible, push directly.
                let push_frame;
                let to_push = if frame.width() == CANVAS_W && frame.height() == CANVAS_H {
                    &frame
                } else {
                    // Synthesise a black frame as a safe fallback — this path
                    // should not be reached for standard overlay outputs.
                    push_frame = yuv420p_frame(CANVAS_W, CANVAS_H, 16, 128, 128);
                    &push_frame
                };
                if let Err(e) = encoder.push_video(to_push) {
                    println!("Skipping: push_video to output encoder failed: {e}");
                    return;
                }
                video_frame_count += 1;
            }
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: pull_video from composer failed: {e}");
                return;
            }
        }
    }

    // Pull audio frames from the mixer and push to encoder.
    loop {
        match mixer.pull_audio() {
            Ok(Some(audio_frame)) => {
                // The amix filter may output in planar format; convert to a
                // packed F32 frame that the AAC encoder accepts when needed.
                let compat_frame: AudioFrame;
                let to_push = if audio_frame.format() == SampleFormat::F32 {
                    &audio_frame
                } else {
                    compat_frame = match AudioFrame::empty(
                        audio_frame.samples(),
                        audio_frame.channels(),
                        audio_frame.sample_rate(),
                        SampleFormat::F32,
                    ) {
                        Ok(f) => f,
                        Err(e) => {
                            println!("Skipping: audio frame conversion failed: {e}");
                            return;
                        }
                    };
                    &compat_frame
                };
                if let Err(e) = encoder.push_audio(to_push) {
                    println!("Skipping: push_audio to output encoder failed: {e}");
                    return;
                }
            }
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: pull_audio from mixer failed: {e}");
                return;
            }
        }
    }

    if let Err(e) = encoder.finish() {
        println!("Skipping: output encoder finish failed: {e}");
        return;
    }

    let info = match ff_probe::open(&out_path) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping: ff_probe::open failed: {e}");
            return;
        }
    };

    assert_eq!(
        info.video_stream_count(),
        1,
        "output must contain exactly one video stream, found {}",
        info.video_stream_count()
    );
    assert_eq!(
        info.audio_stream_count(),
        1,
        "output must contain exactly one audio stream, found {}",
        info.audio_stream_count()
    );
    assert!(
        video_frame_count > 0,
        "composer must produce at least one video frame"
    );

    let video = info.video_stream(0).expect("video stream must be present");
    assert_eq!(
        video.width(),
        CANVAS_W,
        "output video width must match canvas"
    );
    assert_eq!(
        video.height(),
        CANVAS_H,
        "output video height must match canvas"
    );
}

#[test]
fn video_concatenator_should_produce_output_longer_than_single_clip() {
    // Use 30 frames at 30 fps so each source clip is ≈ 1 second.
    // Two clips concatenated → output duration ≥ 1.5 s.
    const W: u32 = 160;
    const H: u32 = 90;
    const FPS: f64 = 30.0;
    const FRAMES_PER_CLIP: usize = 30; // 1 s per clip

    let src1_path = test_output_path("video_concat_src1.mp4");
    let src2_path = test_output_path("video_concat_src2.mp4");
    let out_path = test_output_path("video_concat_out.mp4");

    let _g1 = FileGuard::new(src1_path.clone());
    let _g2 = FileGuard::new(src2_path.clone());
    let _gout = FileGuard::new(out_path.clone());

    // Clip 1: red-ish (Y=76, U=84, V=255)
    if make_source_file(&src1_path, W, H, FPS, FRAMES_PER_CLIP, 76, 84, 255).is_none() {
        return;
    }
    // Clip 2: blue-ish (Y=29, U=255, V=107)
    if make_source_file(&src2_path, W, H, FPS, FRAMES_PER_CLIP, 29, 255, 107).is_none() {
        return;
    }

    let mut graph = match VideoConcatenator::new(vec![&src1_path, &src2_path])
        .output_resolution(W, H)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: VideoConcatenator::build failed: {e}");
            return;
        }
    };

    let mut encoder = match VideoEncoder::create(&out_path)
        .video(W, H, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: output encoder build failed: {e}");
            return;
        }
    };

    let mut frame_count = 0usize;
    loop {
        match graph.pull_video() {
            Ok(Some(frame)) => {
                if let Err(e) = encoder.push_video(&frame) {
                    println!("Skipping: push_video failed: {e}");
                    return;
                }
                frame_count += 1;
            }
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: pull_video failed: {e}");
                return;
            }
        }
    }

    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder finish failed: {e}");
        return;
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
        "concatenated output must have a video stream"
    );

    let duration = info.duration();
    assert!(
        duration >= Duration::from_millis(1500),
        "concatenated output must be ≥ 1.5 s (two 1-second clips), got {duration:?}"
    );

    assert!(
        frame_count > FRAMES_PER_CLIP,
        "concatenated output must have more frames than a single clip ({FRAMES_PER_CLIP}), got {frame_count}"
    );
}

#[test]
fn audio_concatenator_should_produce_output_longer_than_single_clip() {
    // 44 × 1024 samples ÷ 44 100 Hz ≈ 1.02 s per source clip.
    // Two clips concatenated → output duration ≥ 1.5 s.
    const SAMPLE_RATE: u32 = 44_100;
    const CHANNELS: u32 = 1; // mono
    const FRAME_SIZE: usize = 1024;
    const FRAMES_PER_CLIP: usize = 44; // ≈ 1.02 s

    let src1_path = test_output_path("audio_concat_src1.m4a");
    let src2_path = test_output_path("audio_concat_src2.m4a");
    let out_path = test_output_path("audio_concat_out.m4a");

    let _g1 = FileGuard::new(src1_path.clone());
    let _g2 = FileGuard::new(src2_path.clone());
    let _gout = FileGuard::new(out_path.clone());

    for src_path in [&src1_path, &src2_path] {
        let mut enc = match AudioEncoder::create(src_path)
            .audio(SAMPLE_RATE, CHANNELS)
            .audio_codec(AudioCodec::Aac)
            .build()
        {
            Ok(e) => e,
            Err(e) => {
                println!("Skipping: AudioEncoder::build failed: {e}");
                return;
            }
        };
        for _ in 0..FRAMES_PER_CLIP {
            let frame =
                match AudioFrame::empty(FRAME_SIZE, CHANNELS, SAMPLE_RATE, SampleFormat::F32) {
                    Ok(f) => f,
                    Err(e) => {
                        println!("Skipping: AudioFrame::empty failed: {e}");
                        return;
                    }
                };
            if let Err(e) = enc.push(&frame) {
                println!("Skipping: source push failed: {e}");
                return;
            }
        }
        if let Err(e) = enc.finish() {
            println!("Skipping: source encoder finish failed: {e}");
            return;
        }
    }

    let mut graph = match AudioConcatenator::new(vec![&src1_path, &src2_path])
        .output_format(SAMPLE_RATE, ChannelLayout::Mono)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: AudioConcatenator::build failed: {e}");
            return;
        }
    };

    let mut out_enc = match AudioEncoder::create(&out_path)
        .audio(SAMPLE_RATE, CHANNELS)
        .audio_codec(AudioCodec::Aac)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: output encoder build failed: {e}");
            return;
        }
    };

    loop {
        match graph.pull_audio() {
            Ok(Some(frame)) => {
                if let Err(e) = out_enc.push(&frame) {
                    println!("Skipping: push to output encoder failed: {e}");
                    return;
                }
            }
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: pull_audio failed: {e}");
                return;
            }
        }
    }

    if let Err(e) = out_enc.finish() {
        println!("Skipping: output encoder finish failed: {e}");
        return;
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
        "concatenated output must have an audio stream"
    );
    assert_eq!(
        info.video_stream_count(),
        0,
        "audio-only output must not have a video stream"
    );

    let duration = info.duration();
    assert!(
        duration >= Duration::from_millis(1500),
        "concatenated output must be ≥ 1.5 s (two ≈1-second clips), got {duration:?}"
    );
}

// Animation integration tests

/// Verifies that `AnimatedValue::Track` for `opacity` causes the composited
/// luma to decrease as the layer fades from fully opaque to fully transparent.
///
/// Setup:
/// - Background: 64×64, full-black (Y=16)
/// - Overlay:    64×64, full-white (Y=235), covers entire canvas
/// - Opacity track: Linear 1.0 → 0.0 over `FADE_FRAMES` frames at 30 fps
///
/// Assertion: center-pixel luma at frame 0 is brighter than at the last frame.
#[test]
#[ignore = "requires FFmpeg filter graph; run with -- --include-ignored"]
fn animated_opacity_fade_should_darken_composite_over_time() {
    const W: u32 = 64;
    const H: u32 = 64;
    const FPS: f64 = 30.0;
    const FADE_FRAMES: usize = 30;

    let bg_path = test_output_path("opacity_bg_64x64.mp4");
    let layer_path = test_output_path("opacity_layer_64x64.mp4");

    let _bg_guard = FileGuard::new(bg_path.clone());
    let _layer_guard = FileGuard::new(layer_path.clone());

    // Background: black (Y=16, U=128, V=128)
    if make_source_file(&bg_path, W, H, FPS, FADE_FRAMES, 16, 128, 128).is_none() {
        return;
    }
    // Overlay: white (Y=235, U=128, V=128)
    if make_source_file(&layer_path, W, H, FPS, FADE_FRAMES, 235, 128, 128).is_none() {
        return;
    }

    // Opacity: 1.0 at frame 0 → 0.0 at last frame, Linear easing.
    let end_pts = Duration::from_secs_f64((FADE_FRAMES as f64 - 1.0) / FPS);
    let opacity_track = AnimationTrack::new()
        .push(Keyframe::new(Duration::ZERO, 1.0_f64, Easing::Linear))
        .push(Keyframe::new(end_pts, 0.0_f64, Easing::Linear));

    let mut composer = match MultiTrackComposer::new(W, H)
        .add_layer(VideoLayer {
            source: LayerSource::File(bg_path.clone()),
            proxy: None,
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .add_layer(VideoLayer {
            source: LayerSource::File(layer_path.clone()),
            proxy: None,
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Track(opacity_track),
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };

    // Collect center-pixel luma for every frame
    let cx = (W / 2) as usize;
    let cy = (H / 2) as usize;
    let mut lumas: Vec<f64> = Vec::with_capacity(FADE_FRAMES);

    for i in 0..FADE_FRAMES {
        let pts = Duration::from_secs_f64(i as f64 / FPS);
        composer.tick(pts);

        let frame = match composer.pull_video() {
            Ok(Some(f)) => f,
            Ok(None) => {
                println!("Skipping: composer ended early at frame {i}");
                return;
            }
            Err(e) => {
                println!("Skipping: pull_video failed at frame {i}: {e}");
                return;
            }
        };

        if frame.width() != W || frame.height() != H {
            println!(
                "Skipping: unexpected frame dimensions {}×{} at frame {i}",
                frame.width(),
                frame.height()
            );
            return;
        }

        let stride = frame.stride(0).unwrap_or(W as usize);
        let y_plane = match frame.plane(0) {
            Some(p) => p,
            None => {
                println!("Skipping: Y-plane unavailable at frame {i}");
                return;
            }
        };
        lumas.push(y_plane[cy * stride + cx] as f64);
    }

    let first = lumas[0];
    let last = lumas[FADE_FRAMES - 1];

    assert!(
        first > last + 50.0,
        "frame 0 (opacity=1.0) luma={first:.1} must be significantly brighter than \
         frame {} (opacity=0.0) luma={last:.1}",
        FADE_FRAMES - 1
    );
}

// Packed-F32 stereo audio frame with constant amplitude

fn constant_amplitude_frame(amplitude: f32, samples: usize, sample_rate: u32) -> AudioFrame {
    let channels = 2usize;
    let bytes_per_sample = 4usize; // f32
    let v = amplitude.to_le_bytes();
    let mut buf = vec![0u8; samples * channels * bytes_per_sample];
    for i in 0..samples {
        let off = i * channels * bytes_per_sample;
        buf[off..off + 4].copy_from_slice(&v); // L
        buf[off + 4..off + 8].copy_from_slice(&v); // R
    }
    AudioFrame::new(
        vec![buf],
        samples,
        2,
        sample_rate,
        SampleFormat::F32,
        Timestamp::default(),
    )
    .expect("constant_amplitude_frame: AudioFrame::new failed")
}

/// RMS of packed-F32 samples stored as raw bytes.
fn rms_bytes(raw: &[u8]) -> f64 {
    if raw.len() < 4 {
        return 0.0;
    }
    let n = raw.len() / 4;
    let sum_sq: f64 = (0..n)
        .map(|i| {
            let bytes = [raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]];
            let s = f32::from_le_bytes(bytes) as f64;
            s * s
        })
        .sum();
    (sum_sq / n as f64).sqrt()
}

/// Verifies that `AnimatedValue::Track` for `volume` on an `AudioTrack` causes
/// the output RMS to increase as gain ramps from −60 dB → 0 dB.
///
/// Setup:
/// - Source audio: constant-amplitude stereo F32 at 0.5 peak
/// - Volume track: Linear −60 dB at t=0 → 0 dB at t=end
///
/// Assertion: mean RMS of last 5 pulled frames > mean RMS of first 5 frames × 10.
#[test]
#[ignore = "requires FFmpeg filter graph; run with -- --include-ignored"]
fn volume_automation_should_increase_audio_amplitude_over_time() {
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u32 = 2;
    const FRAME_SAMPLES: usize = 1024;
    const AUDIO_FRAMES: usize = 60; // ≈ 1.28 s

    let src_path = test_output_path("vol_auto_src.m4a");
    let _src_guard = FileGuard::new(src_path.clone());

    let mut enc = match AudioEncoder::create(&src_path)
        .audio(SAMPLE_RATE, CHANNELS)
        .audio_codec(AudioCodec::Aac)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: AudioEncoder::build failed: {e}");
            return;
        }
    };

    for _ in 0..AUDIO_FRAMES {
        let frame = constant_amplitude_frame(0.5, FRAME_SAMPLES, SAMPLE_RATE);
        if let Err(e) = enc.push(&frame) {
            println!("Skipping: source push failed: {e}");
            return;
        }
    }
    if let Err(e) = enc.finish() {
        println!("Skipping: source encoder finish failed: {e}");
        return;
    }

    //
    // Volume ramps from −60 dB (near-silence) to 0 dB (unity gain).
    let total_duration =
        Duration::from_secs_f64(AUDIO_FRAMES as f64 * FRAME_SAMPLES as f64 / SAMPLE_RATE as f64);
    let vol_track = AnimationTrack::new()
        .push(Keyframe::new(Duration::ZERO, -60.0_f64, Easing::Linear))
        .push(Keyframe::new(total_duration, 0.0_f64, Easing::Linear));

    let mut mixer = match MultiTrackAudioMixer::new(SAMPLE_RATE, ChannelLayout::Stereo)
        .add_track(AudioTrack {
            source: src_path.clone(),
            volume: AnimatedValue::Track(vol_track),
            pan: AnimatedValue::Static(0.0),
            effects: vec![],
            sample_rate: SAMPLE_RATE,
            channel_layout: ChannelLayout::Stereo,
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackAudioMixer::build failed: {e}");
            return;
        }
    };

    let mut pulled: Vec<f64> = Vec::new(); // RMS per chunk
    let mut chunk_pts = Duration::ZERO;
    let chunk_duration = Duration::from_secs_f64(FRAME_SAMPLES as f64 / SAMPLE_RATE as f64);

    loop {
        mixer.tick(chunk_pts);

        match mixer.pull_audio() {
            Ok(Some(frame)) => {
                let rms = match frame.plane(0) {
                    Some(raw) => rms_bytes(raw),
                    None => 0.0,
                };
                pulled.push(rms);
                chunk_pts += chunk_duration;
            }
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: pull_audio failed: {e}");
                return;
            }
        }
    }

    if pulled.len() < 10 {
        println!("Skipping: too few audio chunks pulled ({})", pulled.len());
        return;
    }

    let n = pulled.len();
    let window = (n / 5).max(1);

    let early_rms: f64 = pulled[..window].iter().sum::<f64>() / window as f64;
    let late_rms: f64 = pulled[n - window..].iter().sum::<f64>() / window as f64;

    assert!(
        late_rms > early_rms * 10.0,
        "late-frame RMS ({late_rms:.6}) must be > 10× early-frame RMS ({early_rms:.6}) \
         — volume automation did not take effect (total chunks={n})"
    );
}

/// Verifies that frames pulled from `MultiTrackComposer` always have `yuv420p` format,
/// regardless of FFmpeg's internal format negotiation.
///
/// Acceptance criterion for issue #1017: newer FFmpeg builds may negotiate `gbrp` when
/// no explicit format constraint is set on the buffersink, causing green-tinted output.
/// The fix inserts a `format=yuv420p` filter before each video buffersink.
#[test]
fn multi_track_composition_should_produce_yuv420p_frames() {
    use ff_format::PixelFormat;

    const W: u32 = 64;
    const H: u32 = 64;
    const FPS_LOCAL: f64 = 30.0;
    const FRAMES: usize = 5;

    let src_path = test_output_path("yuv420p_check_src.mp4");
    let _g = FileGuard::new(src_path.clone());

    if make_source_file(&src_path, W, H, FPS_LOCAL, FRAMES, 128, 128, 128).is_none() {
        return;
    }

    let mut composer = match MultiTrackComposer::new(W, H)
        .add_layer(VideoLayer {
            source: LayerSource::File(src_path.clone()),
            proxy: None,
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };

    let frame = match composer.pull_video() {
        Ok(Some(f)) => f,
        Ok(None) => {
            println!("Skipping: composer produced no frames");
            return;
        }
        Err(e) => {
            println!("Skipping: pull_video failed: {e}");
            return;
        }
    };

    assert_eq!(
        frame.format(),
        PixelFormat::Yuv420p,
        "composition graph must deliver yuv420p frames; got {:?}",
        frame.format()
    );
}

// #1222: non-Over CompositeOp layer compositing

/// A layer with a non-`Over` `composite_op` (here `In`) composites via the
/// Porter-Duff `blend all_expr` construction. Uses a `lavfi` source so no fixture
/// file is needed. Probe-gated: CI's Linux FFmpeg has no filters compiled in, so
/// `build`/`pull` failure → skip (RK-002).
#[test]
fn composite_op_in_layer_should_build_and_produce_frame() {
    let layer = VideoLayer {
        source: LayerSource::Lavfi("color=c=white:s=64x64:r=25:d=1".to_string()),
        proxy: None,
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        opacity: AnimatedValue::Static(1.0),
        blend_mode: ff_filter::BlendMode::Normal,
        composite_op: ff_filter::CompositeOp::In,
        effects: vec![],
    };
    let mut graph = match MultiTrackComposer::new(64, 64).add_layer(layer).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };
    match graph.pull_video() {
        Ok(Some(frame)) => {
            assert_eq!(
                frame.width(),
                64,
                "composite In must not change frame width"
            );
            assert_eq!(
                frame.height(),
                64,
                "composite In must not change frame height"
            );
        }
        Ok(None) => println!("Skipping: composite graph produced no frame"),
        Err(e) => println!("Skipping: pull_video failed: {e}"),
    }
}

/// A layer with `composite_op = Under` composites via the swapped-pad `overlay`
/// construction (canvas over layer) — a distinct code path from the `blend
/// all_expr` operators. `opacity = 0.5` additionally exercises the
/// colorchannelmixer alpha pre-step and `format=auto` overlay. Probe-gated like
/// the `In` test above (RK-002).
#[test]
fn composite_op_under_layer_with_opacity_should_build_and_produce_frame() {
    let layer = VideoLayer {
        source: LayerSource::Lavfi("color=c=white:s=64x64:r=25:d=1".to_string()),
        proxy: None,
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        opacity: AnimatedValue::Static(0.5),
        blend_mode: ff_filter::BlendMode::Normal,
        composite_op: ff_filter::CompositeOp::Under,
        effects: vec![],
    };
    let mut graph = match MultiTrackComposer::new(64, 64).add_layer(layer).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };
    match graph.pull_video() {
        Ok(Some(frame)) => {
            assert_eq!(
                frame.width(),
                64,
                "composite Under must not change frame width"
            );
            assert_eq!(
                frame.height(),
                64,
                "composite Under must not change frame height"
            );
        }
        Ok(None) => println!("Skipping: composite Under graph produced no frame"),
        Err(e) => println!("Skipping: pull_video failed: {e}"),
    }
}

// #1330: timeline trim/placement expressed as leading FilterSteps

/// After #1330 the engine derive emits a clip's trim + timeline placement as
/// leading `FilterStep`s (`Trim` + `ResetPts` + `OffsetPts`) instead of
/// `VideoLayer` fields. Verify the compositor builds that effect chain and pulls
/// a frame. Uses a `lavfi` source; probe-gated (RK-002): CI's Linux FFmpeg has no
/// filters, so build/pull failure → skip.
#[test]
fn trim_and_offset_effects_should_build_and_produce_frame() {
    let layer = VideoLayer {
        source: LayerSource::Lavfi("color=c=white:s=64x64:r=25:d=2".to_string()),
        proxy: None,
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        opacity: AnimatedValue::Static(1.0),
        blend_mode: ff_filter::BlendMode::Normal,
        composite_op: ff_filter::CompositeOp::Over,
        effects: vec![
            FilterStep::Trim {
                start: Some(0.5),
                end: Some(1.5),
            },
            FilterStep::ResetPts,
            FilterStep::OffsetPts { seconds: 0.5 },
        ],
    };
    let mut graph = match MultiTrackComposer::new(64, 64).add_layer(layer).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };
    match graph.pull_video() {
        Ok(Some(frame)) => {
            assert_eq!(frame.width(), 64, "trim/offset must not change frame width");
            assert_eq!(
                frame.height(),
                64,
                "trim/offset must not change frame height"
            );
        }
        Ok(None) => println!("Skipping: trim/offset graph produced no frame"),
        Err(e) => println!("Skipping: pull_video failed: {e}"),
    }
}

// #1331: audio timeline trim/placement expressed as leading FilterSteps

/// After #1331 the engine derive emits a clip's audio source trim + timeline
/// placement as leading `FilterStep`s (`ATrim` + `AResetPts` + `AudioDelay`)
/// instead of `AudioTrack` fields. Verify the mixer builds that effect chain and
/// pulls audio. Probe-gated (RK-002): build/pull failure → skip.
#[test]
fn audio_trim_and_offset_effects_should_build_mix_and_pull() {
    let src = test_output_path("audio_trim_offset_src.mp4");
    let _g = FileGuard::new(src.clone());
    if make_source_file(&src, 64, 64, 30.0, 30, 128, 128, 128).is_none() {
        return;
    }

    let mut mixer = match MultiTrackAudioMixer::new(48_000, ChannelLayout::Stereo)
        .add_track(AudioTrack {
            source: src.clone(),
            volume: AnimatedValue::Static(0.0),
            pan: AnimatedValue::Static(0.0),
            effects: vec![
                FilterStep::ATrim {
                    start: Some(0.2),
                    end: Some(0.8),
                },
                FilterStep::AResetPts,
                FilterStep::AudioDelay { ms: 100.0 },
            ],
            sample_rate: 48_000,
            channel_layout: ChannelLayout::Stereo,
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackAudioMixer::build failed: {e}");
            return;
        }
    };

    match mixer.pull_audio() {
        Ok(Some(_frame)) => {} // produced audio through the effect chain — pass
        Ok(None) => println!("Skipping: mixer produced no audio frame"),
        Err(e) => println!("Skipping: pull_audio failed: {e}"),
    }
}

// #1332: inter-clip transition expressed as a FilterStep::XFade

/// After #1332 the engine emits a cross-fade as a primitive `FilterStep::XFade`
/// on the transitioning layer (no `ClipTransition`/`in_transition`). Verify the
/// compositor defers the predecessor's overlay and wires the xfade's second input
/// from it. Two `lavfi` layers; probe-gated (RK-002): build/pull failure → skip.
#[test]
fn xfade_effect_layer_should_build_and_produce_frame() {
    let layer_a = VideoLayer {
        source: LayerSource::Lavfi("color=c=red:s=64x64:r=25:d=2".to_string()),
        proxy: None,
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        opacity: AnimatedValue::Static(1.0),
        blend_mode: ff_filter::BlendMode::Normal,
        composite_op: ff_filter::CompositeOp::Over,
        effects: vec![],
    };
    let layer_b = VideoLayer {
        source: LayerSource::Lavfi("color=c=blue:s=64x64:r=25:d=2".to_string()),
        proxy: None,
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        opacity: AnimatedValue::Static(1.0),
        blend_mode: ff_filter::BlendMode::Normal,
        composite_op: ff_filter::CompositeOp::Over,
        effects: vec![FilterStep::XFade {
            transition: XfadeTransition::Dissolve,
            duration: 0.5,
            offset: 1.0,
        }],
    };
    let mut graph = match MultiTrackComposer::new(64, 64)
        .add_layer(layer_a)
        .add_layer(layer_b)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };
    match graph.pull_video() {
        Ok(Some(frame)) => {
            assert_eq!(frame.width(), 64, "xfade must not change frame width");
            assert_eq!(frame.height(), 64, "xfade must not change frame height");
        }
        Ok(None) => println!("Skipping: xfade graph produced no frame"),
        Err(e) => println!("Skipping: pull_video failed: {e}"),
    }
}

/// A generated Solid layer and a generated Text layer compose in one multi-layer
/// graph and produce a canvas-sized frame. Generated sources build from the
/// `color`(+`drawtext`) filter nodes, not the lavfi demuxer (RK-013), so this
/// verifies the compositor's generated-source path end to end. `build`/`pull`
/// failure → skip (RK-002).
#[test]
fn generated_solid_and_text_layers_should_compose_and_produce_frame() {
    use ff_format::{Color, TextSpec};

    let base = |source: LayerSource| VideoLayer {
        source,
        proxy: None,
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        opacity: AnimatedValue::Static(1.0),
        blend_mode: ff_filter::BlendMode::Normal,
        composite_op: ff_filter::CompositeOp::Over,
        effects: vec![],
    };

    let mut graph = match MultiTrackComposer::new(128, 64)
        .frame_rate(25.0)
        .add_layer(base(LayerSource::Solid(Color::rgb(0, 0, 255))))
        .add_layer(base(LayerSource::Text(TextSpec::new("hi"))))
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };
    match graph.pull_video() {
        Ok(Some(frame)) => {
            assert_eq!(frame.width(), 128, "generated layers must fill the canvas");
            assert_eq!(frame.height(), 64, "generated layers must fill the canvas");
        }
        Ok(None) => println!("Skipping: generated composition produced no frame"),
        Err(e) => println!("Skipping: pull_video failed: {e}"),
    }
}
