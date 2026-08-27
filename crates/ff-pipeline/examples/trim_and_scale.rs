//! Trim a segment from a video and scale it using `FilterGraph` + `Pipeline`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example trim_and_scale --features pipeline -- \
//!   --input   input.mp4 \
//!   --output  clip.mp4  \
//!   --start   00:00:10  \
//!   --end     00:00:40  \
//!   --width   1280      \
//!   --height  720
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
    time::Duration,
};

use ff_filter::FilterGraphBuilder;
use ff_format::{AudioCodec, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline, Progress};

fn parse_time(s: &str) -> Result<f64, String> {
    if s.contains(':') {
        let parts: Vec<&str> = s.splitn(3, ':').collect();
        if parts.len() == 3 {
            let h: f64 = parts[0]
                .parse()
                .map_err(|_| format!("invalid hours in '{s}'"))?;
            let m: f64 = parts[1]
                .parse()
                .map_err(|_| format!("invalid minutes in '{s}'"))?;
            let sec: f64 = parts[2]
                .parse()
                .map_err(|_| format!("invalid seconds in '{s}'"))?;
            Ok(h * 3600.0 + m * 60.0 + sec)
        } else {
            Err(format!("invalid time '{s}'"))
        }
    } else {
        s.parse::<f64>().map_err(|_| format!("invalid time '{s}'"))
    }
}

fn format_duration(secs: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = secs as u64;
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
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut start_str = None::<String>;
    let mut end_str = None::<String>;
    let mut width = None::<u32>;
    let mut height = None::<u32>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--start" => start_str = Some(args.next().unwrap_or_default()),
            "--end" => end_str = Some(args.next().unwrap_or_default()),
            "--width" => {
                let v = args.next().unwrap_or_default();
                width = v.parse().ok();
            }
            "--height" => {
                let v = args.next().unwrap_or_default();
                height = v.parse().ok();
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: trim_and_scale --input <file> --output <file> --start <time> --end <time> [--width W] [--height H]");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let start_str = start_str.unwrap_or_else(|| {
        eprintln!("--start is required");
        process::exit(1);
    });
    let end_str = end_str.unwrap_or_else(|| {
        eprintln!("--end is required");
        process::exit(1);
    });

    let start = parse_time(&start_str).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });
    let end = parse_time(&end_str).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    if end <= start {
        eprintln!("Error: --end ({end_str}) must be greater than --start ({start_str})");
        process::exit(1);
    }

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:  {in_name}");
    println!(
        "Trim:   {} → {}  ({:.1} s)",
        format_duration(start),
        format_duration(end),
        end - start
    );
    if let (Some(w), Some(h)) = (width, height) {
        println!("Scale:  → {w}×{h}");
    }
    println!("Output: {out_name}");
    println!();

    // Build filter graph: trim → scale (if requested)
    let mut builder = FilterGraphBuilder::new().trim(start, end);
    if let (Some(w), Some(h)) = (width, height) {
        builder = builder.scale(w, h, ff_filter::ScaleAlgorithm::Fast);
    }
    let filter = match builder.build() {
        Ok(fg) => fg,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

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

    println!("Done. {out_name}  {size_str}  ({:.1} s)", end - start);
}
