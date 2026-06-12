//! Remove a green-screen background using chromakey and composite the subject
//! over a replacement background with Porter-Duff Over blending.
//!
//! The foreground video is expected to contain a subject filmed against a solid
//! coloured background (default: `0x00FF00` green).  The `chromakey` filter
//! removes that colour, turning matching pixels transparent.  The keyed
//! foreground is then blended over the background using
//! [`BlendMode::PorterDuffOver`].
//!
//! Both input videos must have the same resolution; the output inherits the
//! background dimensions.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example chroma_key_green_screen --features "decode encode filter" -- \
//!   --fg foreground.mp4 --bg background.mp4 --output composited.mp4 \
//!   [--color 0x00FF00] [--similarity 0.3] [--blend-factor 0.0]
//! ```

use std::{path::PathBuf, process};

use avio::{
    AlphaMode, BlendMode, FilterGraph, FilterGraphBuilder, VideoCodec, VideoDecoder, VideoEncoder,
};

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    fg: PathBuf,
    bg: PathBuf,
    output: PathBuf,
    color: String,
    similarity: f32,
    blend_factor: f32,
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
        bg,
        output,
        color: get("--color").unwrap_or_else(|| "0x00FF00".to_string()),
        similarity: get("--similarity")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.3),
        blend_factor: get("--blend-factor")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
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

    let width = bg_dec.width();
    let height = bg_dec.height();
    let fps = bg_dec.frame_rate();

    println!("Foreground: {}", args.fg.display());
    println!(
        "Background: {}  ({width}x{height}  {fps:.2} fps)",
        args.bg.display()
    );
    println!(
        "Key color:  {}  similarity={:.2}  blend_factor={:.2}",
        args.color, args.similarity, args.blend_factor,
    );
    println!("Output:     {}", args.output.display());
    println!();

    // ── 2. Build compositing filter graph ─────────────────────────────────────
    //
    // fg_builder: chromakey removes the key colour from the foreground (slot 1).
    // The main graph blends the keyed foreground over the background (slot 0)
    // using Porter-Duff Over compositing.

    let fg_builder =
        FilterGraphBuilder::new().chromakey(&args.color, args.similarity, args.blend_factor);

    let mut graph = match FilterGraph::builder()
        .blend(
            fg_builder,
            BlendMode::PorterDuffOver,
            1.0,
            AlphaMode::Straight,
        )
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

    // ── 4. Compositing loop ───────────────────────────────────────────────────
    //
    // Each iteration decodes one frame from each source, pushes the background
    // to slot 0 and the foreground to slot 1, then drains composited frames
    // from the filter graph into the encoder.  The loop ends when either source
    // is exhausted.

    let mut frames: u64 = 0;

    loop {
        let bg_frame = match bg_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: background decode failed: {e}");
                process::exit(1);
            }
        };

        let fg_frame = match fg_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: foreground decode failed: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = graph.push_video(0, &bg_frame) {
            eprintln!("error: push_video(background) failed: {e}");
            process::exit(1);
        }

        if let Err(e) = graph.push_video(1, &fg_frame) {
            eprintln!("error: push_video(foreground) failed: {e}");
            process::exit(1);
        }

        loop {
            match graph.pull_video() {
                Ok(Some(composited)) => {
                    if let Err(e) = encoder.push_video(&composited) {
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
