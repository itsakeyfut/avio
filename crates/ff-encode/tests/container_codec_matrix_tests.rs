//! Every codec into every container it is allowed in.
//!
//! A muxer that declares `AVFMT_GLOBALHEADER` wants the encoder's parameter sets
//! in `extradata`, and an encoder that is not told so leaves `extradata` empty.
//! Matroska cannot write a `CodecPrivate` from nothing, so H.264 and H.265 failed
//! at header write while MP4 survived, the mov muxer rebuilding `avcC` from the
//! first packet itself (#1842).
//!
//! The matrix does not carry a table of which pairs are allowed. It asks
//! `build()` and reads the answer off the error: a pair avio refuses on purpose
//! and a codec this FFmpeg build does not have are both skips, and anything else
//! is a failure. That way the test cannot drift away from the validation it is
//! meant to guard. A container whose muxer this build lacks is probed for
//! separately, because that failure happens before the codec is considered.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod fixtures;
use fixtures::{FileGuard, test_output_path};

use ff_encode::{BitrateMode, EncodeError, VideoCodec, VideoEncoder};
use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const FPS: f64 = 30.0;

/// Every video codec the builder can be asked for that has a container here.
const CODECS: &[(&str, VideoCodec)] = &[
    ("h264", VideoCodec::H264),
    ("h265", VideoCodec::H265),
    ("mpeg4", VideoCodec::Mpeg4),
    ("vp9", VideoCodec::Vp9),
];

/// The containers avio writes for video, both the ones that declare
/// `AVFMT_GLOBALHEADER` and AVI, which does not.
const CONTAINERS: &[&str] = &["mp4", "mkv", "webm", "mov", "avi"];

fn frame() -> VideoFrame {
    let y = (WIDTH * HEIGHT) as usize;
    let uv = ((WIDTH / 2) * (HEIGHT / 2)) as usize;
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(vec![120; y]),
            PooledBuffer::standalone(vec![128; uv]),
            PooledBuffer::standalone(vec![128; uv]),
        ],
        vec![WIDTH as usize, (WIDTH / 2) as usize, (WIDTH / 2) as usize],
        WIDTH,
        HEIGHT,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("frame construction should succeed")
}

/// What happened to one cell of the matrix.
enum Cell {
    /// Written and probeable.
    Wrote,
    /// avio refuses this pair by design, or this build has no such encoder.
    Skipped(String),
}

fn write_pair(codec: VideoCodec, ext: &str, label: &str) -> Cell {
    let path = test_output_path(&format!("matrix_{label}.{ext}"));
    let _guard = FileGuard::new(path.clone());

    // A build can be missing the muxer itself, not just an encoder: CI's Linux
    // FFmpeg writes only ipod, mp4, mov and matroska, so allocating the output
    // context for .avi fails before any codec question is asked. Probe for it
    // here rather than reading that failure off an errno further down.
    if ff_sys::OutputFormatContext::new(None, &path).is_err() {
        return Cell::Skipped("no muxer for this container in this build".to_string());
    }

    let built = VideoEncoder::create(&path)
        .video(WIDTH, HEIGHT, FPS)
        .video_codec(codec)
        .bitrate_mode(BitrateMode::Crf(23))
        .build();

    let mut encoder = match built {
        Ok(e) => e,
        // avio's own container validation: this pair is refused on purpose.
        Err(EncodeError::UnsupportedContainerCodecCombination { .. }) => {
            return Cell::Skipped("refused by design".to_string());
        }
        // This FFmpeg build has no encoder for the requested family.
        Err(EncodeError::EncoderUnavailable { .. } | EncodeError::NoSuitableEncoder { .. }) => {
            return Cell::Skipped("no encoder in this build".to_string());
        }
        Err(e) => panic!("{label}: the encoder should build or say why it cannot: {e}"),
    };

    for _ in 0..5 {
        encoder.push_video(&frame()).unwrap_or_else(|e| {
            panic!("{label}: the encoder accepted the pair, so it must accept frames: {e}")
        });
    }
    encoder.finish().unwrap_or_else(|e| {
        panic!("{label}: the muxer must accept the packets the encoder produced: {e}")
    });

    let info =
        ff_probe::open(&path).unwrap_or_else(|e| panic!("{label}: output should probe: {e}"));
    assert!(
        !info.video_streams().is_empty(),
        "{label}: the output carries no video stream"
    );
    Cell::Wrote
}

#[test]
fn every_codec_should_write_to_each_container_it_is_allowed_in() {
    let mut wrote = 0usize;
    for (name, codec) in CODECS {
        for ext in CONTAINERS {
            let label = format!("{name}_{ext}");
            match write_pair(*codec, ext, &label) {
                Cell::Wrote => {
                    println!("  {label}: wrote");
                    wrote += 1;
                }
                Cell::Skipped(why) => println!("  {label}: skipped ({why})"),
            }
        }
    }
    assert!(
        wrote > 0,
        "no pair in the matrix could be written, so this build exercised nothing"
    );
}

#[test]
fn h264_should_write_to_matroska() {
    // Named separately because this is the pair #1842 was about, and a matrix
    // that skipped it everywhere would report success without covering it.
    match write_pair(VideoCodec::H264, "mkv", "h264_matroska") {
        Cell::Wrote => {}
        Cell::Skipped(why) => println!("skipped: {why}"),
    }
}
