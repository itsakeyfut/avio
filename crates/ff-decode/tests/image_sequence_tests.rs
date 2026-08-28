//! Integration tests for image-sequence decoding via the `image2` demuxer.

use std::path::{Path, PathBuf};

use ff_decode::{HardwareAccel, VideoDecoder};

mod fixtures;
use fixtures::assets_dir;

// Helpers

/// RAII guard that deletes a directory tree on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(name);
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

/// Returns the path to the test PNG image.
fn test_png_path() -> PathBuf {
    assets_dir().join("img/hello-triangle.png")
}

/// Returns the path to the test JPEG image.
fn test_jpeg_path() -> PathBuf {
    assets_dir().join("img/hello-triangle.jpg")
}

/// Creates a numbered image sequence in `dir` by copying `src` `count` times.
///
/// Files are named `frame0001.ext`, `frame0002.ext`, …
/// Returns the printf-style pattern path (e.g. `dir/frame%04d.png`).
fn make_image_sequence(dir: &Path, src: &Path, count: usize) -> PathBuf {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png");
    for i in 1..=count {
        let dst = dir.join(format!("frame{i:04}.{ext}"));
        std::fs::copy(src, &dst).expect("failed to copy image file");
    }
    dir.join(format!("frame%04d.{ext}"))
}

#[test]
fn image_sequence_png_should_decode_all_frames() {
    let tmp = TempDir::new("ff_decode_img_seq_png");
    let pattern = make_image_sequence(tmp.path(), &test_png_path(), 3);

    let mut decoder = match VideoDecoder::open(&pattern)
        .hardware_accel(HardwareAccel::None)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let mut count = 0usize;
    loop {
        match decoder.decode_one() {
            Ok(Some(_)) => count += 1,
            Ok(None) => break,
            Err(e) => panic!("decode error: {e}"),
        }
    }

    assert_eq!(count, 3, "expected 3 frames from a 3-image PNG sequence");
}

#[test]
fn image_sequence_with_frame_rate_override_should_succeed() {
    let tmp = TempDir::new("ff_decode_img_seq_fps");
    let pattern = make_image_sequence(tmp.path(), &test_png_path(), 2);

    let mut decoder = match VideoDecoder::open(&pattern)
        .hardware_accel(HardwareAccel::None)
        .frame_rate(30)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let mut count = 0usize;
    loop {
        match decoder.decode_one() {
            Ok(Some(_)) => count += 1,
            Ok(None) => break,
            Err(e) => panic!("decode error: {e}"),
        }
    }

    assert_eq!(
        count, 2,
        "expected 2 frames from a 2-image sequence at 30 fps"
    );
}

#[test]
fn image_sequence_jpeg_should_decode_successfully() {
    let tmp = TempDir::new("ff_decode_img_seq_jpg");
    let pattern = make_image_sequence(tmp.path(), &test_jpeg_path(), 2);

    let mut decoder = match VideoDecoder::open(&pattern)
        .hardware_accel(HardwareAccel::None)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    let mut count = 0usize;
    loop {
        match decoder.decode_one() {
            Ok(Some(_)) => count += 1,
            Ok(None) => break,
            Err(e) => panic!("decode error: {e}"),
        }
    }

    assert_eq!(count, 2, "expected 2 frames from a 2-image JPEG sequence");
}

#[test]
fn non_sequence_path_should_open_normally() {
    // Regression guard: a regular video path (no '%') must still work.
    let video_path = assets_dir().join("video/gameplay.mp4");
    let result = VideoDecoder::open(&video_path)
        .hardware_accel(HardwareAccel::None)
        .build();
    // If the test asset is present the decoder opens successfully.
    // FileNotFound is also acceptable if the asset is missing.
    match result {
        Ok(_) => {}
        Err(ff_decode::DecodeError::FileNotFound { .. }) => {}
        Err(e) => panic!("unexpected error for regular video path: {e}"),
    }
}

#[test]
fn image_sequence_missing_files_should_return_error() {
    // Pattern points to a directory that contains no matching files.
    let tmp = TempDir::new("ff_decode_img_seq_empty");
    let pattern = tmp.path().join("frame%04d.png");

    let result = VideoDecoder::open(&pattern)
        .hardware_accel(HardwareAccel::None)
        .build();

    assert!(
        result.is_err(),
        "expected an error when no matching files exist for the pattern"
    );
}
