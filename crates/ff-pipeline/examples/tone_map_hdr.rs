//! Convert an HDR video to SDR using tone mapping.
//!
//! Demonstrates:
//! - `FilterGraphBuilder::tone_map()` — add an HDR-to-SDR tone mapping filter
//! - `ToneMap::Hable` / `ToneMap::Reinhard` / `ToneMap::Mobius` — algorithm choice
//! - `VideoStreamInfo::is_hdr()` — detect HDR content before processing
//! - `VideoStreamInfo::color_space()` / `color_primaries()` — HDR metadata
//!
//! The three algorithms produce different results:
//!   Hable    — filmic, preserves highlights (default)
//!   Reinhard — simple, photographic tone mapping
//!   Mobius   — smooth roll-off, good for animation
//!
//! The example skips gracefully if the input is not flagged as HDR.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example tone_map_hdr -- \
//!   --input   input_hdr.mp4   \
//!   --output  output_sdr.mp4  \
//!   [--method hable|reinhard|mobius]
//! ```

use std::{path::Path, process};

use ff_encode::BitrateMode;
use ff_filter::{FilterGraphBuilder, ToneMap};
use ff_format::VideoCodec;
use ff_pipeline::{EncoderConfig, Pipeline, PipelineError};
use ff_probe::open;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut method_str = "hable".to_string();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--method" => method_str = args.next().unwrap_or_else(|| "hable".to_string()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: tone_map_hdr --input <hdr_file> --output <sdr_file> \
             [--method hable|reinhard|mobius]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // ── Parse ToneMap variant ─────────────────────────────────────────────────

    let tone_map = match method_str.to_lowercase().as_str() {
        "hable" => ToneMap::Hable,
        "reinhard" => ToneMap::Reinhard,
        "mobius" => ToneMap::Mobius,
        other => {
            eprintln!("Unknown method '{other}' (try hable, reinhard, mobius)");
            process::exit(1);
        }
    };

    // ── Probe: detect HDR and print colour metadata ───────────────────────────

    let info = match open(&input) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Error probing input: {e}");
            process::exit(1);
        }
    };

    let Some(video) = info.primary_video() else {
        eprintln!("Error: no video stream found in '{input}'");
        process::exit(1);
    };

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!(
        "Input:    {in_name}  {}×{}  {:.2} fps  codec={}",
        video.width(),
        video.height(),
        video.fps(),
        video.codec_name()
    );
    println!(
        "HDR info: is_hdr={}  color_space={}  color_primaries={}",
        video.is_hdr(),
        video.color_space(),
        video.color_primaries()
    );

    // ── Skip if not HDR ───────────────────────────────────────────────────────

    if !video.is_hdr() {
        println!(
            "Skipping: '{in_name}' is not flagged as HDR \
             (color_space={}, color_primaries={}).",
            video.color_space(),
            video.color_primaries()
        );
        println!(
            "Tip: supply an HDR source with BT.2020 primaries and \
             PQ or HLG transfer characteristics."
        );
        return;
    }

    println!("Output:   {out_name}  tone_map={method_str}");
    println!();

    // ── Build tone-mapping filter ─────────────────────────────────────────────
    //
    // tone_map() inserts libavfilter's `tonemap` filter into the graph.
    // The three ToneMap variants select the tone-mapping algorithm:
    //   Hable    → "hable"
    //   Reinhard → "reinhard"
    //   Mobius   → "mobius"

    let filter = match FilterGraphBuilder::new().tone_map(tone_map).build() {
        Ok(fg) => fg,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    // ── Run pipeline ──────────────────────────────────────────────────────────

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Crf(23))
        .build();

    match Pipeline::builder()
        .input(&input)
        .filter(filter)
        .output(&output, config)
        .build()
    {
        Ok(p) => match p.run() {
            Ok(()) => {}
            Err(PipelineError::Decode(e)) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
            Err(PipelineError::Encode(e)) => {
                eprintln!("Encode error: {e}");
                process::exit(1);
            }
            Err(e) => {
                eprintln!("Pipeline error: {e}");
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Error building pipeline: {e}");
            process::exit(1);
        }
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
