//! Normalize audio loudness using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `loudness` — EBU R128 two-pass integrated loudness normalization
//!   `peak`     — peak normalization to a target dBFS ceiling
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_normalize --features pipeline -- \
//!   --input   input.mp4      \
//!   --output  normalized.mp4 \
//!   --effect  loudness       \
//!   [--target-lufs  -23.0]   # loudness: target integrated loudness in LUFS (default: -23.0)
//!   [--true-peak    -1.0]    # loudness: true-peak ceiling in dBTP (default: -1.0)
//!   [--lra           7.0]    # loudness: loudness range target in LU (default: 7.0)
//!   [--peak-db      -1.0]    # peak: target dBFS ceiling (default: -1.0)
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
    let mut target_lufs: f32 = -23.0;
    let mut true_peak: f32 = -1.0;
    let mut lra: f32 = 7.0;
    let mut peak_db: f32 = -1.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--target-lufs" => {
                target_lufs = args.next().unwrap_or_default().parse().unwrap_or(-23.0);
            }
            "--true-peak" => {
                true_peak = args.next().unwrap_or_default().parse().unwrap_or(-1.0);
            }
            "--lra" => lra = args.next().unwrap_or_default().parse().unwrap_or(7.0),
            "--peak-db" => {
                peak_db = args.next().unwrap_or_default().parse().unwrap_or(-1.0);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: audio_normalize --input <file> --output <file> \
             --effect loudness|peak [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (loudness|peak)");
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
        "loudness" => {
            println!("Input:   {in_name}");
            println!(
                "Effect:  loudness_normalize  \
                 (target={target_lufs:.1} LUFS  true_peak={true_peak:.1} dBTP  lra={lra:.1} LU)"
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .loudness_normalize(target_lufs, true_peak, lra)
                .build()
        }
        "peak" => {
            println!("Input:   {in_name}");
            println!("Effect:  normalize_peak  (target={peak_db:.1} dBFS)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().normalize_peak(peak_db).build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try loudness, peak)");
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
