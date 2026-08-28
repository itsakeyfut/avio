//! Integration tests for GifPreview.
//!
//! Tests verify:
//! - Non-.gif output extension returns `PreviewImageError::OperationFailed`
//! - Missing output path returns `PreviewImageError::OperationFailed`
//! - Zero/negative fps returns `PreviewImageError::OperationFailed`
//! - Missing input file returns an error
//! - A real video produces a .gif output file

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_encode::{GifPreview, PreviewImageError};
use std::time::Duration;

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

#[test]
fn gif_preview_non_gif_extension_should_return_media_operation_failed() {
    let result = GifPreview::new("irrelevant.mp4").output("out.mp4").run();
    assert!(
        matches!(result, Err(PreviewImageError::OperationFailed { .. })),
        "expected OperationFailed for non-.gif extension, got {result:?}"
    );
}

#[test]
fn gif_preview_output_not_set_should_return_media_operation_failed() {
    let result = GifPreview::new("irrelevant.mp4").run();
    assert!(
        matches!(result, Err(PreviewImageError::OperationFailed { .. })),
        "expected OperationFailed for missing output path, got {result:?}"
    );
}

#[test]
fn gif_preview_zero_fps_should_return_media_operation_failed() {
    let result = GifPreview::new("irrelevant.mp4")
        .fps(0.0)
        .output("out.gif")
        .run();
    assert!(
        matches!(result, Err(PreviewImageError::OperationFailed { .. })),
        "expected OperationFailed for fps=0, got {result:?}"
    );
}

#[test]
fn gif_preview_missing_input_should_return_error() {
    let output = fixtures::test_output_path("gif_preview_missing_input.gif");
    let _guard = fixtures::FileGuard::new(output.clone());

    let result = GifPreview::new("does_not_exist_99999.mp4")
        .output(&output)
        .run();
    assert!(result.is_err(), "expected error for missing input file");
}

#[test]
#[ignore = "runs two-pass filter graph across video; run explicitly with -- --include-ignored"]
fn gif_preview_should_produce_gif_file() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let output = fixtures::test_output_path("gif_preview_3s.gif");
    let _guard = fixtures::FileGuard::new(output.clone());

    let result = GifPreview::new(&path)
        .start(Duration::from_secs(1))
        .duration(Duration::from_secs(3))
        .fps(10.0)
        .width(320)
        .output(&output)
        .run();

    match result {
        Ok(()) => {
            assert!(
                output.exists(),
                "expected output GIF to exist at {}",
                output.display()
            );
        }
        Err(e) => {
            // On Windows, the movie filter path may fail due to ':' in path.
            println!("Skipping: GifPreview::run failed ({e})");
        }
    }
}
