//! Soften mask edges using `feather_mask` after a rectangle crop.
//!
//! This example demonstrates the difference between a hard-edged mask
//! (`radius = 0`) and a feathered mask (`radius > 0`).  A centered rectangle
//! is first cut with `rect_mask`, then `feather_mask` applies a Gaussian blur
//! to the alpha channel so that the subject blends naturally into composites
//! instead of producing a sharp cutout.
//!
//! Typical production use: apply `rect_mask` or `polygon_matte` to isolate a
//! subject, then chain `feather_mask` before a Porter-Duff composite.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example mask_feathering --features "decode encode filter" -- \
//!   --input video.mp4 --radius 15 --output feathered.mp4 [--invert]
//! ```
//!
//! Set `--radius 0` to see the hard-edged mask for comparison.

use std::{path::PathBuf, process};

use ff_decode::VideoDecoder;
use ff_encode::VideoEncoder;
use ff_filter::FilterGraphBuilder;
use ff_format::VideoCodec;

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    input: PathBuf,
    output: PathBuf,
    radius: u32,
    invert: bool,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    let get = |flag: &str| -> Option<String> {
        raw.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
    };

    let input = if let Some(p) = get("--input") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --input <path> is required");
        process::exit(1);
    };
    let output = if let Some(p) = get("--output") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --output <path> is required");
        process::exit(1);
    };

    Args {
        input,
        output,
        radius: get("--radius").and_then(|v| v.parse().ok()).unwrap_or(15),
        invert: raw.iter().any(|a| a == "--invert"),
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // ── 1. Open decoder ───────────────────────────────────────────────────────

    let mut vdec = match VideoDecoder::open(&args.input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: failed to open '{}': {e}", args.input.display());
            process::exit(1);
        }
    };

    let width = vdec.width();
    let height = vdec.height();
    let fps = vdec.frame_rate();

    // ── 2. Derive a centred rectangle (inner 60 % of the frame) ──────────────

    let rect_x = width / 5;
    let rect_y = height / 5;
    let rect_w = width * 3 / 5;
    let rect_h = height * 3 / 5;

    // ── 3. Build filter graph: rect_mask → feather_mask ───────────────────────
    //
    // rect_mask cuts a hard rectangular window into the frame.
    // feather_mask then blurs the alpha channel edges by `radius` pixels,
    // producing a smooth transition from opaque to transparent.
    // With radius = 0, feather_mask is a no-op — useful for side-by-side
    // comparison with a feathered output.

    let mut graph = match FilterGraphBuilder::new()
        .rect_mask(rect_x, rect_y, rect_w, rect_h, args.invert)
        .feather_mask(args.radius)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: failed to build filter graph: {e}");
            return;
        }
    };

    // ── 4. Build encoder ──────────────────────────────────────────────────────

    let mut encoder = match VideoEncoder::create(&args.output)
        .video(width, height, fps)
        .video_codec(VideoCodec::H264)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping: failed to build encoder: {e}");
            return;
        }
    };

    println!(
        "Input:   {}  ({width}x{height}  {fps:.2} fps)",
        args.input.display()
    );
    println!(
        "Rect:    x={rect_x}  y={rect_y}  w={rect_w}  h={rect_h}  invert={}",
        args.invert
    );
    println!(
        "Feather: radius={}{}",
        args.radius,
        if args.radius == 0 {
            "  (hard edge — no feathering)"
        } else {
            "px"
        }
    );
    println!("Output:  {}", args.output.display());
    println!();
    println!("Encoding...");

    // ── 5. Decode → filter → encode loop ─────────────────────────────────────

    let mut frames: u64 = 0;

    loop {
        let raw = match vdec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: decode failed: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = graph.push_video(0, &raw) {
            eprintln!("error: push_video failed: {e}");
            process::exit(1);
        }

        loop {
            match graph.pull_video() {
                Ok(Some(filtered)) => {
                    if let Err(e) = encoder.push_video(&filtered) {
                        eprintln!("error: encoder push_video failed: {e}");
                        process::exit(1);
                    }
                    frames += 1;
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("error: pull_video failed: {e}");
                    process::exit(1);
                }
            }
        }
    }

    // ── 6. Finish ─────────────────────────────────────────────────────────────

    if let Err(e) = encoder.finish() {
        eprintln!("error: encoder.finish() failed: {e}");
        process::exit(1);
    }

    let size_str = match std::fs::metadata(&args.output) {
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

    println!(
        "Done. {}  {size_str}  frames={frames}",
        args.output.display()
    );
}
