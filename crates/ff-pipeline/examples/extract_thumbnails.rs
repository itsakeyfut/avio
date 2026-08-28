//! Extract JPEG thumbnails at given timestamps using `ThumbnailPipeline`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example extract_thumbnails --features pipeline -- \
//!   --input  video.mp4 \
//!   --output thumbs/   \
//!   --times  0,30,60,90 \
//!   [--width 320]
//! ```

use std::{process, time::Duration};

use ff_decode::VideoDecoder;
use ff_pipeline::ThumbnailPipeline;

fn parse_time(s: &str) -> Result<f64, String> {
    // Accept plain seconds "30" or HH:MM:SS "00:01:30"
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
            Err(format!(
                "invalid time format '{s}' (use HH:MM:SS or plain seconds)"
            ))
        }
    } else {
        s.parse::<f64>().map_err(|_| format!("invalid time '{s}'"))
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut times_str = None::<String>;
    let mut width: u32 = 320;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--times" | "-t" => times_str = Some(args.next().unwrap_or_default()),
            "--width" => {
                let v = args.next().unwrap_or_default();
                width = v.parse().unwrap_or(320);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: extract_thumbnails --input <file> --output <dir> --times 0,30,60");
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let times_str = times_str.unwrap_or_else(|| {
        eprintln!("--times is required (e.g. 0,30,60)");
        process::exit(1);
    });

    // Parse timestamps
    let timestamps: Vec<f64> = times_str
        .split(',')
        .map(|s| {
            parse_time(s.trim()).unwrap_or_else(|e| {
                eprintln!("Error parsing time: {e}");
                process::exit(1);
            })
        })
        .collect();

    // Probe source duration so out-of-range timestamps can be skipped gracefully.
    let src_duration = match VideoDecoder::open(&input).build() {
        Ok(dec) => dec.duration(),
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };

    // Filter timestamps that are within the file duration
    let valid_timestamps: Vec<f64> = timestamps
        .iter()
        .copied()
        .filter(|&t| Duration::from_secs_f64(t) <= src_duration)
        .collect();
    let skipped = timestamps.len() - valid_timestamps.len();

    let in_name = std::path::Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    println!(
        "Extracting {} thumbnails from {in_name}...",
        valid_timestamps.len()
    );
    if skipped > 0 {
        println!("  (skipping {skipped} timestamps beyond file duration)");
    }

    let paths = match ThumbnailPipeline::new(&input)
        .timestamps(valid_timestamps.clone())
        .output_dir(&output)
        .width(width)
        .quality(85)
        .run_to_files()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    for (path, &timestamp) in paths.iter().zip(valid_timestamps.iter()) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let secs = timestamp as u64;
        let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        println!("  {h:02}:{m:02}:{s:02}  →  {name}");
    }

    println!("Done.");
}
