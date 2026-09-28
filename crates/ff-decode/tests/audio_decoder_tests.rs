//! Audio decoder tests covering AudioDecoder creation, stream info,
//! frame decoding, sample format conversion, sample rate conversion,
//! channel conversion, seeking, and iterator patterns.

use std::path::PathBuf;
use std::time::Duration;

mod fixtures;
use fixtures::*;

use ff_decode::{AudioDecoder, SeekMode};
use ff_format::SampleFormat;

// ============================================================================
// Basic Audio Decoder Creation Tests
// ============================================================================

#[test]
fn test_audio_decoder_opens_successfully() {
    let result = create_audio_decoder();
    assert!(
        result.is_ok(),
        "Failed to open audio file: {:?}",
        result.err()
    );
}

#[test]
fn test_audio_decoder_nonexistent_file() {
    let path = assets_dir().join("nonexistent-audio.mp3");
    let result = AudioDecoder::open(&path).build();

    assert!(result.is_err(), "Opening nonexistent file should fail");
}

// ============================================================================
// Audio Stream Information Tests
// ============================================================================

#[test]
fn test_audio_decoder_stream_info() {
    let decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let info = decoder.stream_info();

    // Verify basic properties
    assert!(info.channels() > 0, "Channel count should be positive");
    assert!(info.sample_rate() > 0, "Sample rate should be positive");
}

#[test]
fn test_audio_decoder_stream_info_sample_format() {
    let decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let info = decoder.stream_info();

    // Sample format should be a known format
    let format = info.sample_format();
    assert!(
        !matches!(format, SampleFormat::Other(_)),
        "Sample format should be a known format, got {:?}",
        format
    );
}

#[test]
fn test_audio_decoder_stream_info_codec() {
    let decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let info = decoder.stream_info();

    // Codec should be set
    let codec = info.codec();
    assert!(
        codec != ff_format::codec::AudioCodec::Unknown,
        "Audio codec should be known"
    );
}

#[test]
fn test_audio_decoder_stream_info_duration() {
    let decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let info = decoder.stream_info();

    // Duration should be present and reasonable
    if let Some(duration) = info.duration() {
        assert!(duration > Duration::ZERO, "Duration should be positive");
        assert!(
            duration < Duration::from_secs(3600),
            "Duration seems unreasonably long for a test file"
        );
    }
}

// ============================================================================
// Audio Frame Decoding Tests
// ============================================================================

#[test]
fn test_decode_first_audio_frame() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Decode first frame
    let result = decoder.decode_one();
    assert!(
        result.is_ok(),
        "Failed to decode first audio frame: {:?}",
        result.err()
    );

    let frame_opt = result.unwrap();
    assert!(frame_opt.is_some(), "First audio frame should be Some");

    let frame = frame_opt.unwrap();

    // Verify frame properties
    let info = decoder.stream_info();
    assert_eq!(
        frame.channels(),
        info.channels(),
        "Frame channels should match stream info"
    );
    assert_eq!(
        frame.sample_rate(),
        info.sample_rate(),
        "Frame sample rate should match stream info"
    );
}

#[test]
fn test_decode_multiple_audio_frames() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Decode first 10 frames
    let mut frame_count = 0;
    for i in 0..10 {
        let result = decoder.decode_one();
        assert!(
            result.is_ok(),
            "Failed to decode audio frame {}: {:?}",
            i,
            result.err()
        );

        if let Some(frame) = result.unwrap() {
            frame_count += 1;

            // Verify frame is valid
            assert!(
                frame.samples() > 0,
                "Frame {} sample count should be positive",
                frame_count
            );
            assert!(
                frame.channels() > 0,
                "Frame {} channel count should be positive",
                frame_count
            );
            assert!(
                !frame.planes().is_empty(),
                "Frame {} should have planes",
                frame_count
            );
        } else {
            break;
        }
    }

    assert!(frame_count > 0, "Should decode at least one audio frame");
    assert_eq!(frame_count, 10, "Should decode 10 audio frames");
}

#[test]
fn test_decode_audio_frames_have_data() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Decode first frame
    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    // Verify planes have data
    let planes = frame.planes();
    assert!(
        !planes.is_empty(),
        "Audio frame should have at least one plane"
    );

    for (i, plane) in planes.iter().enumerate() {
        assert!(!plane.is_empty(), "Audio plane {} should not be empty", i);
    }
}

#[test]
fn test_decode_audio_frame_timestamps() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    let mut last_pts = None;

    // Decode first few frames and verify timestamps are increasing
    for i in 0..5 {
        let frame = decoder
            .decode_one()
            .expect("Failed to decode")
            .unwrap_or_else(|| panic!("Audio frame {} should exist", i));

        let timestamp = frame.timestamp();
        let pts = timestamp.pts();

        if let Some(last) = last_pts {
            assert!(
                pts >= last,
                "Timestamp should not decrease: frame {} pts={}, last_pts={}",
                i,
                pts,
                last
            );
        }

        last_pts = Some(pts);
    }
}

// ============================================================================
// Sample Format Conversion Tests
// ============================================================================

#[test]
fn test_decode_with_f32_output() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_format(SampleFormat::F32)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(
        frame.format(),
        SampleFormat::F32,
        "Output format should be F32"
    );
}

#[test]
fn test_decode_with_i16_output() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_format(SampleFormat::I16)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(
        frame.format(),
        SampleFormat::I16,
        "Output format should be I16"
    );
}

#[test]
fn test_decode_with_f32p_output() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_format(SampleFormat::F32p)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(
        frame.format(),
        SampleFormat::F32p,
        "Output format should be F32P (planar)"
    );

    // F32P is planar, should have one plane per channel
    assert_eq!(
        frame.planes().len(),
        frame.channels() as usize,
        "F32P should have one plane per channel"
    );
}

// ============================================================================
// Sample Rate Conversion Tests
// ============================================================================

#[test]
fn test_decode_with_48000hz_sample_rate() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_sample_rate(48000)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(
        frame.sample_rate(),
        48000,
        "Output sample rate should be 48000 Hz"
    );
}

#[test]
fn test_decode_with_44100hz_sample_rate() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_sample_rate(44100)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(
        frame.sample_rate(),
        44100,
        "Output sample rate should be 44100 Hz"
    );
}

#[test]
fn test_decode_with_16000hz_sample_rate() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_sample_rate(16000)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(
        frame.sample_rate(),
        16000,
        "Output sample rate should be 16000 Hz"
    );
}

// ============================================================================
// Channel Conversion Tests
// ============================================================================

#[test]
fn test_decode_with_mono_output() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_channels(1)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(frame.channels(), 1, "Output should be mono (1 channel)");
}

#[test]
fn test_decode_with_stereo_output() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_channels(2)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(frame.channels(), 2, "Output should be stereo (2 channels)");
}

// ============================================================================
// Combined Conversion Tests
// ============================================================================

#[test]
fn test_decode_with_format_rate_channel_conversion() {
    let path = test_audio_path();
    let mut decoder = AudioDecoder::open(&path)
        .output_format(SampleFormat::F32)
        .output_sample_rate(48000)
        .output_channels(1)
        .build()
        .expect("Failed to create audio decoder");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    assert_eq!(frame.format(), SampleFormat::F32, "Should be F32");
    assert_eq!(frame.sample_rate(), 48000, "Should be 48000 Hz");
    assert_eq!(frame.channels(), 1, "Should be mono");
}

// ============================================================================
// Audio Decoder State Tests
// ============================================================================

#[test]
fn test_audio_decoder_is_eof() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Initially should not be EOF
    assert!(
        !decoder.is_eof(),
        "Audio decoder should not be EOF initially"
    );

    // Decode all frames until EOF
    let mut frame_count = 0;
    loop {
        match decoder.decode_one() {
            Ok(Some(_)) => {
                frame_count += 1;
                // Limit to prevent infinite loop
                if frame_count > 100000 {
                    panic!("Too many audio frames, possible infinite loop");
                }
            }
            Ok(None) => {
                // EOF reached
                break;
            }
            Err(e) => {
                panic!("Audio decode error: {:?}", e);
            }
        }
    }

    // Should be EOF now
    assert!(
        decoder.is_eof(),
        "Audio decoder should be EOF after all frames decoded"
    );

    // Further decode_one calls should return None
    let result = decoder
        .decode_one()
        .expect("decode_one should not error at EOF");
    assert!(
        result.is_none(),
        "decode_one should return None at EOF for audio"
    );
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
fn test_audio_decoder_invalid_path() {
    let path = PathBuf::from("/invalid/path/audio.mp3");
    let result = AudioDecoder::open(&path).build();

    assert!(result.is_err(), "Should fail to open invalid path");
}

// ============================================================================
// Audio Frame Properties Validation Tests
// ============================================================================

#[test]
fn test_audio_frame_properties_match_stream_info() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let info = decoder.stream_info();

    let expected_channels = info.channels();
    let expected_sample_rate = info.sample_rate();

    // Decode several frames and verify properties
    for i in 0..5 {
        let frame = decoder
            .decode_one()
            .expect("Failed to decode")
            .unwrap_or_else(|| panic!("Audio frame {} should exist", i));

        assert_eq!(
            frame.channels(),
            expected_channels,
            "Frame {} channels mismatch",
            i
        );
        assert_eq!(
            frame.sample_rate(),
            expected_sample_rate,
            "Frame {} sample rate mismatch",
            i
        );
    }
}

#[test]
fn test_audio_frame_sample_format_consistency() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    let first_frame = decoder
        .decode_one()
        .expect("Failed to decode")
        .expect("First audio frame should exist");

    let expected_format = first_frame.format();

    // Decode more frames and verify format is consistent
    for i in 1..5 {
        let frame = decoder
            .decode_one()
            .expect("Failed to decode")
            .unwrap_or_else(|| panic!("Audio frame {} should exist", i));

        assert_eq!(
            frame.format(),
            expected_format,
            "Frame {} sample format mismatch",
            i
        );
    }
}

// ============================================================================
// Seeking Tests
// ============================================================================

#[test]
fn test_audio_seek_keyframe_mode() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Decode a few frames first
    for _ in 0..5 {
        let _ = decoder.decode_one().expect("Failed to decode");
    }

    // Seek to 2 seconds using keyframe mode
    let target = Duration::from_secs(2);
    let result = decoder.seek(target, SeekMode::Keyframe);

    assert!(
        result.is_ok(),
        "Audio keyframe seek should succeed: {:?}",
        result.err()
    );

    // Decode a frame after seeking
    let frame = decoder
        .decode_one()
        .expect("Failed to decode after seek")
        .expect("Audio frame should exist after seek");

    // Frame timestamp should be somewhere in the audio file
    let frame_time = frame.timestamp().as_duration();

    assert!(
        frame_time >= Duration::from_secs(1),
        "Audio frame after keyframe seek should be past 1s: frame_time={:?}, target={:?}",
        frame_time,
        target
    );
}

#[test]
fn test_audio_seek_exact_mode() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Seek to 3 seconds using exact mode
    let target = Duration::from_secs(3);
    let result = decoder.seek(target, SeekMode::Exact);

    assert!(
        result.is_ok(),
        "Audio exact seek should succeed: {:?}",
        result.err()
    );

    // Decode a frame after seeking
    let frame = decoder
        .decode_one()
        .expect("Failed to decode after seek")
        .expect("Audio frame should exist after seek");

    // Frame timestamp should be at or after target
    let frame_time = frame.timestamp().as_duration();

    assert!(
        frame_time >= target,
        "Audio frame timestamp should be at or after target for exact seek: target={:?}, frame_time={:?}",
        target,
        frame_time
    );
}

/// Timestamps of the first `count` audio frames, read without seeking.
///
/// Measured in the same run so the tests below do not encode a frame duration the
/// asset could change.
fn audio_frame_grid(count: usize) -> Vec<Duration> {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let mut grid = Vec::with_capacity(count);
    while grid.len() < count {
        let frame = decoder
            .decode_one()
            .expect("Failed to decode")
            .expect("Test asset is shorter than the requested grid");
        grid.push(frame.timestamp().as_duration());
    }
    grid
}

#[test]
fn audio_seek_exact_should_land_on_the_target_frame_and_not_consume_it() {
    let grid = audio_frame_grid(60);
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    for &target in &[grid[15], grid[40], grid[7]] {
        decoder
            .seek(target, SeekMode::Exact)
            .expect("Audio exact seek should succeed");
        let frame = decoder
            .decode_one()
            .expect("Failed to decode after seek")
            .expect("Audio frame should exist after seek");
        assert_eq!(
            frame.timestamp().as_duration(),
            target,
            "Exact seek must leave the frame at the target for the caller to read"
        );
    }
}

#[test]
fn audio_seek_exact_should_not_return_a_frame_held_back_by_an_earlier_seek() {
    let grid = audio_frame_grid(60);
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    decoder
        .seek(grid[40], SeekMode::Exact)
        .expect("Audio exact seek should succeed");
    // No decode in between: the frame at grid[40] is still held back when the
    // second seek runs, and must not survive it.
    decoder
        .seek(grid[10], SeekMode::Exact)
        .expect("Audio exact seek should succeed");

    let frame = decoder
        .decode_one()
        .expect("Failed to decode after seek")
        .expect("Audio frame should exist after seek");
    assert_eq!(
        frame.timestamp().as_duration(),
        grid[10],
        "A frame held back by the first seek must not outlive the second"
    );
}

#[test]
fn audio_flush_should_discard_the_frame_a_seek_held_back() {
    let grid = audio_frame_grid(60);
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    decoder
        .seek(grid[20], SeekMode::Exact)
        .expect("Audio exact seek should succeed");
    decoder.flush();

    let frame = decoder
        .decode_one()
        .expect("Failed to decode after flush")
        .expect("Audio frame should exist after flush");
    assert!(
        frame.timestamp().as_duration() > grid[20],
        "Flush must discard the held-back frame: got {:?}, target was {:?}",
        frame.timestamp().as_duration(),
        grid[20]
    );
}

#[test]
fn test_audio_seek_to_beginning() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Decode a few frames first
    for _ in 0..10 {
        let _ = decoder.decode_one().expect("Failed to decode");
    }

    // Seek back to beginning
    let result = decoder.seek(Duration::ZERO, SeekMode::Keyframe);

    assert!(
        result.is_ok(),
        "Audio seek to beginning should succeed: {:?}",
        result.err()
    );

    // Decode first frame
    let frame = decoder
        .decode_one()
        .expect("Failed to decode after seek to beginning")
        .expect("First audio frame should exist");

    // Frame should be near the beginning
    let frame_time = frame.timestamp().as_duration();
    assert!(
        frame_time < Duration::from_secs(1),
        "Audio frame after seek to beginning should be near start: frame_time={:?}",
        frame_time
    );
}

#[test]
fn test_audio_seek_multiple_times() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Perform multiple seeks
    let positions = [
        Duration::from_secs(5),
        Duration::from_secs(2),
        Duration::from_secs(8),
        Duration::from_secs(1),
    ];

    for (i, &pos) in positions.iter().enumerate() {
        let result = decoder.seek(pos, SeekMode::Keyframe);
        assert!(
            result.is_ok(),
            "Audio seek #{} to {:?} should succeed: {:?}",
            i,
            pos,
            result.err()
        );

        // Decode a frame after each seek
        let frame = decoder
            .decode_one()
            .unwrap_or_else(|e| panic!("Failed to decode after audio seek #{}: {:?}", i, e))
            .unwrap_or_else(|| panic!("Audio frame should exist after seek #{}", i));

        let frame_time = frame.timestamp().as_duration();
        assert!(
            frame_time >= Duration::ZERO,
            "Frame time should be valid after audio seek #{}",
            i
        );
    }
}

#[test]
fn test_audio_flush_decoder() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Decode a few frames
    for _ in 0..5 {
        let _ = decoder.decode_one().expect("Failed to decode");
    }

    // Flush the decoder
    decoder.flush();

    // Decoder should not be at EOF after flush
    assert!(
        !decoder.is_eof(),
        "Audio decoder should not be EOF after flush"
    );

    // Should be able to decode frames after flush
    let frame = decoder
        .decode_one()
        .expect("Failed to decode after flush")
        .expect("Audio frame should exist after flush");

    assert!(
        frame.samples() > 0,
        "Audio frame should be valid after flush"
    );
}

#[test]
fn test_audio_position_updates_after_seek() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Decode a few frames first
    for _ in 0..5 {
        let _ = decoder.decode_one().expect("Failed to decode");
    }

    // Initial position should be small
    let initial_pos = decoder.position();
    assert!(
        initial_pos < Duration::from_secs(1),
        "Initial position should be less than 1s after 5 frames"
    );

    // Seek to 2 seconds
    let target = Duration::from_secs(2);
    decoder
        .seek(target, SeekMode::Keyframe)
        .expect("Seek should succeed");

    // Decode a frame to update position
    let frame = decoder
        .decode_one()
        .expect("Failed to decode after seek")
        .expect("Audio frame should exist after seek");

    // Position should now be updated
    let pos_after_seek = decoder.position();
    let frame_time = frame.timestamp().as_duration();

    assert!(
        pos_after_seek >= Duration::from_secs(1),
        "Position after seek and decode should be close to target: pos={:?}, frame_time={:?}, target={:?}",
        pos_after_seek,
        frame_time,
        target
    );
}

// ============================================================================
// Iterator Pattern Tests
// ============================================================================

#[test]
fn test_audio_frame_iterator_basic() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Use iterator to decode first 10 frames
    let frames: Vec<_> = decoder.by_ref().take(10).collect();

    assert_eq!(frames.len(), 10, "Should collect 10 audio frames");

    // All frames should be Ok
    for (i, frame_result) in frames.iter().enumerate() {
        assert!(
            frame_result.is_ok(),
            "Audio frame {} should be Ok: {:?}",
            i,
            frame_result
        );
    }
}

#[test]
fn test_audio_frame_iterator_timestamps_increase() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    let mut last_pts = None;

    // Iterate over first 20 frames
    for (i, frame_result) in decoder.by_ref().take(20).enumerate() {
        let frame =
            frame_result.unwrap_or_else(|e| panic!("Failed to decode audio frame {}: {:?}", i, e));

        let pts = frame.timestamp().pts();

        if let Some(last) = last_pts {
            assert!(
                pts >= last,
                "Audio frame {} pts should not decrease: current={}, last={}",
                i,
                pts,
                last
            );
        }

        last_pts = Some(pts);
    }
}

#[test]
fn test_audio_frame_iterator_with_filter() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Seek to target position first to avoid scanning the entire file
    // (MP3 timestamps can be unreliable without seeking, causing a full-file scan)
    let target = Duration::from_secs(2);
    decoder
        .seek(target, SeekMode::Keyframe)
        .expect("Seek to 2s should succeed");

    // Collect 5 frames from the seeked position
    let frames: Vec<_> = decoder.by_ref().take(5).collect();

    assert_eq!(frames.len(), 5, "Should collect 5 audio frames after 2s");

    for (i, frame_result) in frames.iter().enumerate() {
        assert!(
            frame_result.is_ok(),
            "Audio frame {} after seek should be Ok",
            i
        );
    }
}

#[test]
fn test_audio_frame_iterator_early_break() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // Break early in iteration
    let mut count = 0;
    for frame_result in &mut decoder {
        let _ = frame_result.expect("Audio frame should decode successfully");
        count += 1;
        if count >= 3 {
            break;
        }
    }

    assert_eq!(
        count, 3,
        "Should decode exactly 3 audio frames before breaking"
    );

    // Should be able to continue decoding after early break
    let next_frame = decoder
        .decode_one()
        .expect("decode_one should work after iterator break")
        .expect("Next audio frame should exist");

    assert!(next_frame.samples() > 0, "Next audio frame should be valid");
}

#[test]
fn test_audio_frame_iterator_multiple_iterations() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    // First iteration
    let first_batch: Vec<_> = decoder.by_ref().take(5).collect();
    assert_eq!(
        first_batch.len(),
        5,
        "First batch should have 5 audio frames"
    );

    // Seek back to beginning
    decoder
        .seek(Duration::ZERO, SeekMode::Keyframe)
        .expect("Seek should succeed");

    // Second iteration
    let second_batch: Vec<_> = decoder.by_ref().take(5).collect();
    assert_eq!(
        second_batch.len(),
        5,
        "Second batch should have 5 audio frames"
    );
}

#[test]
fn audio_stream_info_codec_name_should_not_be_empty() {
    let decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let info = decoder.stream_info();

    assert!(
        !info.codec_name().is_empty(),
        "codec_name() should not be empty"
    );
}

// ============================================================================
// Timestamp Tests
// ============================================================================

#[test]
fn audio_frames_should_have_valid_timestamps() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");

    for (i, result) in decoder.by_ref().take(10).enumerate() {
        let frame = result.unwrap_or_else(|e| panic!("Frame {} failed: {:?}", i, e));
        assert!(
            frame.timestamp().is_valid(),
            "Frame {} should have a valid timestamp",
            i
        );
    }
}

#[test]
fn audio_timestamps_should_be_monotonically_increasing() {
    let mut decoder = create_audio_decoder().expect("Failed to create audio decoder");
    let mut last_pts: Option<i64> = None;

    for (i, result) in decoder.by_ref().take(20).enumerate() {
        let frame = result.unwrap_or_else(|e| panic!("Frame {} failed: {:?}", i, e));
        let ts = frame.timestamp();
        assert!(ts.is_valid(), "Frame {} timestamp should be valid", i);

        if let Some(prev) = last_pts {
            assert!(
                ts.pts() > prev,
                "Frame {} pts should increase: current={} last={}",
                i,
                ts.pts(),
                prev
            );
        }
        last_pts = Some(ts.pts());
    }
}
