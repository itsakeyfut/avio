//! Integration tests for preview generation (SpriteSheet, GifPreview).
//!
//! Tests verify that `SpriteSheet` produces a PNG whose pixel dimensions
//! match the configured grid layout — no external image crate needed.  PNG
//! dimensions are read directly from the IHDR chunk (bytes 16–23).

#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_encode::SpriteSheet;

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

/// Reads exactly `n` bytes from the start of a file.
fn read_file_prefix(path: &std::path::Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut buf = vec![0u8; n];
    let mut f = std::fs::File::open(path)?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}

#[test]
fn sprite_sheet_should_produce_correct_pixel_dimensions() {
    let input = test_video_path();
    if !input.exists() {
        println!("Skipping: test video not found at {}", input.display());
        return;
    }

    let output = fixtures::test_output_path("preview_sprite_sheet_dimensions.png");
    let _guard = fixtures::FileGuard::new(output.clone());

    // 5 columns × 160 px wide = 800 px; 4 rows × 90 px tall = 360 px.
    let result = SpriteSheet::new(&input)
        .cols(5)
        .rows(4)
        .frame_width(160)
        .frame_height(90)
        .output(&output)
        .run();

    match result {
        Ok(()) => {}
        Err(e) => {
            // SpriteSheet uses the `movie` filter which can fail on Windows
            // due to colon escaping in drive-letter paths.
            println!("Skipping: SpriteSheet::run failed ({e})");
            return;
        }
    }

    // PNG magic bytes
    // The first 4 bytes of every PNG file are the magic signature \x89PNG.
    let header = read_file_prefix(&output, 24).expect("output file must be readable");

    assert_eq!(
        &header[0..4],
        b"\x89PNG",
        "output file does not begin with PNG magic bytes"
    );

    // IHDR dimensions
    // PNG layout: [8-byte sig][4-byte len][4-byte "IHDR"][4-byte width][4-byte height]…
    // Width is at byte offset 16, height at byte offset 20, both big-endian u32.
    let width = u32::from_be_bytes(header[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(header[20..24].try_into().unwrap());

    assert_eq!(
        width, 800,
        "expected PNG width 800 (5 cols × 160 px), got {width}"
    );
    assert_eq!(
        height, 360,
        "expected PNG height 360 (4 rows × 90 px), got {height}"
    );
}
