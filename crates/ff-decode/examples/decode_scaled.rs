//! Decode video frames at a scaled-down resolution.
//!
//! Demonstrates the three scaling options on [`VideoDecoder`]:
//!
//! - `output_size(width, height)` — exact target dimensions
//! - `output_width(width)` — fit to width, aspect ratio preserved
//! - `output_height(height)` — fit to height, aspect ratio preserved
//!
//! Scaling happens inside the same `libswscale` pass used for pixel-format
//! conversion, so no extra copy is required. Combine with `output_format()`
//! to convert format and resize in one step.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example decode_scaled -- --input input.mp4
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_format::PixelFormat;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: decode_scaled --input <file>");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!("Input: {in_name}");
    println!();

    // ── Source info ───────────────────────────────────────────────────────────

    let probe = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Cannot open video: {e}");
            process::exit(1);
        }
    };
    let src_w = probe.width();
    let src_h = probe.height();
    println!(
        "Source: {src_w}×{src_h}  codec={}",
        probe.stream_info().codec_name()
    );
    println!();

    // ── output_size — exact 320×240 ───────────────────────────────────────────
    //
    // Both width and height are specified directly. Width and height are
    // rounded up to the nearest even number if needed.

    println!("=== output_size(320, 240) ===");

    let mut dec = match VideoDecoder::open(&input).output_size(320, 240).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping: {e}");
            return;
        }
    };

    match dec.decode_one() {
        Ok(Some(frame)) => println!(
            "Frame: {}×{}  format={}",
            frame.width(),
            frame.height(),
            frame.format()
        ),
        Ok(None) => println!("No frames"),
        Err(e) => println!("Error: {e}"),
    }
    println!();

    // ── output_width — fit to 640 px wide ─────────────────────────────────────
    //
    // Height is derived from the source aspect ratio and rounded to the
    // nearest even number. Useful for responsive thumbnails or preview grids.

    println!("=== output_width(640) ===");

    let mut dec = match VideoDecoder::open(&input).output_width(640).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping: {e}");
            return;
        }
    };

    match dec.decode_one() {
        Ok(Some(frame)) => println!(
            "Frame: {}×{}  format={}  (src {}×{})",
            frame.width(),
            frame.height(),
            frame.format(),
            src_w,
            src_h
        ),
        Ok(None) => println!("No frames"),
        Err(e) => println!("Error: {e}"),
    }
    println!();

    // ── output_height — fit to 360 px tall ───────────────────────────────────
    //
    // Width is derived from the source aspect ratio. Useful when you must
    // target a fixed vertical resolution (e.g. a 360p preview stream).

    println!("=== output_height(360) ===");

    let mut dec = match VideoDecoder::open(&input).output_height(360).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping: {e}");
            return;
        }
    };

    match dec.decode_one() {
        Ok(Some(frame)) => println!(
            "Frame: {}×{}  format={}  (src {}×{})",
            frame.width(),
            frame.height(),
            frame.format(),
            src_w,
            src_h
        ),
        Ok(None) => println!("No frames"),
        Err(e) => println!("Error: {e}"),
    }
    println!();

    // ── output_size + output_format — scale and convert in one pass ───────────
    //
    // Combining output_size() with output_format() performs both operations
    // in a single libswscale call — no extra intermediate frame is allocated.

    println!("=== output_size(160, 90) + output_format(Rgb24) ===");

    let mut dec = match VideoDecoder::open(&input)
        .output_size(160, 90)
        .output_format(PixelFormat::Rgb24)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Skipping: {e}");
            return;
        }
    };

    match dec.decode_one() {
        Ok(Some(frame)) => {
            let expected_bytes = 160 * 90 * 3; // RGB24: 3 bytes per pixel
            println!(
                "Frame: {}×{}  format={}  planes={}  total_bytes={}  (expected {})",
                frame.width(),
                frame.height(),
                frame.format(),
                frame.num_planes(),
                frame.total_size(),
                expected_bytes,
            );
        }
        Ok(None) => println!("No frames"),
        Err(e) => println!("Error: {e}"),
    }
}
