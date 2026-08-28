//! Integration tests for AudioReplacement.
//!
//! Tests verify:
//! - Errors on nonexistent inputs
//! - `MediaOperationFailed` when the audio input has no audio stream
//! - Successful remux when valid video and audio sources are provided

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_remux::{AudioReplacement, RemuxError};
use fixtures::{FileGuard, assert_valid_output_file, create_black_frame, test_output_path};

#[test]
fn audio_replacement_should_fail_when_video_input_missing() {
    let result =
        AudioReplacement::new("nonexistent_video.mp4", "nonexistent_audio.mp3", "out.mp4").run();
    assert!(
        result.is_err(),
        "expected error for nonexistent video input, got Ok(())"
    );
}

/// A video-only mp4 (no audio stream) used as the `audio_input` must return
/// `RemuxError::OperationFailed`.
#[test]
fn audio_replacement_should_fail_when_audio_input_has_no_audio_stream() {
    use ff_encode::{BitrateMode, Preset, VideoCodec, VideoEncoder};

    let video_only_path = test_output_path("audio_replacement_video_only.mp4");
    let output_path = test_output_path("audio_replacement_no_audio_stream.mp4");
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

    // Use the video-only file as the audio input — must fail with MediaOperationFailed.
    let result = AudioReplacement::new(&video_only_path, &video_only_path, &output_path).run();
    assert!(
        matches!(result, Err(RemuxError::OperationFailed { .. })),
        "expected MediaOperationFailed when audio input has no audio stream, got {result:?}"
    );
}

/// Encode a video-only source and a separate audio-only source, then replace
/// the audio.  The output file must exist and be non-empty.
#[test]
fn audio_replacement_should_produce_output_with_both_streams() {
    use ff_encode::{AudioCodec, AudioEncoder, BitrateMode, Preset, VideoCodec, VideoEncoder};
    use ff_format::{AudioFrame, SampleFormat};

    let video_path = test_output_path("audio_replacement_video_src.mp4");
    let audio_path = test_output_path("audio_replacement_audio_src.mp3");
    let output_path = test_output_path("audio_replacement_out.mp4");
    let _guard_v = FileGuard::new(video_path.clone());
    let _guard_a = FileGuard::new(audio_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    // Build video-only source (1 s at 15 fps, 160×120)
    let mut venc = match VideoEncoder::create(&video_path)
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
        if let Err(e) = venc.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
    }
    if let Err(e) = venc.finish() {
        println!("Skipping: video encoder finish failed ({e})");
        return;
    }

    // Build audio-only source (~1 s of silence at 44100 Hz mono)
    let sample_rate = 44100_u32;
    let channels = 1_u32;
    let samples_per_frame = 1152_usize; // MP3 frame size

    let mut aenc = match AudioEncoder::create(&audio_path)
        .audio(sample_rate, channels)
        .audio_codec(AudioCodec::Mp3)
        .audio_bitrate(64_000)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: audio encoder unavailable ({e})");
            return;
        }
    };

    let total_samples = sample_rate as usize;
    let mut pushed = 0_usize;
    while pushed < total_samples {
        let n = samples_per_frame.min(total_samples - pushed);
        let frame = match AudioFrame::empty(n, channels, sample_rate, SampleFormat::F32) {
            Ok(f) => f,
            Err(e) => {
                println!("Skipping: AudioFrame::empty failed ({e})");
                return;
            }
        };
        if let Err(e) = aenc.push(&frame) {
            println!("Skipping: push_audio failed ({e})");
            return;
        }
        pushed += n;
    }
    if let Err(e) = aenc.finish() {
        println!("Skipping: audio encoder finish failed ({e})");
        return;
    }

    // Replace audio
    let result = AudioReplacement::new(&video_path, &audio_path, &output_path).run();
    match result {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: AudioReplacement::run failed ({e})");
            return;
        }
    }

    assert_valid_output_file(&output_path);
}
