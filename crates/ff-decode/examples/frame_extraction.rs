//! Extract one frame per time interval from a video file.
//!
//! Uses [`FrameExtractor`] to sample frames at regular intervals across the
//! full video duration.  Prints the timestamp, dimensions, and pixel format
//! of each extracted frame.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example frame_extraction --features decode -- --input video.mp4
//! cargo run --example frame_extraction --features decode -- --input video.mp4 --interval-secs 5
//! ```

use std::process;
use std::time::Duration;

use ff_decode::FrameExtractor;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut interval_secs = 1u64;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--interval-secs" | "-s" => {
                let raw = args.next().unwrap_or_default();
                interval_secs = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid interval-secs: {raw}");
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
        eprintln!("Usage: frame_extraction --input <video> [--interval-secs <s>]");
        process::exit(1);
    });

    println!("Extracting frames from: {input}");
    println!("Interval: {interval_secs}s");
    println!();

    let frames = FrameExtractor::new(&input)
        .interval(Duration::from_secs(interval_secs))
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    println!("Extracted {} frame(s):", frames.len());
    println!(
        "{:<6}  {:<12}  {:<10}  {:<10}  Pixel Format",
        "Index", "Timestamp (s)", "Width", "Height"
    );
    println!("{}", "-".repeat(60));

    for (i, frame) in frames.iter().enumerate() {
        let ts_secs = frame.timestamp().as_secs_f64();
        println!(
            "{:<6}  {:<12.3}  {:<10}  {:<10}  {:?}",
            i,
            ts_secs,
            frame.width(),
            frame.height(),
            frame.format(),
        );
    }
}
