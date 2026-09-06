//! Decoding from a caller-supplied byte source (#1600).
//!
//! The fixture is deliberate: `gameplay.mp4` carries its `moov` atom at the end
//! of the file, so opening it at all forces the demuxer to seek backwards. A
//! source whose seek callback were wrong or absent would fail here rather than
//! quietly decode a prefix.
//!
//! Probe-gated: the whole suite skips when the asset or the H.264 decoder is
//! unavailable, which is the state of CI's `--disable-everything` `FFmpeg` build.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fixtures;

use std::io::Cursor;

use ff_decode::VideoDecoder;
use fixtures::test_video_path;

/// `(frame count, first frame's dimensions)` for a decoder, capped so the test
/// stays quick on a long asset.
fn decode_stats(mut decoder: VideoDecoder) -> (usize, Option<(u32, u32)>) {
    let (mut n, mut dims) = (0usize, None);
    while n < 24 {
        match decoder.decode_one() {
            Ok(Some(frame)) => {
                if dims.is_none() {
                    dims = Some((frame.width(), frame.height()));
                }
                n += 1;
            }
            _ => break,
        }
    }
    (n, dims)
}

/// The asset's bytes, or `None` when the environment cannot run this suite.
fn asset_bytes() -> Option<Vec<u8>> {
    let path = test_video_path();
    if !path.exists() {
        return None;
    }
    // If the file itself will not open, the decoder is missing rather than the
    // reader path being broken: skip instead of reporting a false failure.
    VideoDecoder::open(&path).build().ok()?;
    std::fs::read(&path).ok()
}

#[test]
fn decoding_from_a_reader_should_match_decoding_the_file() {
    let Some(bytes) = asset_bytes() else {
        return;
    };
    let from_file = VideoDecoder::open(test_video_path())
        .build()
        .expect("the control decoder opened once already");
    let (file_n, file_dims) = decode_stats(from_file);

    let from_reader = VideoDecoder::from_reader(Cursor::new(bytes))
        .build()
        .expect("a seekable in-memory source must open");
    let (reader_n, reader_dims) = decode_stats(from_reader);

    println!("custom io: file=({file_n}, {file_dims:?}) reader=({reader_n}, {reader_dims:?})");
    assert!(file_n > 0, "the control must decode something");
    assert_eq!(
        reader_n, file_n,
        "decoding from memory must yield the same frames as decoding the file"
    );
    assert_eq!(
        reader_dims, file_dims,
        "decoding from memory must yield the same dimensions"
    );
}

#[test]
fn a_reader_source_should_be_seekable_enough_for_a_trailing_moov() {
    // `gameplay.mp4` stores `moov` at the end, so `avformat_open_input` only
    // succeeds if the seek callback works. Opening *is* the assertion; the
    // separate test above would still pass on a prefix-only read if the asset
    // ever changed, so this one names the property.
    let Some(bytes) = asset_bytes() else {
        return;
    };
    let opened = VideoDecoder::from_reader(Cursor::new(bytes)).build();
    assert!(
        opened.is_ok(),
        "a seekable source must open a file whose moov atom is at the end: {:?}",
        opened.err()
    );
}

#[test]
fn a_truncated_source_should_fail_to_open_rather_than_decode_garbage() {
    // The other side of the gate: the reader path must still report a bad input
    // as an error, not as an empty-but-successful decode.
    let Some(bytes) = asset_bytes() else {
        return;
    };
    let head = bytes[..bytes.len().min(64)].to_vec();
    let opened = VideoDecoder::from_reader(Cursor::new(head)).build();
    assert!(
        opened.is_err(),
        "64 bytes of a container is not a decodable input"
    );
}
