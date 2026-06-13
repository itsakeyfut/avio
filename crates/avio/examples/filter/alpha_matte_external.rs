//! Composite a video using a separate grayscale matte stream as the alpha channel.
//!
//! An external matte is a greyscale clip where white pixels are opaque and black
//! pixels are transparent.  This workflow is standard for pre-rendered mattes
//! (e.g. rotoscoped mattes, CG renders with a separate alpha pass).
//!
//! The pipeline runs in two stages:
//!
//! 1. **Alpha stage**: `FilterGraphBuilder::alpha_matte(matte_builder)` merges
//!    the foreground video (slot 0) with the grayscale matte (slot 1), producing
//!    an YUVA frame where the matte's luma drives the alpha channel.
//!
//! 2. **Composite stage**: `FilterGraphBuilder::composite(CompositeOp::Over)`
//!    places the alpha-matted foreground (slot 1) over the background (slot 0).
//!
//! All three inputs must share the same resolution.  The output inherits the
//! background dimensions.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example alpha_matte_external --features "decode encode filter" -- \
//!   --fg foreground.mp4 --matte matte.mp4 --bg background.mp4 --output result.mp4
//! ```

use std::{path::PathBuf, process};

use avio::{
    AlphaMode, CompositeOp, FilterGraph, FilterGraphBuilder, VideoCodec, VideoDecoder, VideoEncoder,
};

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    fg: PathBuf,
    matte: PathBuf,
    bg: PathBuf,
    output: PathBuf,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    let get = |flag: &str| -> Option<String> {
        raw.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
    };

    let fg = if let Some(p) = get("--fg") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --fg <path> is required");
        process::exit(1);
    };
    let matte = if let Some(p) = get("--matte") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --matte <path> is required");
        process::exit(1);
    };
    let bg = if let Some(p) = get("--bg") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --bg <path> is required");
        process::exit(1);
    };
    let output = if let Some(p) = get("--output") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --output <path> is required");
        process::exit(1);
    };

    Args {
        fg,
        matte,
        bg,
        output,
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // ── 1. Open decoders ──────────────────────────────────────────────────────

    let mut bg_dec = match VideoDecoder::open(&args.bg).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "error: failed to open background '{}': {e}",
                args.bg.display()
            );
            process::exit(1);
        }
    };

    let mut fg_dec = match VideoDecoder::open(&args.fg).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "error: failed to open foreground '{}': {e}",
                args.fg.display()
            );
            process::exit(1);
        }
    };

    let mut matte_dec = match VideoDecoder::open(&args.matte).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "error: failed to open matte '{}': {e}",
                args.matte.display()
            );
            process::exit(1);
        }
    };

    let width = bg_dec.width();
    let height = bg_dec.height();
    let fps = bg_dec.frame_rate();

    println!("Foreground: {}", args.fg.display());
    println!("Matte:      {}", args.matte.display());
    println!(
        "Background: {}  ({width}x{height}  {fps:.2} fps)",
        args.bg.display()
    );
    println!("Output:     {}", args.output.display());
    println!();

    // ── 2. Build stage-1 graph: alpha_matte(fg + matte) ──────────────────────
    //
    // Stage 1 merges the foreground with its matte:
    //   slot 0 → foreground video
    //   slot 1 → grayscale matte (luma → alpha channel)
    // Output: YUVA frame with the matte applied as alpha.

    let matte_builder = FilterGraphBuilder::new();

    let mut alpha_graph = match FilterGraphBuilder::new().alpha_matte(matte_builder).build() {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: failed to build alpha_matte graph: {e}");
            return;
        }
    };

    // ── 3. Build stage-2 graph: composite(Over) ───────────────────────────────
    //
    // Stage 2 composites the alpha-matted foreground over the background:
    //   slot 0 → background video
    //   slot 1 → alpha-matted foreground (from stage 1)

    let top_builder = FilterGraphBuilder::new();

    let mut blend_graph = match FilterGraph::builder()
        .composite(top_builder, CompositeOp::Over, 1.0, AlphaMode::Straight)
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: failed to build blend graph: {e}");
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

    println!("Encoding...");

    // ── 5. Two-stage compositing loop ─────────────────────────────────────────
    //
    // Each iteration:
    //   (a) Decode fg → alpha_graph slot 0; decode matte → alpha_graph slot 1
    //       → pull alpha-matted fg frame
    //   (b) Decode bg → blend_graph slot 0; push alpha-matted fg → slot 1
    //       → pull composited frame → encode

    let mut frames: u64 = 0;

    loop {
        // ── Stage 1: apply alpha matte ────────────────────────────────────────

        let fg_frame = match fg_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: foreground decode failed: {e}");
                process::exit(1);
            }
        };

        let matte_frame = match matte_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: matte decode failed: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = alpha_graph.push_video(0, &fg_frame) {
            eprintln!("error: alpha_graph push_video(fg) failed: {e}");
            process::exit(1);
        }

        if let Err(e) = alpha_graph.push_video(1, &matte_frame) {
            eprintln!("error: alpha_graph push_video(matte) failed: {e}");
            process::exit(1);
        }

        let alpha_frame = match alpha_graph.pull_video() {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("error: alpha_graph pull_video failed: {e}");
                process::exit(1);
            }
        };

        // ── Stage 2: composite over background ───────────────────────────────

        let bg_frame = match bg_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: background decode failed: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = blend_graph.push_video(0, &bg_frame) {
            eprintln!("error: blend_graph push_video(bg) failed: {e}");
            process::exit(1);
        }

        if let Err(e) = blend_graph.push_video(1, &alpha_frame) {
            eprintln!("error: blend_graph push_video(alpha fg) failed: {e}");
            process::exit(1);
        }

        loop {
            match blend_graph.pull_video() {
                Ok(Some(composited)) => {
                    if let Err(e) = encoder.push_video(&composited) {
                        eprintln!("error: encoder push_video failed: {e}");
                        process::exit(1);
                    }
                    frames += 1;
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("error: blend_graph pull_video failed: {e}");
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
