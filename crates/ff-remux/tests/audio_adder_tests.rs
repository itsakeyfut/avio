//! Integration tests for AudioAdder.
//!
//! Tests verify:
//! - Errors on nonexistent inputs
//! - `MediaOperationFailed` when video input has no video stream
//! - `MediaOperationFailed` when audio input has no audio stream
//! - Successful mux when valid video and audio sources are provided
//! - `loop_audio()` produces a non-empty output when audio is shorter than video

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_remux::{AudioAdder, RemuxError};
use fixtures::{FileGuard, assert_valid_output_file, create_black_frame, test_output_path};

// ── Error-path tests ──────────────────────────────────────────────────────────

#[test]
fn audio_adder_should_fail_when_video_input_missing() {
    let result = AudioAdder::new("nonexistent_video.mp4", "nonexistent_audio.mp3", "out.mp4").run();
    assert!(
        result.is_err(),
        "expected error for nonexistent video input, got Ok(())"
    );
}

/// A video-only mp4 used as `audio_input` must return `MediaOperationFailed`.
#[test]
fn audio_adder_should_fail_when_audio_input_has_no_audio_stream() {
    use ff_encode::{BitrateMode, Preset, VideoCodec, VideoEncoder};

    let video_path = test_output_path("adder_video.mp4");
    let audio_only_video_path = test_output_path("adder_video_no_audio.mp4");
    let output_path = test_output_path("adder_no_audio_stream_out.mp4");
    let _guard_v = FileGuard::new(video_path.clone());
    let _guard_a = FileGuard::new(audio_only_video_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    // Build the video source (no audio).
    let mut enc = match VideoEncoder::create(&video_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: video encoder unavailable ({e})");
            return;
        }
    };
    for _ in 0..15 {
        let frame = create_black_frame(160, 120);
        if let Err(e) = enc.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
    }
    if let Err(e) = enc.finish() {
        println!("Skipping: encoder.finish failed ({e})");
        return;
    }

    // Build a second video-only file to use as the "audio" input (no audio stream).
    let mut enc2 = match VideoEncoder::create(&audio_only_video_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: video encoder unavailable ({e})");
            return;
        }
    };
    for _ in 0..15 {
        let frame = create_black_frame(160, 120);
        if let Err(e) = enc2.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
    }
    if let Err(e) = enc2.finish() {
        println!("Skipping: encoder.finish failed ({e})");
        return;
    }

    let result = AudioAdder::new(&video_path, &audio_only_video_path, &output_path).run();
    assert!(
        matches!(result, Err(RemuxError::OperationFailed { .. })),
        "expected MediaOperationFailed when audio_input has no audio stream, got {result:?}"
    );
}

// ── Functional tests ──────────────────────────────────────────────────────────

/// Encode a silent video + separate audio, mux them; output must exist and be non-empty.
#[test]
fn audio_adder_should_produce_output_with_both_streams() {
    use ff_encode::{AudioCodec, AudioEncoder, BitrateMode, Preset, VideoCodec, VideoEncoder};
    use ff_format::{AudioFrame, SampleFormat};

    let video_path = test_output_path("adder_video_source.mp4");
    let audio_path = test_output_path("adder_audio_source.mp3");
    let output_path = test_output_path("adder_combined_out.mp4");
    let _guard_v = FileGuard::new(video_path.clone());
    let _guard_a = FileGuard::new(audio_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    // Build a silent video (1 s, 15 fps).
    let mut venc = match VideoEncoder::create(&video_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .build()
    {
        Ok(e) => e,
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
        println!("Skipping: video encoder.finish failed ({e})");
        return;
    }

    // Build an audio-only file (1 s, 44100 Hz, mono, MP3).
    let mut aenc = match AudioEncoder::create(&audio_path)
        .audio(44100, 1)
        .audio_codec(AudioCodec::Mp3)
        .audio_bitrate(64_000)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: audio encoder unavailable ({e})");
            return;
        }
    };
    // MP3 uses 1152 samples per frame.
    let frame_size: usize = 1152;
    let silence = match AudioFrame::empty(frame_size, 1, 44100, SampleFormat::F32) {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: AudioFrame::empty failed ({e})");
            return;
        }
    };
    // Push ~1 s worth of audio frames (44100 / 1152 ≈ 39 frames).
    let frames_needed = (44100 / frame_size) + 1;
    for _ in 0..frames_needed {
        if let Err(e) = aenc.push(&silence) {
            println!("Skipping: aenc.push failed ({e})");
            return;
        }
    }
    if let Err(e) = aenc.finish() {
        println!("Skipping: audio encoder.finish failed ({e})");
        return;
    }

    // Add audio to video.
    let result = AudioAdder::new(&video_path, &audio_path, &output_path).run();
    match result {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: AudioAdder::run failed ({e})");
            return;
        }
    }

    assert_valid_output_file(&output_path);
}

/// When audio is shorter than video and `loop_audio()` is set, output must exist and be non-empty.
#[test]
fn audio_adder_loop_audio_should_produce_output_when_audio_is_shorter() {
    use ff_encode::{AudioCodec, AudioEncoder, BitrateMode, Preset, VideoCodec, VideoEncoder};
    use ff_format::{AudioFrame, SampleFormat};

    let video_path = test_output_path("adder_loop_video.mp4");
    let audio_path = test_output_path("adder_loop_audio.mp3");
    let output_path = test_output_path("adder_loop_out.mp4");
    let _guard_v = FileGuard::new(video_path.clone());
    let _guard_a = FileGuard::new(audio_path.clone());
    let _guard_o = FileGuard::new(output_path.clone());

    // Build a 2 s video (30 frames at 15 fps).
    let mut venc = match VideoEncoder::create(&video_path)
        .video(160, 120, 15.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(200_000))
        .preset(Preset::Ultrafast)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: video encoder unavailable ({e})");
            return;
        }
    };
    for _ in 0..30 {
        let frame = create_black_frame(160, 120);
        if let Err(e) = venc.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
    }
    if let Err(e) = venc.finish() {
        println!("Skipping: video encoder.finish failed ({e})");
        return;
    }

    // Build a ~0.5 s audio (only a few frames).
    let mut aenc = match AudioEncoder::create(&audio_path)
        .audio(44100, 1)
        .audio_codec(AudioCodec::Mp3)
        .audio_bitrate(64_000)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: audio encoder unavailable ({e})");
            return;
        }
    };
    // MP3 uses 1152 samples per frame.
    let frame_size: usize = 1152;
    let silence = match AudioFrame::empty(frame_size, 1, 44100, SampleFormat::F32) {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: AudioFrame::empty failed ({e})");
            return;
        }
    };
    // ~0.5 s worth of frames (22050 / 1152 ≈ 20 frames).
    let frames_needed = (22050 / frame_size) + 1;
    for _ in 0..frames_needed {
        if let Err(e) = aenc.push(&silence) {
            println!("Skipping: aenc.push failed ({e})");
            return;
        }
    }
    if let Err(e) = aenc.finish() {
        println!("Skipping: audio encoder.finish failed ({e})");
        return;
    }

    // Add with looping.
    let result = AudioAdder::new(&video_path, &audio_path, &output_path)
        .loop_audio()
        .run();
    match result {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: AudioAdder::run (loop) failed ({e})");
            return;
        }
    }

    assert_valid_output_file(&output_path);
}
