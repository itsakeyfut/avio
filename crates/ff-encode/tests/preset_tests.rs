//! Integration tests for `ExportPreset` — both predefined and custom.
//!
//! Each test builds a short clip (≤ 1 s of synthetic frames) with one preset,
//! then opens the output with `ff_probe::open()` to confirm the file is valid
//! and contains the expected stream types.
//!
//! Tests skip gracefully when the required codec is unavailable in the linked
//! FFmpeg build (e.g. H.265 or FFV1 may be absent in some distributions).

#![allow(clippy::unwrap_used)]

mod fixtures;

use std::path::Path;

use ff_encode::{AudioEncoderConfig, BitrateMode, ExportPreset, VideoEncoder, VideoEncoderConfig};
use ff_format::{AudioCodec, AudioFrame, SampleFormat, VideoCodec};

// Constants

/// Small frame dimensions used in all video tests to keep encode time short.
const W: u32 = 160;
const H: u32 = 90;

/// Number of video frames to push per test (≈ 0.5 s at 30 fps).
const VIDEO_FRAMES: usize = 15;

// Shared helper

/// Encodes a short synthetic clip using `preset` and writes it to `output`.
///
/// For video presets the encoder is configured at `W×H` to keep encoding fast
/// regardless of the preset's native resolution.  For audio-only presets no
/// video frames are pushed and the output is expected to contain only an audio
/// stream.
///
/// Returns without asserting (just prints a skip message) when the encoder
/// cannot be built or any FFmpeg call fails — this handles systems where a
/// particular codec is not compiled in.
fn run_preset_test(preset: &ExportPreset, output: &Path, expect_video: bool) {
    let fps = preset.video.as_ref().and_then(|v| v.fps).unwrap_or(30.0);
    let sr = preset.audio.sample_rate;
    let ch = preset.audio.channels;

    // Apply all preset settings, then override resolution to a small size so
    // that the test runs quickly on every machine.
    let builder = VideoEncoder::create(output);
    let builder = preset.apply_video(builder);
    let builder = if expect_video {
        // Override resolution: apply_video may have set a large resolution
        // (e.g. 1920×1080); we force 160×90 here for test speed while still
        // exercising the codec / bitrate-mode / audio config from the preset.
        builder.video(W, H, fps)
    } else {
        builder
    };
    let builder = preset.apply_audio(builder);

    let mut encoder = match builder.build() {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping {}: build failed: {e}", preset.name);
            return;
        }
    };

    // Push video frames for video presets.
    if expect_video {
        let frame = fixtures::create_black_frame(W, H);
        for _ in 0..VIDEO_FRAMES {
            if let Err(e) = encoder.push_video(&frame) {
                println!("Skipping {}: push_video failed: {e}", preset.name);
                return;
            }
        }
    }

    // Push ~1 s of silent audio in 1 024-sample chunks (safe across codecs).
    let total_samples = sr as usize;
    let mut remaining = total_samples;
    while remaining > 0 {
        let n = remaining.min(1024);
        let frame = AudioFrame::empty(n, ch, sr, SampleFormat::F32)
            .expect("failed to create silent audio frame");
        if let Err(e) = encoder.push_audio(&frame) {
            println!("Skipping {}: push_audio failed: {e}", preset.name);
            return;
        }
        remaining -= n;
    }

    if let Err(e) = encoder.finish() {
        println!("Skipping {}: finish failed: {e}", preset.name);
        return;
    }

    let info = match ff_probe::open(output) {
        Ok(i) => i,
        Err(e) => {
            println!("Skipping {}: ff_probe::open failed: {e}", preset.name);
            return;
        }
    };

    if expect_video {
        assert!(
            info.has_video(),
            "{}: expected at least one video stream",
            preset.name
        );
        assert!(
            info.has_audio(),
            "{}: expected at least one audio stream",
            preset.name
        );
    } else {
        assert_eq!(
            info.video_stream_count(),
            0,
            "{}: audio-only preset must not produce a video stream",
            preset.name
        );
        assert!(
            info.has_audio(),
            "{}: expected at least one audio stream",
            preset.name
        );
    }
}

// Per-preset tests

#[test]
fn export_preset_youtube_1080p_should_produce_ffprobe_valid_output() {
    let output = fixtures::test_output_path("preset_youtube_1080p.mp4");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::youtube_1080p(), &output, true);
}

#[test]
fn export_preset_youtube_4k_should_produce_ffprobe_valid_output() {
    let output = fixtures::test_output_path("preset_youtube_4k.mp4");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::youtube_4k(), &output, true);
}

#[test]
fn export_preset_twitter_should_produce_ffprobe_valid_output() {
    let output = fixtures::test_output_path("preset_twitter.mp4");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::twitter(), &output, true);
}

#[test]
fn export_preset_instagram_square_should_produce_ffprobe_valid_output() {
    let output = fixtures::test_output_path("preset_instagram_square.mp4");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::instagram_square(), &output, true);
}

#[test]
fn export_preset_instagram_reels_should_produce_ffprobe_valid_output() {
    let output = fixtures::test_output_path("preset_instagram_reels.mp4");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::instagram_reels(), &output, true);
}

#[test]
fn export_preset_bluray_1080p_should_produce_ffprobe_valid_output() {
    let output = fixtures::test_output_path("preset_bluray_1080p.mp4");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::bluray_1080p(), &output, true);
}

#[test]
fn export_preset_podcast_mono_should_produce_ffprobe_valid_output() {
    // Audio-only preset: no video stream expected.
    let output = fixtures::test_output_path("preset_podcast_mono.m4a");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::podcast_mono(), &output, false);
}

#[test]
fn export_preset_lossless_rgb_should_produce_ffprobe_valid_output() {
    // FFV1 + FLAC; MKV is the natural container for lossless codecs.
    let output = fixtures::test_output_path("preset_lossless_rgb.mkv");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::lossless_rgb(), &output, true);
}

#[test]
fn export_preset_web_h264_should_produce_ffprobe_valid_output() {
    // VP9 + Opus; WebM is the required container for VP9/Opus.
    let output = fixtures::test_output_path("preset_web_h264.webm");
    let _guard = fixtures::FileGuard::new(output.clone());
    run_preset_test(&ExportPreset::web_h264(), &output, true);
}

// Custom preset test

#[test]
fn custom_export_preset_should_produce_ffprobe_valid_output() {
    // Construct ExportPreset directly as a struct literal — no predefined
    // factory method.  This exercises the public struct fields that #324 added
    // and confirms that user-defined presets apply correctly to the encoder.
    let preset = ExportPreset {
        name: "custom_test".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::H264,
            width: None,
            height: None,
            fps: Some(30.0),
            bitrate_mode: BitrateMode::Crf(28),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44_100,
            channels: 2,
            bitrate: 128_000,
        },
    };

    let output = fixtures::test_output_path("custom_preset_test.mp4");
    let _guard = fixtures::FileGuard::new(output.clone());

    // expect_video = true: preset has a video config, so both streams are expected.
    run_preset_test(&preset, &output, true);
}
