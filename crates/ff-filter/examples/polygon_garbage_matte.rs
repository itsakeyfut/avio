//! Apply a polygon garbage matte to isolate a subject in a video.
//!
//! A garbage matte is a rough polygon drawn around the subject to discard
//! unwanted areas (set edges, lighting rigs, crew) before chroma keying.
//! This example applies a hardcoded centered hexagon that covers roughly the
//! central 70 % of the frame.  Pixels outside the polygon are made transparent
//! (or inside, when `--invert` is passed).  An optional `--feather` radius
//! softens the polygon edges to avoid hard cuts.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example polygon_garbage_matte -- \
//!   --input video.mp4 --output masked.mp4 [--feather 8] [--invert]
//! ```

use std::{path::PathBuf, process};

use ff_decode::VideoDecoder;
use ff_encode::VideoEncoder;
use ff_filter::FilterGraphBuilder;
use ff_format::VideoCodec;

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    input: PathBuf,
    output: PathBuf,
    feather: Option<u32>,
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
        feather: get("--feather").and_then(|v| v.parse().ok()),
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

    // ── 2. Define polygon ─────────────────────────────────────────────────────
    //
    // A centered hexagon that covers roughly the central 70 % of the frame.
    // Vertices are in normalised (0.0–1.0) coordinates, clockwise order.
    // Replace these with your own subject-specific polygon in production.
    let vertices: Vec<(f32, f32)> = vec![
        (0.50, 0.15), // top centre
        (0.85, 0.35), // upper right
        (0.85, 0.65), // lower right
        (0.50, 0.85), // bottom centre
        (0.15, 0.65), // lower left
        (0.15, 0.35), // upper left
    ];

    // ── 3. Build filter graph ─────────────────────────────────────────────────

    let mut builder = FilterGraphBuilder::new().polygon_matte(vertices.clone(), args.invert);

    if let Some(radius) = args.feather {
        builder = builder.feather_mask(radius);
    }

    let mut graph = match builder.build() {
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

    let feather_str = args
        .feather
        .map_or_else(|| "none".to_string(), |r| format!("{r}px"));

    println!(
        "Input:    {}  ({width}x{height}  {fps:.2} fps)",
        args.input.display()
    );
    println!(
        "Polygon:  {} vertices  invert={}",
        vertices.len(),
        args.invert
    );
    println!("Feather:  {feather_str}");
    println!("Output:   {}", args.output.display());
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
