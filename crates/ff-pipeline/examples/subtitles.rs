//! Burn hard subtitles into video using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `srt` — burn SRT subtitles from a `.srt` file (plain text, timed)
//!   `ass` — burn ASS/SSA styled subtitles from a `.ass` or `.ssa` file
//!
//! # Usage
//!
//! ```bash
//! cargo run --example subtitles -- \
//!   --input   input.mp4        \
//!   --output  subtitled.mp4    \
//!   --effect  srt              \
//!   --sub-file subtitles.srt   # path to subtitle file (required)
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
    let mut sub_file = None::<String>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--sub-file" => sub_file = Some(args.next().unwrap_or_default()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: subtitles --input <file> --output <file> \
             --effect srt|ass --sub-file <subtitle-file>"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (srt|ass)");
        process::exit(1);
    });
    let sub_file = sub_file.unwrap_or_else(|| {
        eprintln!("--sub-file is required");
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
    let sub_name = Path::new(&sub_file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&sub_file);

    // Build filter graph

    let filter_result = match effect.as_str() {
        "srt" => {
            println!("Input:   {in_name}");
            println!("Effect:  subtitles_srt  (file={sub_name})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().subtitles_srt(&sub_file).build()
        }
        "ass" => {
            println!("Input:   {in_name}");
            println!("Effect:  subtitles_ass  (file={sub_name})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().subtitles_ass(&sub_file).build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try srt, ass)");
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
