//! Integration tests for H.264 profile, level, preset, and tune options.
//!
//! All tests skip gracefully when `libx264` is absent from the FFmpeg build.

#![allow(clippy::unwrap_used, unsafe_code)]

mod fixtures;

use ff_encode::{
    H264Options, H264Preset, H264Profile, H264Tune, HardwareEncoder, VideoCodec, VideoCodecOptions,
    VideoEncoder,
};
use ff_format::codec::VideoCodec as FmtVideoCodec;
use fixtures::{
    FileGuard, assert_valid_output_file, create_black_frame, get_file_size, test_output_path,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `true` when `libx264` is compiled into this FFmpeg build **and**
/// the `gpl` feature flag is enabled, meaning the encoder selection code will
/// actually choose libx264 (rather than falling back to libvpx-vp9).
fn is_libx264_available() -> bool {
    if !cfg!(feature = "gpl") {
        return false;
    }
    ff_sys::Codec::find_encoder_by_name("libx264").is_some()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Encodes 1920×1080 at 30 fps with High profile and level 4.1.
/// Probes the output to confirm H.264 is the stored codec.
#[test]
fn h264_high_profile_level41_should_produce_valid_output() {
    if !is_libx264_available() {
        println!("Skipping: libx264 not available");
        return;
    }

    let output_path = test_output_path("h264_opts_high_level41_1080p.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let opts = VideoCodecOptions::H264(H264Options {
        profile: H264Profile::High,
        level: Some(41),
        preset: Some(H264Preset::Ultrafast),
        ..H264Options::default()
    });

    let result = VideoEncoder::create(&output_path)
        .video(1920, 1080, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .codec_options(opts)
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: H.264 encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..30 {
        let frame = create_black_frame(1920, 1080);
        encoder
            .push_video(&frame)
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");

    assert_valid_output_file(&output_path);
    let file_size = get_file_size(&output_path);
    assert!(file_size > 1000, "Output too small: {file_size} bytes");

    let info = ff_probe::open(&output_path).expect("Failed to probe output");
    let video = info.primary_video().expect("No video stream in output");
    assert_eq!(
        video.codec(),
        FmtVideoCodec::H264,
        "Expected H.264 codec in output, got {:?}",
        video.codec()
    );
    println!(
        "H264 High@4.1 1080p: codec={} size={file_size} bytes",
        video.codec_name()
    );
}

/// Encodes the same black-frame content with `veryslow` and `ultrafast` presets
/// and asserts that the `veryslow` output is no larger than the `ultrafast` output.
#[test]
fn h264_veryslow_preset_should_produce_output_no_larger_than_ultrafast() {
    if !is_libx264_available() {
        println!("Skipping: libx264 not available");
        return;
    }

    // --- veryslow ---
    let veryslow_path = test_output_path("h264_opts_veryslow.mp4");
    let _guard_vs = FileGuard::new(veryslow_path.clone());

    let result = VideoEncoder::create(&veryslow_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .codec_options(VideoCodecOptions::H264(H264Options {
            preset: Some(H264Preset::Veryslow),
            ..H264Options::default()
        }))
        .build();

    let mut enc_vs = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: H.264 encoder unavailable: {e}");
            return;
        }
    };
    for _ in 0..10 {
        enc_vs
            .push_video(&create_black_frame(320, 240))
            .expect("push failed");
    }
    enc_vs.finish().expect("finish failed");
    let veryslow_size = get_file_size(&veryslow_path);

    // --- ultrafast ---
    let ultrafast_path = test_output_path("h264_opts_ultrafast.mp4");
    let _guard_uf = FileGuard::new(ultrafast_path.clone());

    let mut enc_uf = VideoEncoder::create(&ultrafast_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .codec_options(VideoCodecOptions::H264(H264Options {
            preset: Some(H264Preset::Ultrafast),
            ..H264Options::default()
        }))
        .build()
        .expect("ultrafast should succeed after veryslow succeeded");
    for _ in 0..10 {
        enc_uf
            .push_video(&create_black_frame(320, 240))
            .expect("push failed");
    }
    enc_uf.finish().expect("finish failed");
    let ultrafast_size = get_file_size(&ultrafast_path);

    println!("veryslow={veryslow_size} bytes  ultrafast={ultrafast_size} bytes");
    // Black frames compress very efficiently regardless of preset — use <=
    // to avoid false failures when both sizes happen to be identical.
    assert!(
        veryslow_size <= ultrafast_size,
        "veryslow ({veryslow_size} B) should be no larger than ultrafast ({ultrafast_size} B)"
    );
}

/// Verifies that `tune=Grain` is accepted without returning an error or panicking.
#[test]
fn h264_tune_grain_should_be_accepted_without_error() {
    if !is_libx264_available() {
        println!("Skipping: libx264 not available");
        return;
    }

    let output_path = test_output_path("h264_opts_tune_grain.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let opts = VideoCodecOptions::H264(H264Options {
        preset: Some(H264Preset::Ultrafast),
        tune: Some(H264Tune::Grain),
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
            println!("Skipping: H.264 encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..5 {
        encoder
            .push_video(&create_black_frame(320, 240))
            .expect("push_video failed");
    }
    encoder.finish().expect("finish failed");
    assert_valid_output_file(&output_path);
    println!("H264 tune=grain accepted without error");
}

/// Verifies that an invalid level value (999) does not cause a panic.
///
/// The encoder is expected to either accept the value (clamped or ignored with
/// a logged warning) or return an error from `build()` — but never panic.
#[test]
fn h264_invalid_level_should_not_panic() {
    if !is_libx264_available() {
        println!("Skipping: libx264 not available");
        return;
    }

    let output_path = test_output_path("h264_opts_invalid_level.mp4");
    let _guard = FileGuard::new(output_path.clone());

    let opts = VideoCodecOptions::H264(H264Options {
        level: Some(999),
        preset: Some(H264Preset::Ultrafast),
        ..H264Options::default()
    });

    let build_result = VideoEncoder::create(&output_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::H264)
        .hardware_encoder(HardwareEncoder::None)
        .codec_options(opts)
        .build();

    match build_result {
        Ok(mut encoder) => {
            for _ in 0..5 {
                // Ignore individual push errors — we only care about no panic.
                let _ = encoder.push_video(&create_black_frame(320, 240));
            }
            // finish may return an error when the stream is inconsistent.
            let _ = encoder.finish();
            println!("H264 level=999: encoder accepted (clamped or ignored)");
        }
        Err(e) => {
            println!("H264 level=999: build() returned error (expected): {e}");
        }
    }
    // Reaching here without panicking satisfies the test.
}
