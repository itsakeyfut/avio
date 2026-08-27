//! Decode a video file and re-encode it (video only) to a different codec or quality.
//!
//! Demonstrates the video-only decode → encode pipeline using `VideoPipeline`
//! — a high-level builder that wraps the manual decode/encode loop.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example video_transcode -- \
//!   --input   input.mp4   \
//!   --output  output.mp4  \
//!   [--crf    23]          # CRF quality value (default: 23)
//!   [--bitrate 4000000]    # target CBR bitrate in bps (overrides --crf)
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoCodecEncodeExt};
use ff_format::VideoCodec;
use ff_pipeline::VideoPipeline;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut crf: u32 = 23;
    let mut bitrate: Option<u64> = None;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--crf" => {
                let v = args.next().unwrap_or_default();
                crf = v.parse().unwrap_or(23);
            }
            "--bitrate" => {
                let v = args.next().unwrap_or_default();
                bitrate = v.parse().ok();
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: video_transcode --input <file> --output <file> \
             [--crf N] [--bitrate N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // ── Probe input for display info ──────────────────────────────────────────

    let dec = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let width = dec.width();
    let height = dec.height();
    let fps = dec.frame_rate();
    let in_codec = dec.stream_info().codec_name().to_string();
    drop(dec);

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    let out_codec = VideoCodec::H264;
    let bmode = match bitrate {
        Some(bps) => BitrateMode::Cbr(bps),
        None => BitrateMode::Crf(crf),
    };
    let quality_str = match &bmode {
        BitrateMode::Cbr(bps) => format!("bitrate={bps}"),
        BitrateMode::Crf(q) => format!("crf={q}"),
        BitrateMode::Vbr { .. } => "custom".to_string(),
    };

    println!("Input:   {in_name}  {width}×{height}  {fps:.2} fps  {in_codec}");
    println!(
        "Output:  {out_name}  {width}×{height}  {fps:.2} fps  {}  {quality_str}",
        out_codec.default_extension()
    );
    println!();
    println!("Encoding (video only)...");

    // ── Run pipeline ──────────────────────────────────────────────────────────

    if let Err(e) = VideoPipeline::new()
        .input(&input)
        .output(&output)
        .video_codec(out_codec)
        .bitrate_mode(bmode)
        .mute()
        .run()
    {
        eprintln!("Error: {e}");
        process::exit(1);
    }

    let size_str = match std::fs::metadata(&output) {
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

    println!("Done. {out_name}  {size_str}");
}
