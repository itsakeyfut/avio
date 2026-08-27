//! Detect black intervals in a video file and print their start timestamps.
//!
//! Uses [`BlackFrameDetector`] to identify segments where the proportion of
//! near-black pixels exceeds a configurable threshold.  Black intervals are
//! commonly used to detect chapter breaks, fade-to-black transitions, and
//! advertising boundaries.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example black_frames --features decode -- --input video.mp4
//! cargo run --example black_frames --features decode -- --input video.mp4 --threshold 0.2
//! ```

use std::process;
use std::time::Duration;

use ff_analysis::BlackFrameDetector;

fn fmt_duration(d: Duration) -> String {
    let h = d.as_secs() / 3600;
    let m = (d.as_secs() % 3600) / 60;
    let s = d.as_secs() % 60;
    let ms = d.subsec_millis();
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut threshold = 0.1_f64;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--threshold" | "-t" => {
                let raw = args.next().unwrap_or_default();
                threshold = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid threshold: {raw}");
                    process::exit(1);
                });
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: black_frames --input <video> [--threshold <0.0–1.0>]");
        process::exit(1);
    });

    println!("Detecting black frames in: {input}");
    println!("Threshold: {threshold:.2}");
    println!();

    let black_starts = BlackFrameDetector::new(&input)
        .threshold(threshold)
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    if black_starts.is_empty() {
        println!("No black intervals detected.");
    } else {
        println!("Detected {} black interval(s):", black_starts.len());
        for (i, ts) in black_starts.iter().enumerate() {
            println!("  [{i:3}] {}", fmt_duration(*ts));
        }
    }
}
