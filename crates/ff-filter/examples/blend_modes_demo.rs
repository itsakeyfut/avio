//! Layer two video clips using a photographic blend mode and optional opacity.
//!
//! Demonstrates the 14 working photographic [`BlendMode`] variants by blending
//! a top (overlay) video over a base (bottom) video frame-by-frame.  The blend
//! mode and opacity are selected at runtime via command-line arguments.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example blend_modes_demo --features "decode encode filter" -- \
//!   --base base.mp4 --top overlay.mp4 --mode multiply --opacity 1.0 \
//!   --output result.mp4
//! ```
//!
//! Supported `--mode` values: `normal`, `multiply`, `screen`, `overlay`,
//! `soft_light`, `hard_light`, `color_dodge`, `color_burn`, `darken`,
//! `lighten`, `difference`, `exclusion`, `add`, `subtract`.

use std::{path::PathBuf, process};

use ff_decode::VideoDecoder;
use ff_encode::VideoEncoder;
use ff_filter::{BlendMode, FilterGraph, FilterGraphBuilder};
use ff_format::{AlphaMode, VideoCodec};

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    base: PathBuf,
    top: PathBuf,
    output: PathBuf,
    mode: BlendMode,
    opacity: f32,
}

fn parse_mode(s: &str) -> Option<BlendMode> {
    match s {
        "normal" => Some(BlendMode::Normal),
        "multiply" => Some(BlendMode::Multiply),
        "screen" => Some(BlendMode::Screen),
        "overlay" => Some(BlendMode::Overlay),
        "soft_light" => Some(BlendMode::SoftLight),
        "hard_light" => Some(BlendMode::HardLight),
        "color_dodge" => Some(BlendMode::ColorDodge),
        "color_burn" => Some(BlendMode::ColorBurn),
        "darken" => Some(BlendMode::Darken),
        "lighten" => Some(BlendMode::Lighten),
        "difference" => Some(BlendMode::Difference),
        "exclusion" => Some(BlendMode::Exclusion),
        "add" => Some(BlendMode::Add),
        "subtract" => Some(BlendMode::Subtract),
        _ => None,
    }
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    let get = |flag: &str| -> Option<String> {
        raw.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
    };

    let base = if let Some(p) = get("--base") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --base <path> is required");
        process::exit(1);
    };
    let top = if let Some(p) = get("--top") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --top <path> is required");
        process::exit(1);
    };
    let output = if let Some(p) = get("--output") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --output <path> is required");
        process::exit(1);
    };

    let mode = if let Some(ref s) = get("--mode") {
        if let Some(m) = parse_mode(s) {
            m
        } else {
            eprintln!(
                "error: unknown mode '{s}' — supported: normal, multiply, screen, overlay, \
                 soft_light, hard_light, color_dodge, color_burn, darken, lighten, \
                 difference, exclusion, add, subtract"
            );
            process::exit(1);
        }
    } else {
        eprintln!("error: --mode <mode> is required");
        process::exit(1);
    };

    let opacity: f32 = get("--opacity").and_then(|v| v.parse().ok()).unwrap_or(1.0);

    Args {
        base,
        top,
        output,
        mode,
        opacity,
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // ── 1. Open decoders ──────────────────────────────────────────────────────

    let mut base_dec = match VideoDecoder::open(&args.base).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: failed to open base '{}': {e}", args.base.display());
            process::exit(1);
        }
    };

    let mut top_dec = match VideoDecoder::open(&args.top).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: failed to open top '{}': {e}", args.top.display());
            process::exit(1);
        }
    };

    let width = base_dec.width();
    let height = base_dec.height();
    let fps = base_dec.frame_rate();

    println!(
        "Base:    {}  ({width}x{height}  {fps:.2} fps)",
        args.base.display()
    );
    println!("Top:     {}", args.top.display());
    println!(
        "Mode:    {mode:?}  opacity={opacity:.2}",
        mode = args.mode,
        opacity = args.opacity
    );
    println!("Output:  {}", args.output.display());
    println!();

    // ── 2. Build blend filter graph ───────────────────────────────────────────
    //
    // The main builder (slot 0) receives the base layer; the top builder
    // (slot 1) receives the overlay layer.  Both builders are empty here
    // because no per-input preprocessing is applied.

    let top_builder = FilterGraphBuilder::new();

    let mut graph = match FilterGraph::builder()
        .blend(top_builder, args.mode, args.opacity, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: failed to build filter graph: {e}");
            return;
        }
    };

    // ── 3. Build encoder ──────────────────────────────────────────────────────

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

    println!("Encoding...");

    // ── 4. Blend loop ─────────────────────────────────────────────────────────
    //
    // Each iteration decodes one frame from each source, pushes the base to
    // slot 0 and the top overlay to slot 1, then drains blended frames from
    // the filter graph into the encoder.  The loop ends when either source is
    // exhausted.

    let mut frames: u64 = 0;

    loop {
        let base_frame = match base_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: base decode failed: {e}");
                process::exit(1);
            }
        };

        let top_frame = match top_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: top decode failed: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = graph.push_video(0, &base_frame) {
            eprintln!("error: push_video(base) failed: {e}");
            process::exit(1);
        }

        if let Err(e) = graph.push_video(1, &top_frame) {
            eprintln!("error: push_video(top) failed: {e}");
            process::exit(1);
        }

        loop {
            match graph.pull_video() {
                Ok(Some(blended)) => {
                    if let Err(e) = encoder.push_video(&blended) {
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

    // ── 5. Finish ─────────────────────────────────────────────────────────────

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
