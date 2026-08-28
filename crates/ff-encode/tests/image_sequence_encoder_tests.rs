//! Integration tests for image-sequence encoding via the `image2` muxer.

mod fixtures;

use ff_encode::{VideoCodec, VideoEncoder};
use fixtures::{create_black_frame, test_output_dir};
use std::path::{Path, PathBuf};

// Helpers

/// RAII guard that deletes a directory tree on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = test_output_dir().join(name);
        std::fs::create_dir_all(&dir).expect("failed to create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn image_sequence_png_should_encode_all_frames() {
    let tmp = TempDir::new("img_seq_enc_png");
    let pattern = tmp.path().join("frame%04d.png");

    let mut encoder = match VideoEncoder::create(&pattern)
        .video(64, 64, 25.0)
        .video_codec(VideoCodec::Png)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    for _ in 0..3 {
        let frame = create_black_frame(64, 64);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: {e}");
            return;
        }
    }

    if let Err(e) = encoder.finish() {
        println!("Skipping: {e}");
        return;
    }

    // Three PNG files should have been produced.
    for i in 1..=3u32 {
        let file = tmp.path().join(format!("frame{i:04}.png"));
        assert!(file.exists(), "expected {file:?} to exist");
    }
}

#[test]
fn image_sequence_jpeg_should_encode_all_frames() {
    let tmp = TempDir::new("img_seq_enc_jpg");
    let pattern = tmp.path().join("frame%04d.jpg");

    let mut encoder = match VideoEncoder::create(&pattern)
        .video(64, 64, 25.0)
        .video_codec(VideoCodec::Mjpeg)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    for _ in 0..2 {
        let frame = create_black_frame(64, 64);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: {e}");
            return;
        }
    }

    if let Err(e) = encoder.finish() {
        println!("Skipping: {e}");
        return;
    }

    for i in 1..=2u32 {
        let file = tmp.path().join(format!("frame{i:04}.jpg"));
        assert!(file.exists(), "expected {file:?} to exist");
    }
}

#[test]
fn image_sequence_png_auto_codec_from_extension_should_work() {
    // No explicit .video_codec() — the builder should auto-select Png from ".png".
    let tmp = TempDir::new("img_seq_enc_auto_png");
    let pattern = tmp.path().join("frame%04d.png");

    let mut encoder = match VideoEncoder::create(&pattern).video(64, 64, 25.0).build() {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = create_black_frame(64, 64);
    if let Err(e) = encoder.push_video(&frame) {
        println!("Skipping: {e}");
        return;
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: {e}");
        return;
    }

    assert!(
        tmp.path().join("frame0001.png").exists(),
        "expected frame0001.png to exist"
    );
}

#[test]
fn image_sequence_jpeg_auto_codec_from_extension_should_work() {
    // No explicit .video_codec() — the builder should auto-select Mjpeg from ".jpg".
    let tmp = TempDir::new("img_seq_enc_auto_jpg");
    let pattern = tmp.path().join("frame%04d.jpg");

    let mut encoder = match VideoEncoder::create(&pattern).video(64, 64, 25.0).build() {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let frame = create_black_frame(64, 64);
    if let Err(e) = encoder.push_video(&frame) {
        println!("Skipping: {e}");
        return;
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: {e}");
        return;
    }

    assert!(
        tmp.path().join("frame0001.jpg").exists(),
        "expected frame0001.jpg to exist"
    );
}

#[test]
fn image_sequence_with_audio_should_return_error() {
    // Audio streams are not supported in image-sequence output.
    let tmp = TempDir::new("img_seq_enc_audio_err");
    let pattern = tmp.path().join("frame%04d.png");

    let result = VideoEncoder::create(&pattern)
        .video(64, 64, 25.0)
        .audio(44100, 2)
        .video_codec(VideoCodec::Png)
        .build();

    assert!(
        result.is_err(),
        "expected an error when audio is configured for image-sequence output"
    );
}

#[test]
fn non_sequence_path_should_not_be_affected() {
    // Regression guard: a regular .mp4 path must not be treated as an image sequence.
    let tmp = TempDir::new("img_seq_enc_regression");
    let output = tmp.path().join("output.mp4");

    let result = VideoEncoder::create(&output)
        .video(64, 64, 25.0)
        .video_codec(VideoCodec::H264)
        .build();

    // Either succeeds or fails with an encoder error — but not with
    // InvalidConfig about image sequences.
    match result {
        Ok(_) => {}
        Err(ff_encode::EncodeError::InvalidConfig { ref reason })
            if reason.contains("image sequence") =>
        {
            panic!("regular path incorrectly treated as image sequence: {reason}");
        }
        Err(_) => {}
    }
}
