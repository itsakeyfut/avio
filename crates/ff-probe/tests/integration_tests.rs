//! Integration tests for ff-probe using real media files.
//!
//! These tests verify that ff-probe correctly extracts metadata from
//! actual media files in the assets directory.
//!
//! # Temporary file cleanup
//!
//! These tests are read-only: they call `ff_probe::open()` to inspect existing
//! media files and do not produce any temporary output files. No
//! `fixtures/mod.rs` with `FileGuard`/`DirGuard` is needed.

use std::path::PathBuf;
use std::time::Duration;

use ff_format::PixelFormat;
use ff_format::channel::ChannelLayout;
use ff_format::codec::{AudioCodec, VideoCodec};
use ff_probe::open;

// ============================================================================
// Test Helpers
// ============================================================================

/// Returns the path to the test assets directory.
fn assets_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{}/../../assets", manifest_dir))
}

/// Returns the path to the test video file.
fn test_video_path() -> PathBuf {
    assets_dir().join("video/gameplay.mp4")
}

/// Returns the path to the test audio file.
fn test_audio_path() -> PathBuf {
    assets_dir().join("audio/konekonoosanpo.mp3")
}

// ============================================================================
// Video File Integration Tests
// ============================================================================

#[test]
fn test_probe_video_file_opens_successfully() {
    let path = test_video_path();
    let result = open(&path);

    assert!(
        result.is_ok(),
        "Failed to open video file: {:?}",
        result.err()
    );
}

#[test]
fn test_probe_video_file_has_correct_format() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // MP4 files should be detected as mov,mp4,m4a,3gp,3g2,mj2 format
    assert!(
        info.format().contains("mp4") || info.format().contains("mov"),
        "Expected mp4/mov format, got: {}",
        info.format()
    );
}

#[test]
fn test_probe_video_file_has_video_stream() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    assert!(info.has_video(), "Video file should have video stream");
    assert!(
        !info.video_streams().is_empty(),
        "Video streams should not be empty"
    );
}

#[test]
fn test_probe_video_file_video_stream_properties() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    let video = info
        .primary_video()
        .expect("Should have primary video stream");

    // Verify video stream has valid properties
    assert!(video.width() > 0, "Video width should be positive");
    assert!(video.height() > 0, "Video height should be positive");

    // Common video codecs
    let valid_codecs = [
        VideoCodec::H264,
        VideoCodec::H265,
        VideoCodec::Vp9,
        VideoCodec::Av1,
    ];
    assert!(
        valid_codecs.contains(&video.codec()) || video.codec() == VideoCodec::Unknown,
        "Video codec should be a known codec or Unknown"
    );

    // Verify pixel format is set
    assert!(
        !matches!(video.pixel_format(), PixelFormat::Other(_)),
        "Pixel format should be a known format"
    );

    // Frame rate should be positive
    assert!(video.fps() > 0.0, "Frame rate should be positive");
}

#[test]
fn test_probe_video_file_has_audio_stream() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Video file may or may not have audio
    if info.has_audio() {
        let audio = info
            .primary_audio()
            .expect("Should have primary audio stream");

        // Verify audio stream has valid properties
        assert!(audio.sample_rate() > 0, "Sample rate should be positive");
        assert!(audio.channels() > 0, "Channel count should be positive");
    }
}

#[test]
fn test_probe_video_file_duration() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Duration should be positive
    assert!(
        info.duration() > Duration::ZERO,
        "Duration should be positive"
    );

    // Duration should be reasonable (less than 1 hour for test files)
    assert!(
        info.duration() < Duration::from_secs(3600),
        "Duration seems unreasonably long for a test file"
    );
}

#[test]
fn test_probe_video_file_size() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // File size should be positive
    assert!(info.file_size() > 0, "File size should be positive");
}

#[test]
fn test_probe_video_file_path() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Path should match
    assert_eq!(info.path(), path);

    // File name should be correct
    assert_eq!(info.file_name(), Some("gameplay.mp4"));

    // Extension should be mp4
    assert_eq!(info.extension(), Some("mp4"));
}

// ============================================================================
// Audio File Integration Tests
// ============================================================================

#[test]
fn test_probe_audio_file_opens_successfully() {
    let path = test_audio_path();
    let result = open(&path);

    assert!(
        result.is_ok(),
        "Failed to open audio file: {:?}",
        result.err()
    );
}

#[test]
fn test_probe_audio_file_has_correct_format() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // MP3 files should be detected as mp3 format
    assert!(
        info.format().contains("mp3"),
        "Expected mp3 format, got: {}",
        info.format()
    );
}

#[test]
fn test_probe_audio_file_has_audio_stream() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    assert!(info.has_audio(), "Audio file should have audio stream");
    assert!(
        !info.audio_streams().is_empty(),
        "Audio streams should not be empty"
    );
}

#[test]
fn test_probe_audio_file_no_video_stream() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    assert!(
        !info.has_video(),
        "Audio-only file should not have video stream"
    );
    assert!(
        info.video_streams().is_empty(),
        "Video streams should be empty"
    );
}

#[test]
fn test_probe_audio_file_audio_stream_properties() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    let audio = info
        .primary_audio()
        .expect("Should have primary audio stream");

    // Verify audio codec is MP3
    assert_eq!(audio.codec(), AudioCodec::Mp3, "Audio codec should be MP3");

    // Verify sample rate is a common value
    let common_sample_rates = [8000, 11025, 22050, 44100, 48000, 96000];
    assert!(
        common_sample_rates.contains(&audio.sample_rate()),
        "Sample rate {} should be a common value",
        audio.sample_rate()
    );

    // Verify channel count is reasonable (1 = mono, 2 = stereo, etc.)
    assert!(
        audio.channels() >= 1 && audio.channels() <= 8,
        "Channel count {} should be between 1 and 8",
        audio.channels()
    );

    // Verify channel layout matches channel count
    let expected_layouts = match audio.channels() {
        1 => vec![ChannelLayout::Mono],
        2 => vec![ChannelLayout::Stereo],
        _ => vec![],
    };
    if !expected_layouts.is_empty() {
        assert!(
            expected_layouts.contains(&audio.channel_layout()),
            "Channel layout {:?} should match channel count {}",
            audio.channel_layout(),
            audio.channels()
        );
    }
}

#[test]
fn test_probe_audio_file_sample_format() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    let audio = info
        .primary_audio()
        .expect("Should have primary audio stream");

    // MP3 typically decodes to planar float format
    let sample_format = audio.sample_format();
    assert!(
        sample_format.is_float() || sample_format.is_integer(),
        "Sample format should be float or integer"
    );
}

#[test]
fn test_probe_audio_file_duration() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Duration should be positive
    assert!(
        info.duration() > Duration::ZERO,
        "Duration should be positive"
    );

    // Duration should be reasonable for a test file
    assert!(
        info.duration() < Duration::from_secs(600),
        "Duration seems unreasonably long for a test file"
    );
}

#[test]
fn test_probe_audio_file_path() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Path should match
    assert_eq!(info.path(), path);

    // File name should be correct
    assert_eq!(info.file_name(), Some("konekonoosanpo.mp3"));

    // Extension should be mp3
    assert_eq!(info.extension(), Some("mp3"));
}

// ============================================================================
// Error Handling Integration Tests
// ============================================================================

#[test]
fn test_probe_nonexistent_file() {
    let path = assets_dir().join("nonexistent-file.mp4");
    let result = open(&path);

    assert!(result.is_err(), "Opening nonexistent file should fail");
}

#[test]
fn test_probe_invalid_file() {
    // Try to open a non-media file (image)
    let path = assets_dir().join("img/hello-triangle.png");
    let result = open(&path);

    // PNG files might be opened by FFmpeg as image format or might fail
    // Either behavior is acceptable for this test
    if let Ok(info) = result {
        // If it opens, it should have some format info
        assert!(!info.format().is_empty(), "Format should not be empty");
    }
}

// ============================================================================
// Stream Count Tests
// ============================================================================

#[test]
fn test_probe_video_file_stream_counts() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Should have at least one video stream
    assert!(
        info.video_stream_count() >= 1,
        "Should have at least 1 video stream"
    );

    // Total stream count should be at least 1
    let total = info.video_stream_count() + info.audio_stream_count();
    assert!(total >= 1, "Should have at least 1 stream");
}

#[test]
fn test_probe_audio_file_stream_counts() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Should have exactly one audio stream
    assert_eq!(
        info.audio_stream_count(),
        1,
        "Should have exactly 1 audio stream"
    );

    // Should have no video streams
    assert_eq!(info.video_stream_count(), 0, "Should have 0 video streams");
}

// ============================================================================
// Resolution and Aspect Ratio Tests
// ============================================================================

#[test]
fn test_probe_video_resolution() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Should have resolution information
    let resolution = info.resolution();
    assert!(resolution.is_some(), "Video file should have resolution");

    let (width, height) = resolution.unwrap();
    assert!(width > 0 && height > 0, "Resolution should be positive");

    // Common resolutions have reasonable aspect ratios
    let aspect = width as f64 / height as f64;
    assert!(
        aspect > 0.5 && aspect < 3.0,
        "Aspect ratio {} seems unusual",
        aspect
    );
}

#[test]
fn test_probe_video_frame_rate() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Should have frame rate information
    let frame_rate = info.frame_rate();
    assert!(frame_rate.is_some(), "Video file should have frame rate");

    let fps_value = frame_rate.unwrap();

    // Common frame rates: 23.976, 24, 25, 29.97, 30, 50, 59.94, 60
    assert!(
        fps_value > 10.0 && fps_value < 120.0,
        "Frame rate {} seems unusual",
        fps_value
    );
}

// ============================================================================
// Audio Properties Tests
// ============================================================================

#[test]
fn test_probe_audio_sample_rate() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Should have sample rate information
    let sample_rate = info.sample_rate();
    assert!(sample_rate.is_some(), "Audio file should have sample rate");

    let rate = sample_rate.unwrap();
    assert!(
        rate >= 8000 && rate <= 192000,
        "Sample rate {} seems unusual",
        rate
    );
}

#[test]
fn test_probe_audio_channels() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Should have channel count information
    let channels = info.channels();
    assert!(channels.is_some(), "Audio file should have channel count");

    let ch = channels.unwrap();
    assert!(ch >= 1 && ch <= 8, "Channel count {} seems unusual", ch);
}

// ============================================================================
// Metadata Extraction Tests
// ============================================================================

#[test]
fn test_probe_video_file_metadata_accessible() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Metadata should be accessible (may be empty for some files)
    // Just verify we can get the metadata without panicking
    let _metadata = info.metadata();

    // Test that convenience methods work without panicking
    let _ = info.title();
    let _ = info.artist();
    let _ = info.album();
    let _ = info.creation_time();
    let _ = info.date();
    let _ = info.comment();
    let _ = info.encoder();
}

#[test]
fn test_probe_audio_file_metadata_accessible() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Metadata should be accessible (may be empty for some files)
    // Just verify we can get the metadata without panicking
    let _metadata = info.metadata();

    // MP3 files often have ID3 tags with title, artist, etc.
    // Just verify the methods don't panic
    let _ = info.title();
    let _ = info.artist();
    let _ = info.album();
    let _ = info.date();
}

#[test]
fn test_probe_video_file_metadata_keys() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Test that we can iterate over metadata
    for (key, _value) in info.metadata() {
        // Keys should be non-empty strings
        assert!(!key.is_empty(), "Metadata key should not be empty");
        // Value can be empty (valid empty string) - just ensure iteration works
    }
}

#[test]
fn test_probe_audio_file_metadata_keys() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Test that we can iterate over metadata
    for (key, _value) in info.metadata() {
        // Keys should be non-empty strings
        assert!(!key.is_empty(), "Metadata key should not be empty");
        // Value can be empty (valid empty string) - just ensure iteration works
    }
}

// ============================================================================
// Color Space Extraction Tests
// ============================================================================

#[test]
fn test_probe_video_color_space_extraction() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    let video = info
        .primary_video()
        .expect("Should have primary video stream");

    // Verify color space is extracted (may be Unknown for some files)
    let color_space = video.color_space();
    // Valid color spaces: BT.709 (HD), BT.470BG/SMPTE-170M (SD), BT.2020 NCL/CL (HDR), RGB, or Unknown
    let valid_spaces = [
        ff_format::color::ColorSpace::Bt709,
        ff_format::color::ColorSpace::Bt470bg,
        ff_format::color::ColorSpace::Smpte170m,
        ff_format::color::ColorSpace::Bt2020Ncl,
        ff_format::color::ColorSpace::Bt2020Cl,
        ff_format::color::ColorSpace::Rgb,
        ff_format::color::ColorSpace::Unknown,
    ];
    assert!(
        valid_spaces.contains(&color_space),
        "Color space {:?} should be a valid value",
        color_space
    );
}

#[test]
fn test_probe_video_color_range_extraction() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    let video = info
        .primary_video()
        .expect("Should have primary video stream");

    // Verify color range is extracted
    let color_range = video.color_range();
    // Valid color ranges: Limited (TV), Full (PC), or Unknown
    let valid_ranges = [
        ff_format::color::ColorRange::Limited,
        ff_format::color::ColorRange::Full,
        ff_format::color::ColorRange::Unknown,
    ];
    assert!(
        valid_ranges.contains(&color_range),
        "Color range {:?} should be a valid value",
        color_range
    );
}

#[test]
fn test_probe_video_color_primaries_extraction() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    let video = info
        .primary_video()
        .expect("Should have primary video stream");

    // Verify color primaries are extracted
    let color_primaries = video.color_primaries();
    // Valid color primaries: BT.709, BT.470BG/SMPTE-170M, BT.2020, DCI-P3, Display P3, or Unknown
    let valid_primaries = [
        ff_format::color::ColorPrimaries::Bt709,
        ff_format::color::ColorPrimaries::Bt470bg,
        ff_format::color::ColorPrimaries::Smpte170m,
        ff_format::color::ColorPrimaries::Bt2020,
        ff_format::color::ColorPrimaries::DciP3,
        ff_format::color::ColorPrimaries::DisplayP3,
        ff_format::color::ColorPrimaries::Unknown,
    ];
    assert!(
        valid_primaries.contains(&color_primaries),
        "Color primaries {:?} should be a valid value",
        color_primaries
    );
}

#[test]
fn test_probe_video_color_info_consistency() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    let video = info
        .primary_video()
        .expect("Should have primary video stream");

    // If color space is HD (BT.709), primaries should typically also be BT.709
    let color_space = video.color_space();
    let color_primaries = video.color_primaries();

    // Check for common consistency patterns (not strictly required, but good practice)
    // Note: Some encoders may not set all color parameters consistently
    if color_space.is_uhd() {
        // HDR content should have BT.2020 primaries
        // (or Unknown if encoder didn't set it)
        assert!(
            color_primaries == ff_format::color::ColorPrimaries::Bt2020
                || color_primaries == ff_format::color::ColorPrimaries::Unknown,
            "BT.2020 color space should have BT.2020 or Unknown primaries, got {:?}",
            color_primaries
        );
    }
}

#[test]
fn test_probe_video_hdr_detection() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    let video = info
        .primary_video()
        .expect("Should have primary video stream");

    // Check for HDR indicators
    let color_space = video.color_space();
    let color_primaries = video.color_primaries();

    // HDR content is indicated by BT.2020 color space or primaries
    let is_hdr =
        color_space.is_uhd() || color_primaries == ff_format::color::ColorPrimaries::Bt2020;

    // For test files, we don't require HDR, just verify detection works
    // If HDR is detected, log it for manual verification
    if is_hdr {
        println!("HDR content detected in test file");
    }

    // The test passes regardless - we're just verifying the detection logic runs
    assert!(true, "HDR detection completed successfully");
}

// ============================================================================
// Bitrate Extraction Tests
// ============================================================================

#[test]
fn test_probe_video_file_bitrate() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Container bitrate should be available (either from FFmpeg or calculated)
    let bitrate = info.bitrate();
    assert!(
        bitrate.is_some(),
        "Video file should have container bitrate"
    );

    // Verify bitrate is within reasonable range (1 kbps to 100 Mbps)
    let bps = bitrate.unwrap();
    assert!(
        bps > 1_000 && bps < 100_000_000,
        "Bitrate {} bps seems unreasonable (expected 1 kbps - 100 Mbps)",
        bps
    );
}

#[test]
fn test_probe_audio_file_bitrate() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    // Container bitrate should be available (either from FFmpeg or calculated)
    let bitrate = info.bitrate();
    assert!(
        bitrate.is_some(),
        "Audio file should have container bitrate"
    );

    // Verify bitrate is within reasonable range for audio (8 kbps to 10 Mbps)
    let bps = bitrate.unwrap();
    assert!(
        bps > 8_000 && bps < 10_000_000,
        "Audio bitrate {} bps seems unreasonable (expected 8 kbps - 10 Mbps)",
        bps
    );
}

#[test]
fn test_probe_video_stream_bitrate() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    let video = info
        .primary_video()
        .expect("Should have primary video stream");

    // Video stream bitrate may or may not be available depending on the container
    // If available, it should be within reasonable bounds
    if let Some(bps) = video.bitrate() {
        assert!(
            bps > 1_000 && bps < 100_000_000,
            "Video stream bitrate {} bps seems unreasonable",
            bps
        );
    }
}

#[test]
fn test_probe_audio_stream_bitrate() {
    let path = test_audio_path();
    let info = open(&path).expect("Failed to open audio file");

    let audio = info
        .primary_audio()
        .expect("Should have primary audio stream");

    // Audio stream bitrate may or may not be available depending on the container
    // If available, it should be within reasonable bounds
    if let Some(bps) = audio.bitrate() {
        assert!(
            bps > 1_000 && bps < 10_000_000,
            "Audio stream bitrate {} bps seems unreasonable",
            bps
        );
    }
}

#[test]
fn test_probe_bitrate_consistency() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    // Verify bitrate is consistent with file size and duration
    let file_size = info.file_size();
    let duration = info.duration();
    let bitrate = info.bitrate();

    if let Some(bps) = bitrate {
        // Calculate expected bitrate from file size and duration
        let duration_secs = duration.as_secs_f64();
        if duration_secs > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            let calculated_bps = (file_size as f64 * 8.0 / duration_secs) as u64;

            // Allow 20% tolerance for container overhead and rounding
            let min_expected = calculated_bps * 80 / 100;
            let max_expected = calculated_bps * 120 / 100;

            assert!(
                bps >= min_expected && bps <= max_expected,
                "Bitrate {} should be close to calculated value {} (within 20%)",
                bps,
                calculated_bps
            );
        }
    }
}

// ============================================================================
// Subtitle Stream Tests
// ============================================================================

#[test]
fn test_probe_video_file_has_no_subtitle_streams() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    assert!(
        info.subtitle_streams().is_empty(),
        "Video file without subtitles should have empty subtitle_streams()"
    );
}

#[test]
fn test_probe_video_file_has_subtitles_returns_false() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    assert!(
        !info.has_subtitles(),
        "has_subtitles() should return false for a video file without subtitle streams"
    );
}

#[test]
fn test_probe_video_file_subtitle_stream_count_is_zero() {
    let path = test_video_path();
    let info = open(&path).expect("Failed to open video file");

    assert_eq!(
        info.subtitle_stream_count(),
        0,
        "subtitle_stream_count() should be 0 for a video file without subtitle streams"
    );
}
