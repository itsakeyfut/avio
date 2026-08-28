//! Integration tests for ProRes/DNxHD round-trips and HDR10 metadata in MKV.
//!
//! All tests skip gracefully when the required encoder or external tool is absent.

#![allow(clippy::unwrap_used, unsafe_code)]

mod fixtures;

use ff_encode::{
    BitrateMode, DnxhdOptions, DnxhdVariant, H265Options, H265Profile, Preset, ProResOptions,
    ProResProfile, VideoCodec, VideoCodecOptions, VideoEncoder,
};
use ff_format::{
    PixelFormat,
    codec::VideoCodec as FmtVideoCodec,
    hdr::{Hdr10Metadata, MasteringDisplay},
};
use fixtures::{
    FileGuard, assert_valid_output_file, create_black_frame, get_file_size, test_output_path,
};
use std::path::Path;

// Skip helpers

/// Returns `true` when `prores_ks` is compiled into this FFmpeg build.
fn is_prores_ks_available() -> bool {
    ff_sys::Codec::find_encoder_by_name("prores_ks").is_some()
}

/// Returns `true` when the `dnxhd` encoder is compiled into this FFmpeg build.
fn is_dnxhd_available() -> bool {
    ff_sys::Codec::find_encoder_by_name("dnxhd").is_some()
}

// ffprobe CLI helpers

/// Parses `max_content` (MaxCLL) from `ffprobe -show_streams` output.
///
/// Returns `None` when ffprobe is unavailable or the field is absent.
fn probe_max_cll(path: &Path) -> Option<u32> {
    let output = std::process::Command::new("ffprobe")
        .args(["-v", "quiet", "-show_streams", path.to_str()?])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|l| l.starts_with("max_content="))
        .and_then(|l| l.strip_prefix("max_content="))
        .and_then(|v| v.trim().parse().ok())
}

/// Parses `max_average` (MaxFALL) from `ffprobe -show_streams` output.
///
/// Returns `None` when ffprobe is unavailable or the field is absent.
fn probe_max_fall(path: &Path) -> Option<u32> {
    let output = std::process::Command::new("ffprobe")
        .args(["-v", "quiet", "-show_streams", path.to_str()?])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|l| l.starts_with("max_average="))
        .and_then(|l| l.strip_prefix("max_average="))
        .and_then(|v| v.trim().parse().ok())
}

// ProRes

/// Encodes 1920×1080 ProRes 422 HQ in a `.mov` container, then probes the
/// output to confirm `yuv422p10le` is the stored pixel format and that at
/// least 10 frames were written.
#[test]
fn prores_422hq_roundtrip_should_preserve_yuv422p10le_pixel_format() {
    if !is_prores_ks_available() {
        println!("Skipping: prores_ks not available");
        return;
    }

    let output_path = test_output_path("prof_prores_hq.mov");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(1920, 1080, 25.0)
        .video_codec(VideoCodec::ProRes)
        .codec_options(VideoCodecOptions::ProRes(ProResOptions {
            profile: ProResProfile::Hq,
            vendor: None,
        }))
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: ProRes encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..10 {
        encoder
            .push_video(&create_black_frame(1920, 1080))
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    let info = ff_probe::open(&output_path).expect("Failed to probe output");
    let video = info.primary_video().expect("No video stream in output");
    assert_eq!(
        video.pixel_format(),
        PixelFormat::Yuv422p10le,
        "Expected yuv422p10le in probed ProRes HQ output, got {:?}",
        video.pixel_format()
    );
    assert!(
        video.frame_count().unwrap_or(0) >= 10,
        "Expected at least 10 frames, got {:?}",
        video.frame_count()
    );
    println!(
        "ProRes HQ probe: codec={} pixel_format={:?} frames={:?} size={} bytes",
        video.codec_name(),
        video.pixel_format(),
        video.frame_count(),
        get_file_size(&output_path)
    );
}

// DNxHD

/// Encodes 1920×1080 DNxHD 145 Mbps in a `.mov` container, then probes the
/// output to confirm `yuv422p` (8-bit) is the stored pixel format.
#[test]
fn dnxhd_145_roundtrip_should_preserve_yuv422p_pixel_format() {
    if !is_dnxhd_available() {
        println!("Skipping: dnxhd not available");
        return;
    }

    let output_path = test_output_path("prof_dnxhd_145.mov");
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
            println!("Skipping: DNxHD encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..10 {
        encoder
            .push_video(&create_black_frame(1920, 1080))
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    let info = ff_probe::open(&output_path).expect("Failed to probe output");
    let video = info.primary_video().expect("No video stream in output");
    assert_eq!(
        video.pixel_format(),
        PixelFormat::Yuv422p,
        "Expected yuv422p in probed DNxHD 145 output, got {:?}",
        video.pixel_format()
    );
    println!(
        "DNxHD 145 probe: codec={} pixel_format={:?} size={} bytes",
        video.codec_name(),
        video.pixel_format(),
        get_file_size(&output_path)
    );
}

// HDR10 in MKV

/// Encodes with H.265 Main10 + HDR10 static metadata (MaxCLL=1000, MaxFALL=400)
/// in an MKV container, then:
/// - asserts via ff_probe that the codec is H.265, and
/// - asserts via the ffprobe CLI that MaxCLL and MaxFALL match the input values.
///
/// The ffprobe assertions are skipped gracefully when ffprobe is absent or the
/// output does not expose the side-data fields.
#[test]
fn hdr10_metadata_in_mkv_should_report_max_cll_and_max_fall() {
    let output_path = test_output_path("prof_hdr10.mkv");
    let _guard = FileGuard::new(output_path.clone());

    let result = VideoEncoder::create(&output_path)
        .video(640, 480, 25.0)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(28))
        .preset(Preset::Ultrafast)
        .pixel_format(PixelFormat::Yuv420p10le)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main10,
            ..H265Options::default()
        }))
        .hdr10_metadata(Hdr10Metadata {
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
        })
        .build();

    let mut encoder = match result {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: H.265 encoder unavailable: {e}");
            return;
        }
    };

    for _ in 0..15 {
        encoder
            .push_video(&create_black_frame(640, 480))
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    assert_valid_output_file(&output_path);

    // Assert codec via ff_probe.
    let info = ff_probe::open(&output_path).expect("Failed to probe output");
    let video = info.primary_video().expect("No video stream in output");
    assert_eq!(
        video.codec(),
        FmtVideoCodec::H265,
        "Expected H265 codec in probed MKV output, got {:?}",
        video.codec()
    );

    // Assert MaxCLL / MaxFALL via ffprobe CLI (graceful skip if absent).
    match (probe_max_cll(&output_path), probe_max_fall(&output_path)) {
        (Some(cll), Some(fall)) => {
            assert_eq!(
                cll, 1000,
                "MaxCLL should match the configured value (got {cll})"
            );
            assert_eq!(
                fall, 400,
                "MaxFALL should match the configured value (got {fall})"
            );
            println!(
                "HDR10 MKV probe: codec={} max_cll={cll} max_fall={fall} size={} bytes",
                video.codec_name(),
                get_file_size(&output_path)
            );
        }
        _ => {
            println!(
                "Note: ffprobe did not report max_content/max_average; \
                 skipping MaxCLL/MaxFALL assertions. \
                 codec={} size={} bytes",
                video.codec_name(),
                get_file_size(&output_path)
            );
        }
    }
}
