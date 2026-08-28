//! Apply audio dynamics processing using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `gate`       — noise gate: silence audio below a threshold
//!   `compressor` — dynamic range compressor: reduce peaks above a threshold
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_dynamics -- \
//!   --input   input.mp4      \
//!   --output  processed.mp4  \
//!   --effect  gate           \
//!   [--threshold  -40.0]     # gate/compressor: threshold in dBFS (default: -40.0)
//!   [--attack      10.0]     # gate/compressor: attack time in ms (default: 10.0)
//!   [--release    100.0]     # gate/compressor: release time in ms (default: 100.0)
//!   [--ratio        4.0]     # compressor: compression ratio e.g. 4.0 = 4:1 (default: 4.0)
//!   [--makeup       6.0]     # compressor: make-up gain in dB (default: 6.0)
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
};

use ff_filter::FilterGraphBuilder;
use ff_format::{AudioCodec, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline, Progress};

fn render_progress(p: &Progress) {
    match p.percent() {
        Some(pct) => {
            let bar_width = 20usize;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let bar = "=".repeat(filled) + &" ".repeat(bar_width - filled);
            print!("\r{pct:5.1}%  [{bar}]    ");
        }
        None => {
            print!("\r{} frames    ", p.frames_processed);
        }
    }
    let _ = io::stdout().flush();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut effect = None::<String>;
    let mut threshold: f32 = -40.0;
    let mut attack: f32 = 10.0;
    let mut release: f32 = 100.0;
    let mut ratio: f32 = 4.0;
    let mut makeup: f32 = 6.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--threshold" => {
                threshold = args.next().unwrap_or_default().parse().unwrap_or(-40.0);
            }
            "--attack" => attack = args.next().unwrap_or_default().parse().unwrap_or(10.0),
            "--release" => {
                release = args.next().unwrap_or_default().parse().unwrap_or(100.0);
            }
            "--ratio" => ratio = args.next().unwrap_or_default().parse().unwrap_or(4.0),
            "--makeup" => makeup = args.next().unwrap_or_default().parse().unwrap_or(6.0),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: audio_dynamics --input <file> --output <file> \
             --effect gate|compressor [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (gate|compressor)");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    // ── Build filter graph ────────────────────────────────────────────────────

    let filter_result = match effect.as_str() {
        "gate" => {
            println!("Input:   {in_name}");
            println!(
                "Effect:  agate  \
                 (threshold={threshold:.1} dBFS  attack={attack:.0} ms  release={release:.0} ms)"
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .agate(threshold, attack, release)
                .build()
        }
        "compressor" => {
            println!("Input:   {in_name}");
            println!(
                "Effect:  acompressor  \
                 (threshold={threshold:.1} dBFS  ratio={ratio:.1}:1  \
                 attack={attack:.0} ms  release={release:.0} ms  makeup={makeup:+.1} dB)"
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .compressor(threshold, ratio, attack, release, makeup)
                .build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try gate, compressor)");
            process::exit(1);
        }
    };

    let filter = match filter_result {
        Ok(fg) => fg,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    println!();

    // ── Assemble pipeline ─────────────────────────────────────────────────────

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .audio_codec(AudioCodec::Aac)
        .crf(23)
        .build();

    let pipeline = match Pipeline::builder()
        .input(&input)
        .filter(filter)
        .output(&output, config)
        .on_progress(|p: &Progress| {
            render_progress(p);
            true
        })
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = pipeline.run() {
        println!();
        eprintln!("Error: {e}");
        process::exit(1);
    }

    println!();

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
