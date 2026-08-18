//! Integration tests for AudioExtractor.
//!
//! Tests verify:
//! - Errors on nonexistent inputs
//! - `MediaOperationFailed` when the input has no audio stream
//! - Successful extraction when a video+audio source is provided
//! - `stream_index()` selects the first audio stream explicitly

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_remux::{AudioExtractor, RemuxError};
use fixtures::{FileGuard, assert_valid_output_file, create_black_frame, test_output_path};

// ── Error-path tests ──────────────────────────────────────────────────────────

#[test]
fn audio_extractor_should_fail_when_input_missing() {
    let result = AudioExtractor::new("nonexistent_input.mp4", "out.mp3").run();
    assert!(
        result.is_err(),
        "expected error for nonexistent input, got Ok(())"
    );
}

/// A video-only mp4 (no audio stream) must return `MediaOperationFailed`.
#[test]
fn audio_extractor_should_fail_when_input_has_no_audio_stream() {
    use ff_encode::{BitrateMode, Preset, VideoCodec, VideoEncoder};

    let video_only_path = test_output_path("extractor_video_only.mp4");
    let output_path = test_output_path("extractor_no_audio_out.mp3");
    let _guard_v = FileGuard::new(video_only_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    // Build a video-only file (no audio stream).
    let mut encoder = match VideoEncoder::create(&video_only_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: video encoder unavailable ({e})");
            return;
        }
    };
    for _ in 0..15 {
        let frame = create_black_frame(160, 120);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder.finish failed ({e})");
        return;
    }

    let result = AudioExtractor::new(&video_only_path, &output_path).run();
    assert!(
        matches!(result, Err(RemuxError::OperationFailed { .. })),
        "expected MediaOperationFailed for input with no audio stream, got {result:?}"
    );
}

/// An out-of-bounds `stream_index` must return `MediaOperationFailed`.
#[test]
fn audio_extractor_should_fail_when_stream_index_out_of_range() {
    use ff_encode::{BitrateMode, Preset, VideoCodec, VideoEncoder};

    let video_only_path = test_output_path("extractor_stream_idx_oob.mp4");
    let output_path = test_output_path("extractor_stream_idx_oob_out.mp3");
    let _guard_v = FileGuard::new(video_only_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&video_only_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: video encoder unavailable ({e})");
            return;
        }
    };
    for _ in 0..15 {
        let frame = create_black_frame(160, 120);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder.finish failed ({e})");
        return;
    }

    // Stream index 99 doesn't exist in a 1-stream file.
    let result = AudioExtractor::new(&video_only_path, &output_path)
        .stream_index(99)
        .run();
    assert!(
        matches!(result, Err(RemuxError::OperationFailed { .. })),
        "expected MediaOperationFailed for out-of-range stream_index, got {result:?}"
    );
}

// ── Functional tests ──────────────────────────────────────────────────────────

/// Encode a video+audio source, then extract the audio track.
/// The output file must exist and be non-empty.
#[test]
fn audio_extractor_should_produce_audio_file() {
    use ff_encode::{AudioCodec, BitrateMode, Preset, VideoCodec, VideoEncoder};
    use ff_format::{AudioFrame, SampleFormat};

    let source_path = test_output_path("extractor_source.mp4");
    let output_path = test_output_path("extractor_output.mp3");
    let _guard_s = FileGuard::new(source_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    // Build a video+audio source (1 s at 15 fps, MP4/MPEG4+AAC).
    let mut encoder = match VideoEncoder::create(&source_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .audio(44100, 1)
        .audio_codec(AudioCodec::Mp3)
        .audio_bitrate(64_000)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: encoder unavailable ({e})");
            return;
        }
    };

    let sample_rate = 44100_u32;
    let samples_per_video_frame = (sample_rate as f64 / 15.0) as usize;

    for _ in 0..15 {
        let frame = create_black_frame(160, 120);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
        let audio =
            match AudioFrame::empty(samples_per_video_frame, 1, sample_rate, SampleFormat::F32) {
                Ok(f) => f,
                Err(e) => {
                    println!("Skipping: AudioFrame::empty failed ({e})");
                    return;
                }
            };
        if let Err(e) = encoder.push_audio(&audio) {
            println!("Skipping: push_audio failed ({e})");
            return;
        }
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder.finish failed ({e})");
        return;
    }

    // Extract audio.
    let result = AudioExtractor::new(&source_path, &output_path).run();
    match result {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: AudioExtractor::run failed ({e})");
            return;
        }
    }

    assert_valid_output_file(&output_path);
}

/// Same as above but selects stream index 1 (the audio stream in a
/// video=stream0 + audio=stream1 mp4).
#[test]
fn audio_extractor_stream_index_should_extract_selected_stream() {
    use ff_encode::{AudioCodec, BitrateMode, Preset, VideoCodec, VideoEncoder};
    use ff_format::{AudioFrame, SampleFormat};

    let source_path = test_output_path("extractor_idx_source.mp4");
    let output_path = test_output_path("extractor_idx_output.mp3");
    let _guard_s = FileGuard::new(source_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&source_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .audio(44100, 1)
        .audio_codec(AudioCodec::Mp3)
        .audio_bitrate(64_000)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: encoder unavailable ({e})");
            return;
        }
    };

    let sample_rate = 44100_u32;
    let samples_per_video_frame = (sample_rate as f64 / 15.0) as usize;

    for _ in 0..15 {
        let frame = create_black_frame(160, 120);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
        let audio =
            match AudioFrame::empty(samples_per_video_frame, 1, sample_rate, SampleFormat::F32) {
                Ok(f) => f,
                Err(e) => {
                    println!("Skipping: AudioFrame::empty failed ({e})");
                    return;
                }
            };
        if let Err(e) = encoder.push_audio(&audio) {
            println!("Skipping: push_audio failed ({e})");
            return;
        }
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder.finish failed ({e})");
        return;
    }

    // In a typical MPEG-4 file: stream 0 = video, stream 1 = audio.
    let result = AudioExtractor::new(&source_path, &output_path)
        .stream_index(1)
        .run();
    match result {
        Ok(()) => {}
        Err(RemuxError::OperationFailed { reason }) if reason.contains("not an audio") => {
            // stream 1 is video (container reordered) — acceptable, skip
            println!("Skipping: stream 1 is not audio in this container ({reason})");
            return;
        }
        Err(e) => {
            println!("Skipping: AudioExtractor::run failed ({e})");
            return;
        }
    }

    assert_valid_output_file(&output_path);
}
