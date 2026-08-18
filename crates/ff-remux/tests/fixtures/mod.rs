//! Test fixtures and helpers for ff-encode integration tests.
//!
//! This module provides common utilities for testing video encoding:
//! - Test frame generation (black frames, colored frames)
//! - Encoder creation helpers with default settings
//! - Output path helpers with automatic cleanup
//! - Assertions and validation helpers

#![allow(dead_code)]

use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};
use std::path::PathBuf;

// ============================================================================
// Output Path Helpers
// ============================================================================

/// Returns the path to the test output directory.
pub fn test_output_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{}/target/test-output", manifest_dir))
}

/// Creates a test output path with the given filename.
///
/// The file will be created in the test output directory and will be
/// automatically cleaned up after tests complete.
pub fn test_output_path(filename: &str) -> PathBuf {
    let output_dir = test_output_dir();
    std::fs::create_dir_all(&output_dir).ok();
    output_dir.join(filename)
}

// ============================================================================
// Frame Generation Helpers
// ============================================================================

/// Creates a black video frame with the specified dimensions.
///
/// Uses YUV420P pixel format, which is the most common for video encoding.
pub fn create_black_frame(width: u32, height: u32) -> VideoFrame {
    create_yuv420p_frame(width, height, 0, 128, 128)
}

/// Creates a white video frame with the specified dimensions.
pub fn create_white_frame(width: u32, height: u32) -> VideoFrame {
    create_yuv420p_frame(width, height, 255, 128, 128)
}

/// Creates a YUV420P video frame with the specified Y, U, V values.
fn create_yuv420p_frame(width: u32, height: u32, y: u8, u: u8, v: u8) -> VideoFrame {
    // Calculate plane sizes for YUV420P
    let y_size = (width * height) as usize;
    let uv_size = ((width / 2) * (height / 2)) as usize;

    // Create standalone buffers
    let y_plane = PooledBuffer::standalone(vec![y; y_size]);
    let u_plane = PooledBuffer::standalone(vec![u; uv_size]);
    let v_plane = PooledBuffer::standalone(vec![v; uv_size]);

    // Create strides
    let strides = vec![width as usize, (width / 2) as usize, (width / 2) as usize];

    VideoFrame::new(
        vec![y_plane, u_plane, v_plane],
        strides,
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("Failed to create video frame")
}

// ============================================================================
// Test Cleanup Helpers
// ============================================================================

/// Guard that deletes a file when dropped.
///
/// Useful for ensuring test output files are cleaned up even if tests panic.
pub struct FileGuard {
    path: PathBuf,
}

impl FileGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// ============================================================================
// Validation Helpers
// ============================================================================

/// Checks if a file exists and has non-zero size.
pub fn assert_valid_output_file(path: &PathBuf) {
    assert!(path.exists(), "Output file does not exist: {:?}", path);

    let metadata = std::fs::metadata(path).expect("Failed to read file metadata");
    assert!(metadata.len() > 0, "Output file is empty: {:?}", path);
}

/// Returns the file size in bytes.
pub fn get_file_size(path: &PathBuf) -> u64 {
    std::fs::metadata(path)
        .expect("Failed to read file metadata")
        .len()
}
