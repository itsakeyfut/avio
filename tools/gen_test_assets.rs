//! Standalone tool to generate committed test assets.
//!
//! Run with:
//!   cargo run --manifest-path tools/Cargo.toml
//!
//! Writes `assets/test/hard_cut_video.mp4`: a 6-second video with five
//! hard scene cuts at 1 s, 2 s, 3 s, 4 s, and 5 s.  Used by
//! `ff-analysis/tests/analysis_tests.rs` so that test does not need
//! `ff-encode` as a dev-dependency.
//!
//! Writes `assets/test/negative_first_pts.mkv`: half a second of silent AAC in
//! Matroska, whose first decoded frame carries a negative timestamp because the
//! encoder's priming delay is expressed that way.  Used by
//! `ff-decode/tests/negative_pts_tests.rs` for the same reason.
//!
//! **Running this rewrites every asset below, and two of them no longer come out as they
//! are committed**: `hard_cut_video.mp4` regenerates one frame longer (6.000s against the
//! committed 5.967s) because the encoder has changed since it was written, and
//! `negative_first_pts.mkv` differs byte for byte. `ff-analysis` asserts scene cuts on the
//! first of those, so regenerate one asset at a time and commit only the one you meant to.
//!
//! Writes `assets/test/audio_longer_than_video.mp4` and
//! `assets/test/video_longer_than_audio.mp4`: the same content with the two stream
//! lengths swapped, half a second against two seconds.  A container's duration is its
//! longest stream, so each file is a case where the container says four times what one
//! of its streams holds.  Used by `ff-decode/tests/stream_duration_tests.rs` and
//! `avio/tests/clip_order_tests.rs` (#1861).

use std::path::{Path, PathBuf};

use ff_encode::{AudioCodec, AudioEncoder, VideoCodec, VideoEncoder};
use ff_format::{AudioFrame, PixelFormat, PooledBuffer, SampleFormat, Timestamp, VideoFrame};

fn yuv420p_frame(width: u32, height: u32, y: u8, cb: u8, cr: u8) -> VideoFrame {
    let y_plane = PooledBuffer::standalone(vec![y; (width * height) as usize]);
    let u_plane = PooledBuffer::standalone(vec![cb; ((width / 2) * (height / 2)) as usize]);
    let v_plane = PooledBuffer::standalone(vec![cr; ((width / 2) * (height / 2)) as usize]);
    VideoFrame::new(
        vec![y_plane, u_plane, v_plane],
        vec![width as usize, (width / 2) as usize, (width / 2) as usize],
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("failed to create frame")
}

fn generate_hard_cut_video(path: &Path) {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 120;
    const FPS: f64 = 30.0;
    const FRAMES_PER_SEGMENT: usize = 30;

    // Distinct solid colours — adjacent pairs differ by |ΔY| ≥ 65 so the
    // scene detector fires at every cut (threshold 0.4).
    let segments: &[(u8, u8, u8)] = &[
        (82, 90, 240),   // red    Y=82
        (235, 128, 128), // white  Y=235
        (41, 240, 110),  // blue   Y=41
        (210, 16, 146),  // yellow Y=210
        (145, 54, 34),   // green  Y=145
        (16, 128, 128),  // black  Y=16
    ];

    let mut encoder = VideoEncoder::create(path)
        .video(WIDTH, HEIGHT, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .build()
        .expect("failed to build encoder");

    for &(y, cb, cr) in segments {
        for _ in 0..FRAMES_PER_SEGMENT {
            encoder
                .push_video(&yuv420p_frame(WIDTH, HEIGHT, y, cb, cr))
                .expect("push_video failed");
        }
    }

    encoder.finish().expect("encoder finish failed");
    println!("Written: {}", path.display());
}

fn generate_negative_first_pts_audio(path: &Path) {
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u32 = 2;
    // `AudioFrame::new_silent` emits 1024 samples, which is also the AAC frame
    // size, so 24 frames is half a second.
    const FRAMES: i64 = 24;
    const SAMPLES_PER_FRAME: i64 = 1024;

    let mut encoder = AudioEncoder::create(path)
        .audio(SAMPLE_RATE, CHANNELS)
        .audio_codec(AudioCodec::Aac)
        .build()
        .expect("failed to build audio encoder");

    for i in 0..FRAMES {
        let pts_ms = i * SAMPLES_PER_FRAME * 1000 / i64::from(SAMPLE_RATE);
        let frame = AudioFrame::new_silent(SAMPLE_RATE, CHANNELS, SampleFormat::F32p, pts_ms);
        encoder.push(&frame).expect("push failed");
    }

    encoder.finish().expect("audio encoder finish failed");
    println!("Written: {}", path.display());
}

/// Writes an MP4 whose video and audio deliberately run for different lengths.
///
/// A container's duration is its longest stream, so `video_secs` of picture beside
/// `audio_secs` of sound gives a file where the container overstates the shorter of the
/// two by the difference. Both are written with a four-fold gap, which is far wider than
/// the rounding an AAC frame (1024 samples, about 21 ms) can account for, so a test using
/// them cannot pass by accident.
fn generate_mismatched_stream_lengths(path: &Path, video_secs: f64, audio_secs: f64) {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 120;
    const FPS: f64 = 30.0;
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u32 = 2;
    // The AAC frame size, so the audio lands on a whole number of packets.
    const SAMPLES_PER_FRAME: usize = 1024;

    let mut encoder = VideoEncoder::create(path)
        .video(WIDTH, HEIGHT, FPS)
        .video_codec(VideoCodec::Mpeg4)
        .audio(SAMPLE_RATE, CHANNELS)
        .audio_codec(AudioCodec::Aac)
        .audio_bitrate(128_000)
        .build()
        .expect("failed to build encoder");

    let video_frames = (video_secs * FPS).round() as usize;
    for _ in 0..video_frames {
        encoder
            .push_video(&yuv420p_frame(WIDTH, HEIGHT, 120, 90, 160))
            .expect("push_video failed");
    }

    let audio_samples = (f64::from(SAMPLE_RATE) * audio_secs).round() as usize;
    for _ in 0..audio_samples.div_ceil(SAMPLES_PER_FRAME) {
        let frame = AudioFrame::empty(SAMPLES_PER_FRAME, CHANNELS, SAMPLE_RATE, SampleFormat::F32)
            .expect("failed to create silent frame");
        encoder.push_audio(&frame).expect("push_audio failed");
    }

    encoder.finish().expect("encoder finish failed");
    println!("Written: {}", path.display());
}

fn main() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let out_dir = workspace_root.join("assets/test");
    std::fs::create_dir_all(&out_dir).expect("failed to create assets/test/");
    generate_hard_cut_video(&out_dir.join("hard_cut_video.mp4"));
    generate_negative_first_pts_audio(&out_dir.join("negative_first_pts.mkv"));
    generate_mismatched_stream_lengths(&out_dir.join("audio_longer_than_video.mp4"), 0.5, 2.0);
    generate_mismatched_stream_lengths(&out_dir.join("video_longer_than_audio.mp4"), 2.0, 0.5);
}
