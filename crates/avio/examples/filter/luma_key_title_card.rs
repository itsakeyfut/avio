//! Composite a title card over a background using luma keying.
//!
//! Luma keying makes pixels transparent based on their luminance value.  A
//! white-background title card can be keyed by setting a high threshold (e.g.
//! `0.9`), making bright areas transparent and leaving the dark title text
//! opaque.  For dark-background cards, pass `--invert` to key out the shadows
//! instead.
//!
//! The keyed title card is composited over the background video using
//! Porter-Duff Over blending.
//!
//! Both input videos must have the same resolution; the output inherits the
//! background dimensions.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example luma_key_title_card --features "decode encode filter" -- \
//!   --title title.mp4 --bg background.mp4 --output result.mp4 \
//!   [--threshold 0.9] [--tolerance 0.1] [--softness 0.0] [--invert]
//! ```

use std::{path::PathBuf, process};

use avio::{
    AlphaMode, BlendMode, FilterGraph, FilterGraphBuilder, VideoCodec, VideoDecoder, VideoEncoder,
};

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    title: PathBuf,
    bg: PathBuf,
    output: PathBuf,
    threshold: f32,
    tolerance: f32,
    softness: f32,
    invert: bool,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    let get = |flag: &str| -> Option<String> {
        raw.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
    };

    let title = if let Some(p) = get("--title") {
        PathBuf::from(p)
    } else {
        eprintln!("error: --title <path> is required");
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
        title,
        bg,
        output,
        threshold: get("--threshold")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.9),
        tolerance: get("--tolerance")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.1),
        softness: get("--softness")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        invert: raw.iter().any(|a| a == "--invert"),
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

    let mut title_dec = match VideoDecoder::open(&args.title).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "error: failed to open title card '{}': {e}",
                args.title.display()
            );
            process::exit(1);
        }
    };

    let width = bg_dec.width();
    let height = bg_dec.height();
    let fps = bg_dec.frame_rate();

    println!(
        "Background: {}  ({width}x{height}  {fps:.2} fps)",
        args.bg.display()
    );
    println!("Title card: {}", args.title.display());
    println!(
        "Luma key:   threshold={:.2}  tolerance={:.2}  softness={:.2}  invert={}",
        args.threshold, args.tolerance, args.softness, args.invert,
    );
    println!("Output:     {}", args.output.display());
    println!();

    // ── 2. Build compositing filter graph ─────────────────────────────────────
    //
    // title_builder: lumakey removes the bright (or dark if inverted) background
    // from the title card (slot 1).  The main graph composites the keyed title
    // over the background (slot 0) using Porter-Duff Over.

    let title_builder = FilterGraphBuilder::new().lumakey(
        args.threshold,
        args.tolerance,
        args.softness,
        args.invert,
    );

    let mut graph = match FilterGraph::builder()
        .blend(
            title_builder,
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
    // to slot 0 and the title card to slot 1, then drains composited frames
    // into the encoder.  The loop ends when either source is exhausted.

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

        let title_frame = match title_dec.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("error: title card decode failed: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = graph.push_video(0, &bg_frame) {
            eprintln!("error: push_video(background) failed: {e}");
            process::exit(1);
        }

        if let Err(e) = graph.push_video(1, &title_frame) {
            eprintln!("error: push_video(title) failed: {e}");
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
