//! Generate a sprite sheet (thumbnail grid) PNG from a video file.
//!
//! Uses [`SpriteSheet`] to extract `cols × rows` evenly-spaced frames and tile
//! them into a single PNG image.  Sprite sheets are used in video players as
//! hover-preview scrub bars.
//!
//! The output PNG dimensions are `(cols × frame_width) × (rows × frame_height)`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example sprite_sheet --features preview-image -- \
//!     --input  video.mp4        \
//!     --output preview.png
//!
//! # Custom grid (5 columns × 4 rows, 160×90 px per frame):
//! cargo run --example sprite_sheet --features preview-image -- \
//!     --input       video.mp4   \
//!     --output      preview.png \
//!     --cols        5           \
//!     --rows        4           \
//!     --frame-width  160        \
//!     --frame-height  90
//! ```

use std::process;

use ff_encode::SpriteSheet;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut cols = 10u32;
    let mut rows = 10u32;
    let mut frame_width = 160u32;
    let mut frame_height = 90u32;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--cols" => {
                let raw = args.next().unwrap_or_default();
                cols = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid cols: {raw}");
                    process::exit(1);
                });
            }
            "--rows" => {
                let raw = args.next().unwrap_or_default();
                rows = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid rows: {raw}");
                    process::exit(1);
                });
            }
            "--frame-width" => {
                let raw = args.next().unwrap_or_default();
                frame_width = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid frame-width: {raw}");
                    process::exit(1);
                });
            }
            "--frame-height" => {
                let raw = args.next().unwrap_or_default();
                frame_height = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid frame-height: {raw}");
                    process::exit(1);
                });
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: sprite_sheet --input <video> --output <png> [--cols N] [--rows N] [--frame-width W] [--frame-height H]");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    let total_w = cols * frame_width;
    let total_h = rows * frame_height;

    println!("Input:        {input}");
    println!(
        "Grid:         {cols}×{rows}  ({} frames total)",
        cols * rows
    );
    println!("Frame size:   {frame_width}×{frame_height} px");
    println!("Output size:  {total_w}×{total_h} px");
    println!("Output:       {output}");
    println!();
    println!("Generating sprite sheet…");

    SpriteSheet::new(&input)
        .cols(cols)
        .rows(rows)
        .frame_width(frame_width)
        .frame_height(frame_height)
        .output(&output)
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    let size = match std::fs::metadata(&output) {
        Ok(m) => {
            #[allow(clippy::cast_precision_loss)]
            let kb = m.len() as f64 / 1024.0;
            if kb < 1024.0 {
                format!("{kb:.0} KB")
            } else {
                format!("{:.1} MB", kb / 1024.0)
            }
        }
        Err(_) => "(unknown size)".to_string(),
    };

    println!("Done. {output}  {size}  ({total_w}×{total_h})");
}
