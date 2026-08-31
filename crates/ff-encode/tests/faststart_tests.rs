//! Integration tests for `.faststart()` progressive MP4/MOV output.
//!
//! `.faststart()` sets `movflags=+faststart`, which relocates the `moov` atom to
//! the front of the file so it plays via progressive download before the whole
//! file is fetched. These tests encode a tiny MP4 and scan its top-level boxes to
//! confirm `moov` precedes `mdat` (and that, without the flag, it does not).
//!
//! All tests skip gracefully when no MP4-capable video encoder is compiled in.

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_encode::{VideoCodec, VideoEncoder};
use fixtures::{FileGuard, assert_valid_output_file, create_black_frame, test_output_path};
use std::path::Path;

/// Byte offset of the first top-level ISO-BMFF box of type `ty` (e.g. `b"moov"`).
///
/// Walks the box list from the start of the file: each box is a 32-bit big-endian
/// `size` followed by a 4-byte `type`. `size == 1` signals a 64-bit extended size
/// stored in the 8 bytes after the type; `size == 0` means the box runs to EOF.
/// Returns `None` if the type is not found or the structure is truncated.
fn top_level_box_offset(data: &[u8], ty: &[u8; 4]) -> Option<usize> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size32 = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let box_type = &data[pos + 4..pos + 8];
        if box_type == ty {
            return Some(pos);
        }
        let box_size: u64 = if size32 == 1 {
            if pos + 16 > data.len() {
                return None;
            }
            u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ])
        } else if size32 == 0 {
            // Extends to end of file: no further boxes.
            return None;
        } else {
            u64::from(size32)
        };
        if box_size < 8 {
            return None;
        }
        pos = pos.checked_add(usize::try_from(box_size).ok()?)?;
    }
    None
}

/// Encodes `frames` black frames of a tiny MP4 at `path`, optionally with
/// `.faststart()`. Returns `false` if no MP4-capable encoder is available (the
/// caller should skip). The default codec is H.264, which writes an `.mp4`.
fn encode_tiny_mp4(path: &Path, faststart: bool, frames: usize) -> bool {
    let builder = VideoEncoder::create(path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::default());
    let builder = if faststart {
        builder.faststart()
    } else {
        builder
    };

    let mut encoder = match builder.build() {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: MP4 video encoder unavailable: {e}");
            return false;
        }
    };

    for _ in 0..frames {
        encoder
            .push_video(&create_black_frame(320, 240))
            .expect("Failed to push video frame");
    }
    encoder.finish().expect("Failed to finish encoding");
    true
}

/// With `.faststart()`, the `moov` atom must appear before `mdat` in the file.
#[test]
fn faststart_should_place_moov_before_mdat() {
    let output_path = test_output_path("faststart_on.mp4");
    let _guard = FileGuard::new(output_path.clone());

    if !encode_tiny_mp4(&output_path, true, 10) {
        return;
    }
    assert_valid_output_file(&output_path);

    let bytes = std::fs::read(&output_path).expect("Failed to read output file");
    let moov = top_level_box_offset(&bytes, b"moov").expect("no moov box in output");
    let mdat = top_level_box_offset(&bytes, b"mdat").expect("no mdat box in output");
    assert!(
        moov < mdat,
        "faststart should place moov before mdat (moov={moov}, mdat={mdat})"
    );

    // Media content is intact: probe reports the frames we wrote.
    let info = ff_probe::open(&output_path).expect("Failed to probe output");
    let video = info.primary_video().expect("No video stream in output");
    assert!(
        video.frame_count().unwrap_or(0) >= 10,
        "Expected at least 10 frames, got {:?}",
        video.frame_count()
    );
}

/// Without `.faststart()`, the MP4 muxer writes `moov` after `mdat`. This proves
/// the flag is what moves the atom (non-vacuous baseline for the test above).
#[test]
fn without_faststart_moov_should_follow_mdat() {
    let output_path = test_output_path("faststart_off.mp4");
    let _guard = FileGuard::new(output_path.clone());

    if !encode_tiny_mp4(&output_path, false, 10) {
        return;
    }
    assert_valid_output_file(&output_path);

    let bytes = std::fs::read(&output_path).expect("Failed to read output file");
    let moov = top_level_box_offset(&bytes, b"moov").expect("no moov box in output");
    let mdat = top_level_box_offset(&bytes, b"mdat").expect("no mdat box in output");
    assert!(
        moov > mdat,
        "without faststart the default MP4 layout places moov after mdat (moov={moov}, mdat={mdat})"
    );
}
