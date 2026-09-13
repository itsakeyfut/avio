//! Codec selection tests.
//!
//! Tests how an encoder is chosen for a requested codec:
//! - within the requested codec's own family (hardware, then software)
//! - across families, which is opt-in and never silent
//! - LGPL compliance verification
//! - Hardware encoder preference

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_encode::{EncodeError, HardwareEncoder, VideoCodec, VideoEncoder};
use fixtures::{FileGuard, assert_valid_output_file, create_black_frame, test_output_path};

/// Whether an encoder name belongs to the H.264 family.
///
/// Covers `libx264` and every `h264_*` hardware encoder.
fn is_h264_encoder(name: &str) -> bool {
    name.contains("264")
}

/// Whether an encoder name belongs to the HEVC family.
fn is_hevc_encoder(name: &str) -> bool {
    name.contains("265") || name.contains("hevc")
}

// ============================================================================
// H.264 Codec Selection Tests
// ============================================================================

#[test]
fn requesting_h264_should_yield_h264_or_fail() {
    let output_path = test_output_path("test_h264_family.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(1280, 720, 30.0)
        .video_codec(VideoCodec::H264)
        .build();

    // The outcome depends on what this FFmpeg build registers, so the assertion
    // is on the shape: H.264 or a refusal, never a different codec.
    match result {
        Ok(encoder) => {
            let actual_codec = encoder.actual_video_codec();
            assert!(
                is_h264_encoder(actual_codec),
                "requested H.264 and got {actual_codec}, which is a different codec"
            );
        }
        Err(EncodeError::EncoderUnavailable { codec, hint }) => {
            println!("no H.264 encoder: codec={codec} hint={hint}");
            assert!(
                !hint.is_empty(),
                "the refusal must say what would be needed"
            );
        }
        Err(EncodeError::NoSuitableEncoder { .. }) => {
            println!("skipped: this build registers no H.264 encoder and no stand-in");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[test]
fn test_h264_hardware_preference() {
    let output_path = test_output_path("test_h264_hw.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Request H.264 with explicit hardware encoder preference
    let result = VideoEncoder::create(&output_path)
        .video(1280, 720, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::Auto)
        .build();

    match result {
        Ok(encoder) => {
            let actual_codec = encoder.actual_video_codec();
            println!("H.264 with HW auto selected: {}", actual_codec);

            // Check if hardware encoder was selected
            if encoder.is_hardware_encoding() {
                println!("✓ Hardware encoder is being used");
                assert!(
                    actual_codec.contains("nvenc")
                        || actual_codec.contains("qsv")
                        || actual_codec.contains("amf")
                        || actual_codec.contains("videotoolbox")
                        || actual_codec.contains("vaapi"),
                    "Should use hardware encoder, got: {}",
                    actual_codec
                );
            } else {
                println!("⚠ Hardware encoder not available, using software fallback");
            }
        }
        Err(e) => {
            println!("H.264 hardware encoder creation failed: {}", e);
        }
    }
}

#[test]
fn test_h264_software_only() {
    let output_path = test_output_path("test_h264_sw.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Request H.264 with explicit software-only encoding
    let result = VideoEncoder::create(&output_path)
        .video(1280, 720, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .build();

    match result {
        Ok(encoder) => {
            let actual_codec = encoder.actual_video_codec();
            println!("H.264 software-only selected: {}", actual_codec);

            // Should not be a hardware encoder
            assert!(
                !encoder.is_hardware_encoding(),
                "Should use software encoder"
            );

            // Whatever was chosen, it must still be H.264. Without the `gpl`
            // feature there is usually nothing left to choose and this arm is
            // not reached at all.
            assert!(
                is_h264_encoder(actual_codec),
                "requested H.264 and got {actual_codec}, which is a different codec"
            );
        }
        Err(e) => {
            println!("H.264 software encoder creation failed: {}", e);
        }
    }
}

// ============================================================================
// H.265 Codec Selection Tests
// ============================================================================

#[test]
fn requesting_h265_should_yield_hevc_or_fail() {
    let output_path = test_output_path("test_h265_fallback.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Request H.265, let the encoder choose the best available
    let result = VideoEncoder::create(&output_path)
        .video(1280, 720, 30.0)
        .video_codec(VideoCodec::H265)
        .build();

    match result {
        Ok(encoder) => {
            let actual_codec = encoder.actual_video_codec();
            println!("H.265 selected codec: {}", actual_codec);

            assert!(
                is_hevc_encoder(actual_codec),
                "requested H.265 and got {actual_codec}, which is a different codec"
            );
        }
        Err(e) => {
            println!("H.265 encoder creation failed: {}", e);
        }
    }
}

#[test]
fn allow_codec_substitution_should_permit_what_the_default_refuses() {
    let refused_path = test_output_path("test_substitution_refused.mp4");
    let _refused_guard = FileGuard::new(refused_path.clone());

    let refused = VideoEncoder::create(&refused_path)
        .video(320, 180, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .build();

    // Only meaningful where the default refused because a stand-in exists.
    let Err(EncodeError::EncoderUnavailable { .. }) = refused else {
        println!("skipped: this build does not reach the substitution decision");
        return;
    };

    let allowed_path = test_output_path("test_substitution_allowed.mp4");
    let _allowed_guard = FileGuard::new(allowed_path.clone());

    let encoder = VideoEncoder::create(&allowed_path)
        .video(320, 180, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .allow_codec_substitution()
        .build()
        .expect("opting in should accept the stand-in the default refused");

    println!(
        "substitution accepted: encoder={}",
        encoder.actual_video_codec()
    );
}

// ============================================================================
// LGPL Compliance Tests
// ============================================================================

#[test]
fn test_lgpl_compliance_without_gpl_feature() {
    #[cfg(not(feature = "gpl"))]
    {
        let output_path = test_output_path("test_lgpl_compliance.mp4");
        let _guard = FileGuard::new(output_path.clone());

        // Request H.264 without GPL feature enabled
        let result = VideoEncoder::create(&output_path)
            .video(1280, 720, 30.0)
            .video_codec(VideoCodec::H264)
            .hardware_encoder(HardwareEncoder::None) // Force software encoding
            .build();

        match result {
            Ok(encoder) => {
                let actual_codec = encoder.actual_video_codec();
                println!("LGPL-compliant codec selected: {}", actual_codec);

                // Without GPL feature, should always be LGPL-compliant
                assert!(
                    encoder.is_lgpl_compliant(),
                    "Encoder should be LGPL-compliant, got: {}",
                    actual_codec
                );
            }
            Err(e) => {
                println!("LGPL-compliant encoder creation failed: {}", e);
            }
        }
    }

    #[cfg(feature = "gpl")]
    {
        println!("Skipping LGPL test (GPL feature is enabled)");
    }
}

#[test]
fn test_lgpl_compliance_with_hardware() {
    let output_path = test_output_path("test_lgpl_hw.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Request H.264 with hardware encoder (should be LGPL-compliant)
    let result = VideoEncoder::create(&output_path)
        .video(1280, 720, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::Auto)
        .build();

    match result {
        Ok(encoder) => {
            let actual_codec = encoder.actual_video_codec();
            println!("Hardware codec selected: {}", actual_codec);

            // Hardware encoders are always LGPL-compliant
            if encoder.is_hardware_encoding() {
                assert!(
                    encoder.is_lgpl_compliant(),
                    "Hardware encoder should be LGPL-compliant: {}",
                    actual_codec
                );
                println!("✓ Hardware encoder is LGPL-compliant");
            }
        }
        Err(e) => {
            println!("Hardware encoder creation failed: {}", e);
        }
    }
}

// ============================================================================
// Specific Codec Tests
// ============================================================================

#[test]
fn test_vp9_codec() {
    let output_path = test_output_path("test_vp9_codec.webm");
    let _guard = FileGuard::new(output_path.clone());

    // Request VP9 explicitly
    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .build();

    match result {
        Ok(mut encoder) => {
            let actual_codec = encoder.actual_video_codec();
            println!("VP9 codec: {}", actual_codec);

            assert!(
                actual_codec.contains("vp9"),
                "Expected VP9 codec, got: {}",
                actual_codec
            );

            // VP9 should always be LGPL-compliant
            assert!(encoder.is_lgpl_compliant(), "VP9 should be LGPL-compliant");

            // Encode a few frames to verify it works
            for _ in 0..10 {
                let frame = create_black_frame(640, 480);
                encoder.push_video(&frame).expect("Failed to push frame");
            }

            encoder.finish().expect("Failed to finish encoding");
            assert_valid_output_file(&output_path);
        }
        Err(e) => {
            println!("VP9 encoder creation failed: {}", e);
        }
    }
}

#[test]
fn test_av1_codec() {
    let output_path = test_output_path("test_av1_codec.webm");
    let _guard = FileGuard::new(output_path.clone());

    // Request AV1 explicitly
    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Av1)
        .build();

    match result {
        Ok(encoder) => {
            let actual_codec = encoder.actual_video_codec();
            println!("AV1 codec: {}", actual_codec);

            assert!(
                actual_codec.contains("av1") || actual_codec.contains("aom"),
                "Expected AV1 codec, got: {}",
                actual_codec
            );

            // AV1 should always be LGPL-compliant
            assert!(encoder.is_lgpl_compliant(), "AV1 should be LGPL-compliant");
        }
        Err(e) => {
            println!("AV1 encoder not available: {}", e);
        }
    }
}

#[test]
fn test_mpeg4_codec() {
    let output_path = test_output_path("test_mpeg4_codec.mp4");
    let _guard = FileGuard::new(output_path.clone());

    // Request MPEG-4 explicitly (should always be available)
    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .build();

    match result {
        Ok(mut encoder) => {
            let actual_codec = encoder.actual_video_codec();
            println!("MPEG-4 codec: {}", actual_codec);

            assert!(
                actual_codec.contains("mpeg4"),
                "Expected MPEG-4 codec, got: {}",
                actual_codec
            );

            // MPEG-4 should be LGPL-compliant
            assert!(
                encoder.is_lgpl_compliant(),
                "MPEG-4 should be LGPL-compliant"
            );

            // Encode a few frames to verify it works
            for _ in 0..10 {
                let frame = create_black_frame(640, 480);
                encoder.push_video(&frame).expect("Failed to push frame");
            }

            encoder.finish().expect("Failed to finish encoding");
            assert_valid_output_file(&output_path);
        }
        Err(e) => {
            panic!("MPEG-4 encoder should always be available, got: {}", e);
        }
    }
}
