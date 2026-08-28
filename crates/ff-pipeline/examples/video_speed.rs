//! Change video playback speed using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `speed`        — change playback speed (fast / slow motion)
//!   `reverse`      — reverse video playback (buffers entire clip)
//!   `areverse`     — reverse audio playback (buffers entire clip)
//!   `freeze-frame` — freeze a specific frame for a given duration
//!
//! # Usage
//!
//! ```bash
//! cargo run --example video_speed -- \
//!   --input   input.mp4   \
//!   --output  out.mp4     \
//!   --effect  speed       \
//!   [--factor   2.0]       # speed factor > 1.0 = faster, < 1.0 = slower (default: 2.0)
//!   [--pts      5.0]       # freeze-frame: PTS in seconds to freeze (default: 5.0)
//!   [--duration 3.0]       # freeze-frame: duration to hold the frame (default: 3.0)
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
    let mut factor: f64 = 2.0;
    let mut pts: f64 = 5.0;
    let mut duration: f64 = 3.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--factor" => factor = args.next().unwrap_or_default().parse().unwrap_or(2.0),
            "--pts" => pts = args.next().unwrap_or_default().parse().unwrap_or(5.0),
            "--duration" => duration = args.next().unwrap_or_default().parse().unwrap_or(3.0),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: video_speed --input <file> --output <file> \
             --effect speed|reverse|areverse|freeze-frame [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (speed|reverse|areverse|freeze-frame)");
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

    // Build filter graph

    let filter_result = match effect.as_str() {
        "speed" => {
            let label = if factor > 1.0 {
                "fast motion"
            } else {
                "slow motion"
            };
            println!("Input:   {in_name}");
            println!("Effect:  speed  (factor={factor:.2}×  {label})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().speed(factor).build()
        }
        "reverse" => {
            println!("Input:   {in_name}");
            println!("Effect:  reverse  (video — buffers entire clip)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().reverse().build()
        }
        "areverse" => {
            println!("Input:   {in_name}");
            println!("Effect:  areverse  (audio — buffers entire clip)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().areverse().build()
        }
        "freeze-frame" => {
            println!("Input:   {in_name}");
            println!("Effect:  freeze_frame  (pts={pts:.1}s  duration={duration:.1}s)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .freeze_frame(pts, duration)
                .build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try speed, reverse, areverse, freeze-frame)");
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

    // Assemble pipeline

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
