//! Concatenate multiple video files end-to-end using `Pipeline`'s multi-input mode.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example concat_clips --features pipeline -- \
//!   --output joined.mp4 \
//!   clip1.mp4 clip2.mp4 clip3.mp4
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
    time::Duration,
};

use ff_format::{AudioCodec, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline, Progress};
use ff_probe::open;

fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    let m = s / 60;
    format!("{:02}:{:02}", m, s % 60)
}

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
            print!("\r{pct:5.1}%  [{bar}]  {}    ", format_elapsed(p.elapsed));
        }
        None => {
            print!(
                "\r{} frames  {}    ",
                p.frames_processed,
                format_elapsed(p.elapsed)
            );
        }
    }
    let _ = io::stdout().flush();
}

fn main() {
    let mut args_iter = std::env::args().skip(1);
    let mut output = None::<String>;
    let mut inputs: Vec<String> = Vec::new();

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--output" | "-o" => output = Some(args_iter.next().unwrap_or_default()),
            other if other.starts_with('-') => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
            _ => inputs.push(arg),
        }
    }

    let output = output.unwrap_or_else(|| {
        eprintln!("Usage: concat_clips --output <file> clip1.mp4 clip2.mp4 ...");
        process::exit(1);
    });

    if inputs.len() < 2 {
        eprintln!("Error: at least two input clips are required");
        process::exit(1);
    }

    // Probe each input
    let mut total_duration = Duration::ZERO;
    let mut first_width = 0u32;
    let mut first_height = 0u32;

    println!("Inputs ({}):", inputs.len());
    for (i, path) in inputs.iter().enumerate() {
        let name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);
        match open(path) {
            Ok(info) => {
                let dur = info.duration();
                let w = info.video_streams().first().map_or(0, |v| v.width());
                let h = info.video_streams().first().map_or(0, |v| v.height());
                if i == 0 {
                    first_width = w;
                    first_height = h;
                } else if w != first_width || h != first_height {
                    println!(
                        "  Warning: #{i} resolution {w}×{h} differs from #0 {first_width}×{first_height}; will use source dimensions"
                    );
                }
                total_duration += dur;
                println!("  #{i}  {name}  {w}×{h}  {}", format_duration(dur));
            }
            Err(e) => {
                eprintln!("  Error probing {name}: {e}");
                process::exit(1);
            }
        }
    }

    println!("Expected total: {}", format_duration(total_duration));
    println!();

    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .audio_codec(AudioCodec::Aac)
        .crf(23)
        .build();

    let mut builder = Pipeline::builder();
    for input in &inputs {
        builder = builder.input(input);
    }
    let pipeline = match builder
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
