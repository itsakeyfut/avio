//! Standalone tool to generate committed test assets.
//!
//! Run with:
//!   cargo run --manifest-path tools/Cargo.toml
//!
//! Writes `assets/test/hard_cut_video.mp4`: a 6-second video with five
//! hard scene cuts at 1 s, 2 s, 3 s, 4 s, and 5 s.  Used by
//! `ff-analysis/tests/analysis_tests.rs` so that test does not need
//! `ff-encode` as a dev-dependency.

use std::path::{Path, PathBuf};

use ff_encode::{VideoCodec, VideoEncoder};
use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

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

fn main() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let out_dir = workspace_root.join("assets/test");
    std::fs::create_dir_all(&out_dir).expect("failed to create assets/test/");
    generate_hard_cut_video(&out_dir.join("hard_cut_video.mp4"));
}
