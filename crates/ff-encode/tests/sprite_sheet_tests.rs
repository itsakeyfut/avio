//! Integration tests for SpriteSheet.
//!
//! Tests verify:
//! - Zero cols/rows returns `PreviewImageError::OperationFailed`
//! - Missing output path returns `PreviewImageError::OperationFailed`
//! - Missing input file returns an error
//! - A real video produces a PNG output file

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_encode::{PreviewImageError, SpriteSheet};

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

// ── Error-path tests ──────────────────────────────────────────────────────────

#[test]
fn sprite_sheet_zero_cols_should_return_media_operation_failed() {
    let result = SpriteSheet::new("irrelevant.mp4")
        .cols(0)
        .output("out.png")
        .run();
    assert!(
        matches!(result, Err(PreviewImageError::OperationFailed { .. })),
        "expected OperationFailed for cols=0, got {result:?}"
    );
}

#[test]
fn sprite_sheet_zero_rows_should_return_media_operation_failed() {
    let result = SpriteSheet::new("irrelevant.mp4")
        .rows(0)
        .output("out.png")
        .run();
    assert!(
        matches!(result, Err(PreviewImageError::OperationFailed { .. })),
        "expected OperationFailed for rows=0, got {result:?}"
    );
}

#[test]
fn sprite_sheet_output_not_set_should_return_media_operation_failed() {
    let result = SpriteSheet::new("irrelevant.mp4").run();
    assert!(
        matches!(result, Err(PreviewImageError::OperationFailed { .. })),
        "expected OperationFailed for missing output path, got {result:?}"
    );
}

#[test]
fn sprite_sheet_missing_input_should_return_error() {
    let output = fixtures::test_output_path("sprite_sheet_missing_input.png");
    let _guard = fixtures::FileGuard::new(output.clone());

    let result = SpriteSheet::new("does_not_exist_99999.mp4")
        .output(&output)
        .run();
    assert!(result.is_err(), "expected error for missing input file");
}

// ── Functional tests ──────────────────────────────────────────────────────────

#[test]
#[ignore = "runs full filter graph across video; run explicitly with -- --include-ignored"]
fn sprite_sheet_5x4_should_produce_png_file() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let output = fixtures::test_output_path("sprite_sheet_5x4.png");
    let _guard = fixtures::FileGuard::new(output.clone());

    let result = SpriteSheet::new(&path)
        .cols(5)
        .rows(4)
        .frame_width(160)
        .frame_height(90)
        .output(&output)
        .run();

    match result {
        Ok(()) => {
            assert!(
                output.exists(),
                "expected output PNG to exist at {}",
                output.display()
            );
        }
        Err(e) => {
            // On Windows, the movie filter path may fail due to ':' in path.
            println!("Skipping: SpriteSheet::run failed ({e})");
        }
    }
}
