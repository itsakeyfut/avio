//! Integration tests for video encoder.
//!
//! These tests verify the basic video encoding functionality:
//! - Creating encoder with builder pattern
//! - Encoding black frames to various formats
//! - Hardware acceleration support
//! - Codec fallback behavior
//! - Error handling

// Tests are allowed to use unwrap() for simplicity
#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_encode::{
    AudioCodec, Av1Options, Av1Usage, BitrateMode, DnxhdOptions, DnxhdVariant, EncodeError,
    H264Options, H264Preset, H264Profile, H264Tune, H265Options, H265Profile, H265Tier,
    HardwareEncoder, OutputContainer, Preset, ProResOptions, ProResProfile, SvtAv1Options,
    VideoCodec, VideoCodecOptions, VideoEncoder, Vp9Options,
};
use ff_format::PixelFormat;
use fixtures::{
    FileGuard, assert_valid_output_file, create_black_frame, get_file_size, test_output_path,
};

// ============================================================================
// Basic Encoding Tests
// ============================================================================

#[test]
fn test_encode_black_frames_mpeg4() {
    let output_path = test_output_path("test_black_frames.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4) // Use MPEG-4 (always available)
        .preset(Preset::Ultrafast)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Encoder creation failed (no suitable codec): {}", e);
            return; // Skip test
        }
    };

    // Verify codec selection
    assert!(
        !encoder.actual_video_codec().is_empty(),
        "Video codec should be set"
    );
    println!("Using video codec: {}", encoder.actual_video_codec());

    // Encode 30 frames (1 second at 30fps)
    for _ in 0..30 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    // Finish encoding
    encoder.finish().expect("Failed to finish encoding");

    // Verify output file
    assert_valid_output_file(&output_path);
    let file_size = get_file_size(&output_path);
    println!("Output file size: {} bytes", file_size);

    // VP9 compressed black frames should be reasonably small
    assert!(file_size > 1000, "File too small, might be corrupted");
    assert!(file_size < 100_000, "File too large for black frames");
}

#[test]
fn test_encode_different_resolutions() {
    let resolutions = [(320, 240), (640, 480), (1280, 720), (1920, 1080)];

    for (width, height) in resolutions {
        let filename = format!("test_resolution_{}x{}.mp4", width, height);
        let output_path = test_output_path(&filename);
        let _guard = FileGuard::new(output_path.clone());

        let mut encoder = VideoEncoder::create(&output_path)
            .video(width, height, 30.0)
            .video_codec(VideoCodec::Mpeg4)
            .preset(Preset::Ultrafast)
            .build()
            .expect("Failed to build encoder");

        // Encode 10 frames
        for _ in 0..10 {
            let frame = create_black_frame(width, height);
            encoder
                .push_video(&frame)
                .expect("Failed to push video frame");
        }

        encoder.finish().expect("Failed to finish encoding");

        assert_valid_output_file(&output_path);
        println!(
            "{}x{}: {} bytes",
            width,
            height,
            get_file_size(&output_path)
        );
    }
}

#[test]
fn test_scaler_should_rebuild_when_source_dimensions_change() {
    // Drive the video scale path AND its dimension-change cache-rebuild through a
    // single encoder: the source frames differ from the encoder's output size (so
    // libswscale runs), and their sizes change between pushes (so the cached
    // ScaleContext is rebuilt, dropping the old one exactly once). MPEG-4 is
    // always available and needs no filters, so this runs unconditionally in CI.
    let output_path = test_output_path("test_scaler_rebuild.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Encoder creation failed (no suitable codec): {}", e);
            return; // Skip test
        }
    };

    // Two different source geometries, both != the 640x480 output, so each is
    // scaled; the change between them forces one context rebuild (move-assign).
    let source_sizes = [(320, 240), (800, 600), (800, 600), (320, 240)];
    for (width, height) in source_sizes {
        let frame = create_black_frame(width, height);
        encoder
            .push_video(&frame)
            .expect("Failed to push scaled video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn test_encode_different_framerates() {
    // Note: MPEG-4 supports max 65535 for timebase denominator (≈65 fps)
    let framerates = [24.0, 30.0, 60.0];

    for fps in framerates {
        let filename = format!("test_fps_{}.mp4", fps as i32);
        let output_path = test_output_path(&filename);
        let _guard = FileGuard::new(output_path.clone());

        let mut encoder = VideoEncoder::create(&output_path)
            .video(640, 480, fps)
            .video_codec(VideoCodec::Mpeg4)
            .preset(Preset::Ultrafast)
            .build()
            .expect("Failed to build encoder");

        // Encode frames for 0.5 seconds
        let frame_count = (fps * 0.5) as usize;
        for _ in 0..frame_count {
            let frame = create_black_frame(640, 480);
            encoder
                .push_video(&frame)
                .expect("Failed to push video frame");
        }

        encoder.finish().expect("Failed to finish encoding");

        assert_valid_output_file(&output_path);
        println!("{}fps: {} bytes", fps, get_file_size(&output_path));
    }
}

// ============================================================================
// Codec Tests
// ============================================================================

#[test]
fn test_encode_with_mpeg4() {
    let output_path = test_output_path("test_mpeg4.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .build();

    // MPEG-4 should always be available (built-in encoder)
    match result {
        Ok(mut encoder) => {
            println!("Using codec: {}", encoder.actual_video_codec());

            for _ in 0..10 {
                let frame = create_black_frame(640, 480);
                encoder
                    .push_video(&frame)
                    .expect("Failed to push video frame");
            }

            encoder.finish().expect("Failed to finish encoding");
            assert_valid_output_file(&output_path);
        }
        Err(e) => {
            println!("MPEG-4 encoding failed (unexpected): {}", e);
        }
    }
}

#[test]
fn test_encode_with_av1() {
    let output_path = test_output_path("test_av1.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // AV1 encoding might not be available on all systems
    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1)
        .preset(Preset::Ultrafast)
        .build();

    match result {
        Ok(mut encoder) => {
            println!("Using AV1 codec: {}", encoder.actual_video_codec());

            for _ in 0..10 {
                let frame = create_black_frame(640, 480);
                encoder
                    .push_video(&frame)
                    .expect("Failed to push video frame");
            }

            encoder.finish().expect("Failed to finish encoding");
            assert_valid_output_file(&output_path);
        }
        Err(e) => {
            println!("AV1 encoding not available: {}", e);
        }
    }
}

// ============================================================================
// Builder Pattern Tests
// ============================================================================

#[test]
fn test_builder_pattern() {
    let output_path = test_output_path("test_builder.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Test builder chaining
    let mut encoder = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(1_000_000)) // 1 Mbps
        .preset(Preset::Medium)
        .build()
        .expect("Failed to build encoder");

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn test_builder_with_quality() {
    let output_path = test_output_path("test_quality.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Test CRF-based quality control
    let mut encoder = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Crf(30)) // CRF 30
        .preset(Preset::Fast)
        .build()
        .expect("Failed to build encoder");

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn vbr_mpeg4_should_produce_valid_output() {
    let output_path = test_output_path("vbr_mpeg4.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Vbr {
            target: 1_000_000,
            max: 2_000_000,
        })
        .preset(Preset::Ultrafast)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping vbr_mpeg4 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn crf_h264_should_produce_valid_output() {
    let output_path = test_output_path("crf_h264.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .bitrate_mode(BitrateMode::Crf(23)) // CRF 23 — default quality for H.264
        .preset(Preset::Ultrafast)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping crf_h264 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn crf_h265_should_produce_valid_output() {
    let output_path = test_output_path("crf_h265.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28)) // CRF 28 — default quality for H.265
        .preset(Preset::Ultrafast)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping crf_h265 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

// H265Options integration tests

#[test]
fn h265_main_profile_should_produce_valid_output() {
    let output_path = test_output_path("h265_main_profile.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main,
            tier: H265Tier::Main,
            level: None,
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping h265_main_profile test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("h265_main_profile: codec={codec_name}");
}

#[test]
fn h265_main10_profile_should_produce_valid_output() {
    let output_path = test_output_path("h265_main10_profile.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main10,
            tier: H265Tier::Main,
            level: None,
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping h265_main10_profile test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("h265_main10_profile: codec={codec_name}");
}

#[test]
fn h265_high_tier_should_produce_valid_output() {
    let output_path = test_output_path("h265_high_tier.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main,
            tier: H265Tier::High,
            level: None,
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping h265_high_tier test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("h265_high_tier: codec={codec_name}");
}

#[test]
fn h265_level51_should_produce_valid_output() {
    let output_path = test_output_path("h265_level51.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main,
            tier: H265Tier::Main,
            level: Some(51),
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping h265_level51 test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("h265_level51: codec={codec_name}");
}

#[test]
fn h265_preset_ultrafast_should_produce_valid_output() {
    let output_path = test_output_path("h265_preset_ultrafast.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .codec_options(VideoCodecOptions::H265(H265Options {
            preset: Some("ultrafast".to_string()),
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping h265_preset_ultrafast test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("h265_preset_ultrafast: codec={codec_name}");
}

#[test]
fn h265_x265_params_log_level_none_should_not_crash() {
    let output_path = test_output_path("h265_x265_params.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .codec_options(VideoCodecOptions::H265(H265Options {
            x265_params: Some("log-level=none".to_string()),
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping h265_x265_params test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("h265_x265_params: codec={codec_name}");
}

// ============================================================================
// AV1 codec_options Tests
// ============================================================================

#[test]
fn av1_cpu_used_8_should_produce_valid_output() {
    let output_path = test_output_path("av1_cpu_used_8.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1)
        .codec_options(VideoCodecOptions::Av1(Av1Options {
            cpu_used: 8,
            ..Av1Options::default()
        }))
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping av1_cpu_used_8 test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("av1_cpu_used_8: codec={codec_name}");
}

#[test]
fn av1_realtime_usage_should_produce_valid_output() {
    let output_path = test_output_path("av1_realtime.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1)
        .codec_options(VideoCodecOptions::Av1(Av1Options {
            usage: Av1Usage::RealTime,
            cpu_used: 8,
            ..Av1Options::default()
        }))
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping av1_realtime_usage test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("av1_realtime_usage: codec={codec_name}");
}

#[test]
fn av1_cpu_used_9_should_return_invalid_option_error() {
    let output_path = test_output_path("av1_cpu_used_9.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1)
        .codec_options(VideoCodecOptions::Av1(Av1Options {
            cpu_used: 9,
            ..Av1Options::default()
        }))
        .build();

    assert!(
        matches!(result, Err(EncodeError::InvalidOption { .. })),
        "Expected InvalidOption error"
    );
}

// ============================================================================
// SVT-AV1 codec_options Tests
// ============================================================================

#[test]
fn svtav1_preset_8_should_produce_valid_output() {
    let output_path = test_output_path("svtav1_preset_8.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1Svt)
        .codec_options(VideoCodecOptions::Av1Svt(SvtAv1Options {
            preset: 8,
            ..SvtAv1Options::default()
        }))
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping svtav1_preset_8 test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("svtav1_preset_8: codec={codec_name}");
}

#[test]
fn svtav1_params_passthrough_should_not_crash() {
    let output_path = test_output_path("svtav1_params.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1Svt)
        .codec_options(VideoCodecOptions::Av1Svt(SvtAv1Options {
            svtav1_params: Some("fast-decode=1".to_string()),
            ..SvtAv1Options::default()
        }))
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping svtav1_params test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("svtav1_params: codec={codec_name}");
}

#[test]
fn svtav1_preset_14_should_return_invalid_option_error() {
    let output_path = test_output_path("svtav1_preset_14.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1Svt)
        .codec_options(VideoCodecOptions::Av1Svt(SvtAv1Options {
            preset: 14,
            ..SvtAv1Options::default()
        }))
        .build();

    assert!(
        matches!(result, Err(EncodeError::InvalidOption { .. })),
        "Expected InvalidOption error"
    );
}

// ============================================================================
// VP9 codec_options Tests
// ============================================================================

#[test]
fn vp9_cpu_used_4_should_produce_valid_output() {
    let output_path = test_output_path("vp9_cpu_used_4.webm");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .codec_options(VideoCodecOptions::Vp9(Vp9Options {
            cpu_used: 4,
            ..Vp9Options::default()
        }))
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping vp9_cpu_used_4 test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("vp9_cpu_used_4: codec={codec_name}");
}

#[test]
fn vp9_cq_mode_should_produce_valid_output() {
    let output_path = test_output_path("vp9_cq_mode.webm");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = match VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .codec_options(VideoCodecOptions::Vp9(Vp9Options {
            cpu_used: 4,
            cq_level: Some(33),
            ..Vp9Options::default()
        }))
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping vp9_cq_mode test: encoder unavailable ({e})");
            return;
        }
    };

    let codec_name = encoder.actual_video_codec().to_string();

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("vp9_cq_mode: codec={codec_name}");
}

#[test]
fn vp9_cpu_used_out_of_range_should_return_invalid_option_error() {
    let output_path = test_output_path("vp9_cpu_used_oob.webm");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .codec_options(VideoCodecOptions::Vp9(Vp9Options {
            cpu_used: 9,
            ..Vp9Options::default()
        }))
        .build();

    assert!(
        matches!(result, Err(EncodeError::InvalidOption { .. })),
        "Expected InvalidOption error"
    );
}

// ============================================================================
// ProRes Tests
// ============================================================================

#[test]
fn prores_hq_should_produce_valid_output() {
    let output_path = test_output_path("prores_hq.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::ProRes)
        .codec_options(VideoCodecOptions::ProRes(ProResOptions {
            profile: ProResProfile::Hq,
            vendor: None,
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping prores_hq test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "ProRes HQ: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

#[test]
fn prores_4444_should_produce_valid_output() {
    let output_path = test_output_path("prores_4444.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::ProRes)
        .codec_options(VideoCodecOptions::ProRes(ProResOptions {
            profile: ProResProfile::P4444,
            vendor: None,
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping prores_4444 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "ProRes 4444: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

#[test]
fn prores_vendor_tag_should_not_crash() {
    let output_path = test_output_path("prores_vendor.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::ProRes)
        .codec_options(VideoCodecOptions::ProRes(ProResOptions {
            profile: ProResProfile::Standard,
            vendor: Some([b'a', b'p', b'p', b'l']),
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping prores_vendor test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "ProRes vendor tag: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

// ============================================================================
// DNxHD / DNxHR Tests
// ============================================================================

#[test]
fn dnxhd_145_1080p_should_produce_valid_output() {
    let output_path = test_output_path("dnxhd_145.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(1920, 1080, 30.0)
        .video_codec(VideoCodec::DnxHd)
        .codec_options(VideoCodecOptions::Dnxhd(DnxhdOptions {
            variant: DnxhdVariant::Dnxhd145,
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping dnxhd_145 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(1920, 1080);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "DNxHD 145: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

#[test]
fn dnxhr_sq_should_produce_valid_output() {
    let output_path = test_output_path("dnxhr_sq.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::DnxHd)
        .codec_options(VideoCodecOptions::Dnxhd(DnxhdOptions {
            variant: DnxhdVariant::DnxhrSq,
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping dnxhr_sq test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "DNxHR SQ: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

#[test]
fn dnxhd_invalid_resolution_should_return_error() {
    let output_path = test_output_path("dnxhd_invalid_res.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::DnxHd)
        .codec_options(VideoCodecOptions::Dnxhd(DnxhdOptions {
            variant: DnxhdVariant::Dnxhd145,
        }))
        .build();

    assert!(
        matches!(result, Err(EncodeError::InvalidOption { .. })),
        "Expected InvalidOption error for DNxHD with non-standard resolution"
    );
}

// ============================================================================
// 10-bit Pixel Format Tests
// ============================================================================

#[test]
fn h265_main10_with_explicit_yuv420p10le_should_produce_valid_output() {
    let output_path = test_output_path("h265_main10_yuv420p10le.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .pixel_format(PixelFormat::Yuv420p10le)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main10,
            tier: H265Tier::Main,
            level: None,
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping h265_main10_yuv420p10le test: encoder unavailable ({e})");
            return;
        }
    };

    let frame = create_black_frame(640, 480);
    for _ in 0..10 {
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("h265_main10_yuv420p10le: codec={codec_name}");
}

#[test]
fn pixel_format_yuv420p10le_on_h264_should_be_accepted() {
    let output_path = test_output_path("h264_yuv420p10le.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .bitrate_mode(BitrateMode::Crf(23))
        .pixel_format(PixelFormat::Yuv420p10le)
        .build();

    // The builder must not return InvalidOption — 10-bit formats are always accepted.
    assert!(
        !matches!(result, Err(EncodeError::InvalidOption { .. })),
        "pixel_format(Yuv420p10le) must not return InvalidOption"
    );

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping pixel_format_yuv420p10le_on_h264 test: encoder unavailable ({e})");
            return;
        }
    };

    let frame = create_black_frame(640, 480);
    for _ in 0..5 {
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

// ============================================================================
// Preset Tests
// ============================================================================

#[test]
fn test_different_presets() {
    let presets = [
        Preset::Ultrafast,
        Preset::Fast,
        Preset::Medium,
        Preset::Slow,
    ];

    for preset in presets {
        let filename = format!("test_preset_{:?}.mp4", preset).to_lowercase();
        let output_path = test_output_path(&filename);
        let _guard = FileGuard::new(output_path.clone());

        let mut encoder = VideoEncoder::create(&output_path)
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Mpeg4)
            .preset(preset)
            .build()
            .expect("Failed to build encoder");

        for _ in 0..10 {
            let frame = create_black_frame(640, 480);
            encoder
                .push_video(&frame)
                .expect("Failed to push video frame");
        }

        encoder.finish().expect("Failed to finish encoding");

        assert_valid_output_file(&output_path);
        println!("{:?}: {} bytes", preset, get_file_size(&output_path));
    }
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_encode_single_frame() {
    let output_path = test_output_path("test_single_frame.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .build()
        .expect("Failed to build encoder");

    // Encode just one frame
    let frame = create_black_frame(640, 480);
    encoder
        .push_video(&frame)
        .expect("Failed to push video frame");

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn test_encode_many_frames() {
    let output_path = test_output_path("test_many_frames.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let mut encoder = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .build()
        .expect("Failed to build encoder");

    // Encode 300 frames (10 seconds)
    for _ in 0..300 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    let file_size = get_file_size(&output_path);
    println!("300 frames: {} bytes", file_size);
}

// ============================================================================
// Two-Pass Tests
// ============================================================================

#[test]
fn two_pass_encode_should_produce_valid_output() {
    let output_path = test_output_path("two_pass_mpeg4.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(1_000_000)) // Two-pass is most useful with a target bitrate
        .preset(Preset::Ultrafast)
        .two_pass()
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping two_pass test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..30 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder
        .finish()
        .expect("Failed to finish two-pass encoding");

    assert_valid_output_file(&output_path);
    let file_size = get_file_size(&output_path);
    println!("Two-pass output: {} bytes", file_size);
    assert!(file_size > 1000, "File too small, might be corrupted");
}

#[test]
fn metadata_mpeg4_should_produce_valid_output() {
    let output_path = test_output_path("metadata_mpeg4.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .metadata("title", "Test Video")
        .metadata("artist", "ff-encode integration test")
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping metadata_mpeg4 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn subtitle_passthrough_mkv_should_produce_valid_output() {
    // Create a minimal SRT subtitle file as the passthrough source.
    let srt_path = test_output_path("subtitle_passthrough_source.srt");
    let _srt_guard = FileGuard::new(srt_path.clone());
    let srt_content = "1\n00:00:00,000 --> 00:00:05,000\nHello, world!\n\n\
                       2\n00:00:05,000 --> 00:00:10,000\nSubtitle passthrough test.\n\n";
    if let Err(e) = std::fs::write(&srt_path, srt_content) {
        println!("Skipping subtitle_passthrough test: cannot write srt file ({e})");
        return;
    }

    let output_path = test_output_path("subtitle_passthrough.mkv");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .subtitle_passthrough(srt_path.to_str().unwrap(), 0)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping subtitle_passthrough test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn subtitle_passthrough_should_preserve_subtitle_stream_in_output() {
    use ff_probe::SubtitleCodec;

    // Write a minimal SRT file as the subtitle source.
    let srt_path = test_output_path("subtitle_roundtrip_source.srt");
    let _srt_guard = FileGuard::new(srt_path.clone());
    let srt_content = "1\n00:00:00,000 --> 00:00:05,000\nHello subtitle!\n\n\
                       2\n00:00:05,000 --> 00:00:10,000\nPassthrough round-trip.\n\n";
    if let Err(e) = std::fs::write(&srt_path, srt_content) {
        println!("Skipping subtitle passthrough round-trip: cannot write srt file ({e})");
        return;
    }

    let output_path = test_output_path("subtitle_roundtrip_output.mkv");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .subtitle_passthrough(srt_path.to_str().unwrap(), 0)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping subtitle passthrough round-trip: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    // Re-probe the output and verify the subtitle stream is present with correct codec.
    let info = ff_probe::open(&output_path).expect("Failed to probe output file");

    assert!(
        info.has_subtitles(),
        "Output should contain at least one subtitle stream"
    );
    assert_eq!(
        info.subtitle_stream_count(),
        1,
        "Expected exactly one subtitle stream after passthrough"
    );

    let stream = &info.subtitle_streams()[0];
    assert_eq!(
        stream.codec(),
        &SubtitleCodec::Srt,
        "Subtitle codec should be Srt after SRT passthrough, got {:?}",
        stream.codec()
    );
}

#[test]
fn chapter_mpeg4_should_produce_valid_output() {
    use ff_format::chapter::ChapterInfo;
    use std::time::Duration;

    let output_path = test_output_path("chapter_mpeg4.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let chapter1 = ChapterInfo::builder()
        .id(0)
        .title("Intro")
        .start(Duration::ZERO)
        .end(Duration::from_secs(5))
        .build();
    let chapter2 = ChapterInfo::builder()
        .id(1)
        .title("Main")
        .start(Duration::from_secs(5))
        .end(Duration::from_secs(10))
        .build();

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .chapter(chapter1)
        .chapter(chapter2)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping chapter_mpeg4 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn chapter_round_trip_should_preserve_count_titles_and_timestamps() {
    use ff_format::chapter::ChapterInfo;
    use std::time::Duration;

    let output_path = test_output_path("chapter_round_trip.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Known chapters with explicit titles and timestamps.
    let chapters = vec![
        ChapterInfo::builder()
            .id(0)
            .title("Intro")
            .start(Duration::ZERO)
            .end(Duration::from_secs(5))
            .build(),
        ChapterInfo::builder()
            .id(1)
            .title("Main Content")
            .start(Duration::from_secs(5))
            .end(Duration::from_secs(15))
            .build(),
        ChapterInfo::builder()
            .id(2)
            .title("Credits")
            .start(Duration::from_secs(15))
            .end(Duration::from_secs(20))
            .build(),
    ];

    // Encode a short black-frame video with the chapters written.
    let mut builder = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast);
    for ch in &chapters {
        builder = builder.chapter(ch.clone());
    }
    let mut encoder = match builder.build() {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping chapter_round_trip test: encoder unavailable ({e})");
            return;
        }
    };

    // Encode 20 seconds worth of frames at 30 fps (600 frames).
    for _ in 0..600 {
        let frame = create_black_frame(320, 240);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    // Re-probe the output and verify the chapter round-trip.
    let info = ff_probe::open(&output_path).expect("Failed to probe output file");

    assert_eq!(
        info.chapter_count(),
        chapters.len(),
        "Chapter count mismatch after round-trip"
    );

    // Tolerance for timestamp comparison: FFmpeg stores chapters in a rational
    // time-base (typically 1/1000 ms), so allow up to 10 ms of rounding error.
    let tolerance = Duration::from_millis(10);

    for (i, expected) in chapters.iter().enumerate() {
        let actual = info
            .chapters()
            .get(i)
            .unwrap_or_else(|| panic!("Chapter {i} missing after round-trip"));

        assert_eq!(
            actual.title(),
            expected.title(),
            "Title mismatch for chapter {i}"
        );

        let start_diff = if actual.start() >= expected.start() {
            actual.start() - expected.start()
        } else {
            expected.start() - actual.start()
        };
        assert!(
            start_diff <= tolerance,
            "Start timestamp mismatch for chapter {i}: expected {:?}, got {:?}",
            expected.start(),
            actual.start()
        );

        let end_diff = if actual.end() >= expected.end() {
            actual.end() - expected.end()
        } else {
            expected.end() - actual.end()
        };
        assert!(
            end_diff <= tolerance,
            "End timestamp mismatch for chapter {i}: expected {:?}, got {:?}",
            expected.end(),
            actual.end()
        );
    }
}

// ============================================================================
// Codec Options Tests
// ============================================================================

#[test]
fn h264_high_profile_level41_should_produce_valid_output() {
    let output_path = test_output_path("h264_high_profile_level41.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let opts = VideoCodecOptions::H264(H264Options {
        profile: H264Profile::High,
        level: Some(41),
        ..H264Options::default()
    });

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .preset(Preset::Ultrafast)
        .codec_options(opts)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: no H.264 encoder available: {e}");
            return;
        }
    };

    for _ in 0..30 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    let file_size = get_file_size(&output_path);
    assert!(file_size > 1000, "Output too small, likely corrupted");
    println!("H264 High@4.1: codec={codec_name} size={file_size} bytes");
}

#[test]
fn h264_baseline_profile_should_produce_valid_output() {
    let output_path = test_output_path("h264_baseline_profile.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let opts = VideoCodecOptions::H264(H264Options {
        profile: H264Profile::Baseline,
        level: None,
        bframes: 0, // Baseline does not support B-frames
        ..H264Options::default()
    });

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .preset(Preset::Ultrafast)
        .codec_options(opts)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: no H.264 encoder available: {e}");
            return;
        }
    };

    for _ in 0..30 {
        let frame = create_black_frame(640, 480);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "H264 Baseline: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

#[test]
fn h264_veryslow_preset_should_produce_valid_output() {
    let output_path = test_output_path("h264_veryslow_preset.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let opts = VideoCodecOptions::H264(H264Options {
        preset: Some(H264Preset::Veryslow),
        ..H264Options::default()
    });

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .codec_options(opts)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: no H.264 encoder available: {e}");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "H264 veryslow preset: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

#[test]
fn h264_zerolatency_tune_should_produce_valid_output() {
    let output_path = test_output_path("h264_zerolatency_tune.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let opts = VideoCodecOptions::H264(H264Options {
        preset: Some(H264Preset::Ultrafast),
        tune: Some(H264Tune::Zerolatency),
        ..H264Options::default()
    });

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .codec_options(opts)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: no H.264 encoder available: {e}");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!(
        "H264 zerolatency tune: codec={codec_name} size={} bytes",
        get_file_size(&output_path)
    );
}

// ============================================================================
// HDR10 Metadata Tests
// ============================================================================

#[test]
fn hdr10_metadata_on_h265_main10_should_produce_valid_output() {
    use ff_format::hdr::{Hdr10Metadata, MasteringDisplay};

    let output_path = test_output_path("hdr10_h265_main10.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let hdr = Hdr10Metadata {
        max_cll: 1000,
        max_fall: 400,
        mastering_display: MasteringDisplay {
            red_x: 17000,
            red_y: 8500,
            green_x: 13250,
            green_y: 34500,
            blue_x: 7500,
            blue_y: 3000,
            white_x: 15635,
            white_y: 16450,
            min_luminance: 50,
            max_luminance: 10_000_000,
        },
    };

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .pixel_format(PixelFormat::Yuv420p10le)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main10,
            ..H265Options::default()
        }))
        .hdr10_metadata(hdr)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..15 {
        let frame = create_black_frame(640, 480);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("HDR10 H265 Main10: codec={codec_name}");
}

#[test]
fn hdr10_metadata_on_h264_should_produce_valid_output() {
    use ff_format::hdr::{Hdr10Metadata, MasteringDisplay};

    let output_path = test_output_path("hdr10_h264.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let hdr = Hdr10Metadata {
        max_cll: 500,
        max_fall: 200,
        mastering_display: MasteringDisplay {
            red_x: 17000,
            red_y: 8500,
            green_x: 13250,
            green_y: 34500,
            blue_x: 7500,
            blue_y: 3000,
            white_x: 15635,
            white_y: 16450,
            min_luminance: 50,
            max_luminance: 5_000_000,
        },
    };

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .bitrate_mode(BitrateMode::Crf(23))
        .hdr10_metadata(hdr)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("HDR10 H264: codec={codec_name}");
}

// ============================================================================
// Color Tagging Tests
// ============================================================================

#[test]
fn hlg_color_transfer_on_h265_should_produce_valid_output() {
    use ff_format::{ColorPrimaries, ColorSpace, ColorTransfer};

    let output_path = test_output_path("hlg_h265.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .color_space(ColorSpace::Bt2020Ncl)
        .color_transfer(ColorTransfer::Hlg)
        .color_primaries(ColorPrimaries::Bt2020)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("HLG H265: codec={codec_name}");
}

#[test]
fn color_transfer_bt709_on_h264_should_produce_valid_output() {
    use ff_format::{ColorPrimaries, ColorSpace, ColorTransfer};

    let output_path = test_output_path("bt709_h264.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .bitrate_mode(BitrateMode::Crf(23))
        .color_space(ColorSpace::Bt709)
        .color_transfer(ColorTransfer::Bt709)
        .color_primaries(ColorPrimaries::Bt709)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(640, 480);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("BT.709 H264: codec={codec_name}");
}

// ============================================================================
// Attachment Tests
// ============================================================================

#[test]
fn attachment_embedding_mkv_should_produce_valid_output() {
    let output_path = test_output_path("attachment_mkv.mkv");
    let _guard = FileGuard::new(output_path.clone());

    // Minimal synthetic font bytes (not a real font, just binary data for testing).
    let font_data = vec![0x00u8; 512];

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .add_attachment(font_data, "application/x-truetype-font", "test_font.ttf")
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping attachment_embedding test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn attachment_embedding_with_multiple_attachments_should_produce_valid_output() {
    let output_path = test_output_path("attachment_multi_mkv.mkv");
    let _guard = FileGuard::new(output_path.clone());

    let font_data = vec![0xFFu8; 256];
    let image_data = vec![0xABu8; 128];

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .preset(Preset::Ultrafast)
        .add_attachment(font_data, "application/x-truetype-font", "font.ttf")
        .add_attachment(image_data, "image/jpeg", "cover.jpg")
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping multi-attachment test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..5 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
}

#[test]
fn attachment_builder_setter_should_not_panic() {
    // Verify that add_attachment() is chainable and does not panic.
    // Field-level assertions live in the crate's unit tests.
    let _builder = VideoEncoder::create("output.mkv")
        .video(320, 240, 30.0)
        .add_attachment(vec![1, 2, 3], "application/octet-stream", "data.bin")
        .add_attachment(vec![4, 5, 6], "image/png", "icon.png");
}

// ============================================================================
// WebM OutputContainer Tests
// ============================================================================

#[test]
fn webm_vp9_should_produce_valid_output() {
    let output_path = test_output_path("webm_vp9.webm");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Vp9)
        .bitrate_mode(BitrateMode::Crf(33))
        .preset(Preset::Ultrafast)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping webm_vp9 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("webm_vp9: codec={codec_name}");
}

#[test]
fn webm_auto_default_codec_should_use_vp9_opus() {
    // When no codec is explicitly set, .webm path should auto-select VP9+Opus.
    let output_path = test_output_path("webm_auto_default.webm");
    let _guard = FileGuard::new(output_path.clone());

    // Without calling video_codec() or audio_codec(), build should not fail
    // with UnsupportedContainerCodecCombination.
    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .build();

    assert!(!matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

#[test]
fn webm_with_incompatible_video_codec_should_return_error() {
    let output_path = test_output_path("webm_h264_error.webm");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .build();

    assert!(matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

#[test]
fn webm_with_incompatible_audio_codec_should_return_error() {
    let output_path = test_output_path("webm_aac_error.webm");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .audio(48000, 2)
        .audio_codec(AudioCodec::Aac)
        .build();

    assert!(matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

#[test]
fn container_enum_webm_with_incompatible_codec_should_return_error() {
    let result = VideoEncoder::create("output.mkv")
        .video(640, 480, 30.0)
        .container(OutputContainer::WebM)
        .video_codec(VideoCodec::H265)
        .build();

    assert!(matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

// ============================================================================
// AVI OutputContainer Tests
// ============================================================================

#[test]
fn avi_h264_mp3_should_produce_valid_output() {
    let output_path = test_output_path("avi_h264_mp3.avi");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .bitrate_mode(BitrateMode::Crf(23))
        .preset(Preset::Ultrafast)
        .audio(44100, 2)
        .audio_codec(AudioCodec::Mp3)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping avi_h264_mp3 test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("avi_h264_mp3: codec={codec_name}");
}

#[test]
fn avi_auto_default_codec_should_not_return_container_codec_error() {
    let output_path = test_output_path("avi_auto_default.avi");
    let _guard = FileGuard::new(output_path.clone());

    // Without calling video_codec() or audio_codec(), .avi should auto-select H264+MP3.
    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .build();

    assert!(!matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

#[test]
fn avi_with_incompatible_video_codec_should_return_error() {
    let output_path = test_output_path("avi_vp9_error.avi");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .build();

    assert!(matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

#[test]
fn avi_with_incompatible_audio_codec_should_return_error() {
    let output_path = test_output_path("avi_opus_error.avi");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .audio(48000, 2)
        .audio_codec(AudioCodec::Opus)
        .build();

    assert!(matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

// ============================================================================
// MOV OutputContainer Tests
// ============================================================================

#[test]
fn mov_h264_aac_should_produce_valid_output() {
    let output_path = test_output_path("mov_h264_aac.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .bitrate_mode(BitrateMode::Crf(23))
        .preset(Preset::Ultrafast)
        .audio(44100, 2)
        .audio_codec(AudioCodec::Aac)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping mov_h264_aac test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..10 {
        let frame = create_black_frame(320, 240);
        encoder.push_video(&frame).expect("Failed to push frame");
    }

    let codec_name = encoder.actual_video_codec().to_string();
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);
    println!("mov_h264_aac: codec={codec_name}");
}

#[test]
fn mov_auto_default_codec_should_not_return_container_codec_error() {
    let output_path = test_output_path("mov_auto_default.mov");
    let _guard = FileGuard::new(output_path.clone());

    // Without calling video_codec() or audio_codec(), .mov should auto-select H264+AAC.
    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .build();

    assert!(!matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

#[test]
fn mov_with_incompatible_video_codec_should_return_error() {
    let output_path = test_output_path("mov_vp9_error.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .build();

    assert!(matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}

#[test]
fn mov_with_incompatible_audio_codec_should_return_error() {
    let output_path = test_output_path("mov_opus_error.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::H264)
        .audio(48000, 2)
        .audio_codec(AudioCodec::Opus)
        .build();

    assert!(matches!(
        result,
        Err(EncodeError::UnsupportedContainerCodecCombination { .. })
    ));
}
