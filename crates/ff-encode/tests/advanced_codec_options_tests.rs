//! Integration tests for H.265, AV1, and VP9 codec options.
//!
//! All tests skip gracefully when the required encoder is absent from the
//! FFmpeg build. The AV1 timing test is marked `#[ignore]` because timing
//! assertions are environment-dependent.

#![allow(clippy::unwrap_used, unsafe_code)]

mod fixtures;

use ff_encode::{
    Av1Options, BitrateMode, EncodeError, H265Options, H265Profile, H265Tier, Preset, VideoCodec,
    VideoCodecOptions, VideoEncoder, Vp9Options,
};
use ff_format::{PixelFormat, codec::VideoCodec as FmtVideoCodec};
use fixtures::{
    FileGuard, assert_valid_output_file, create_black_frame, get_file_size, test_output_path,
};
use std::time::Instant;

// Skip helpers

/// Returns `true` when `libx265` is compiled into this FFmpeg build.
fn is_libx265_available() -> bool {
    ff_sys::Codec::find_encoder_by_name("libx265").is_some()
}

/// Returns `true` when `libaom-av1` is compiled into this FFmpeg build.
fn is_libaom_av1_available() -> bool {
    ff_sys::Codec::find_encoder_by_name("libaom-av1").is_some()
}

/// Returns `true` when `libvpx-vp9` is compiled into this FFmpeg build.
fn is_libvpx_vp9_available() -> bool {
    ff_sys::Codec::find_encoder_by_name("libvpx-vp9").is_some()
}

// H.265

/// Encodes with H.265 Main10 profile and explicit `yuv420p10le` pixel format,
/// then probes the output to confirm the stored pixel format is `yuv420p10le`.
#[test]
fn h265_main10_with_yuv420p10le_should_report_yuv420p10le_via_probe() {
    if !is_libx265_available() {
        println!("Skipping: libx265 not available");
        return;
    }

    let output_path = test_output_path("adv_h265_main10_probe.mp4");
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
            ..H265Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: H.265 encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..10 {
        encoder
            .push_video(&create_black_frame(640, 480))
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    let info = ff_probe::open(&output_path).expect("Failed to probe output");
    let video = info.primary_video().expect("No video stream in output");
    assert_eq!(
        video.pixel_format(),
        PixelFormat::Yuv420p10le,
        "Expected yuv420p10le pixel format in probed output, got {:?}",
        video.pixel_format()
    );
    println!(
        "H265 Main10 probe: codec={} pixel_format={:?} size={} bytes",
        video.codec_name(),
        video.pixel_format(),
        get_file_size(&output_path)
    );
}

// AV1

/// Encodes the same content with `cpu_used=8` and `cpu_used=4` and asserts
/// that the faster setting finishes in no more time than the slower one.
///
/// Marked `#[ignore]` because timing thresholds are environment-dependent —
/// run explicitly with `cargo test -- --include-ignored`.
#[test]
#[ignore = "performance thresholds are environment-dependent; run explicitly with -- --include-ignored"]
fn av1_cpu_used_8_should_encode_faster_than_cpu_used_4() {
    if !is_libaom_av1_available() {
        println!("Skipping: libaom-av1 not available");
        return;
    }

    // --- cpu_used=4 (slower, higher quality) ---
    let path4 = test_output_path("adv_av1_cpu4_timing.mp4");
    let _guard4 = FileGuard::new(path4.clone());

    let mut enc4 = match VideoEncoder::create(&path4)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Av1)
        .codec_options(VideoCodecOptions::Av1(Av1Options {
            cpu_used: 4,
            ..Av1Options::default()
        }))
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: AV1 encoder unavailable: {e}");
            return;
        }
    };
    let start4 = Instant::now();
    for _ in 0..10 {
        enc4.push_video(&create_black_frame(320, 240))
            .expect("push failed");
    }
    enc4.finish().expect("finish failed");
    let elapsed4 = start4.elapsed();

    // --- cpu_used=8 (faster, lower quality) ---
    let path8 = test_output_path("adv_av1_cpu8_timing.mp4");
    let _guard8 = FileGuard::new(path8.clone());

    let mut enc8 = VideoEncoder::create(&path8)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Av1)
        .codec_options(VideoCodecOptions::Av1(Av1Options {
            cpu_used: 8,
            ..Av1Options::default()
        }))
        .build()
        .expect("cpu_used=8 should succeed after cpu_used=4 succeeded");
    let start8 = Instant::now();
    for _ in 0..10 {
        enc8.push_video(&create_black_frame(320, 240))
            .expect("push failed");
    }
    enc8.finish().expect("finish failed");
    let elapsed8 = start8.elapsed();

    println!("AV1 timing: cpu_used=4 took {elapsed4:?}  cpu_used=8 took {elapsed8:?}");
    assert!(
        elapsed8 <= elapsed4,
        "cpu_used=8 ({elapsed8:?}) should be no slower than cpu_used=4 ({elapsed4:?})"
    );
}

/// Verifies that `cpu_used=9` is rejected with `EncodeError::InvalidOption`.
///
/// Validation happens in `build()` before any codec lookup, so this test
/// does not require `libaom-av1` to be present.
#[test]
fn av1_cpu_used_9_should_be_rejected_with_invalid_option_error() {
    let output_path = test_output_path("adv_av1_cpu9.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Av1)
        .codec_options(VideoCodecOptions::Av1(Av1Options {
            cpu_used: 9,
            ..Av1Options::default()
        }))
        .build();

    assert!(
        matches!(result, Err(EncodeError::InvalidOption { .. })),
        "Expected InvalidOption error for cpu_used=9, got an Ok or different Err"
    );
}

// VP9

/// Encodes with VP9 in constrained-quality (CQ) mode at `cq_level=33`,
/// then probes the output to confirm VP9 is the stored codec.
#[test]
fn vp9_cq_level_33_should_produce_valid_output_with_vp9_codec_in_probe() {
    if !is_libvpx_vp9_available() {
        println!("Skipping: libvpx-vp9 not available");
        return;
    }

    let output_path = test_output_path("adv_vp9_cq33_probe.webm");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 30.0)
        .video_codec(VideoCodec::Vp9)
        .codec_options(VideoCodecOptions::Vp9(Vp9Options {
            cpu_used: 4,
            cq_level: Some(33),
            ..Vp9Options::default()
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: VP9 encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..10 {
        encoder
            .push_video(&create_black_frame(640, 480))
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    let info = ff_probe::open(&output_path).expect("Failed to probe output");
    let video = info.primary_video().expect("No video stream in output");
    assert_eq!(
        video.codec(),
        FmtVideoCodec::Vp9,
        "Expected VP9 codec in probed output, got {:?}",
        video.codec()
    );
    println!(
        "VP9 CQ33 probe: codec={} size={} bytes",
        video.codec_name(),
        get_file_size(&output_path)
    );
}
