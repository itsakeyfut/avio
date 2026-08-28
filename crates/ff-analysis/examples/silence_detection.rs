//! Detect silent intervals in an audio file and print their time ranges.
//!
//! Uses [`SilenceDetector`] to find audio segments where the amplitude stays
//! below a configurable threshold for at least a minimum duration.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example silence_detection --features decode -- --input audio.mp3
//! cargo run --example silence_detection --features decode -- \
//!     --input audio.mp3 --threshold-db -50 --min-duration-ms 300
//! ```

use std::process;
use std::time::Duration;

use ff_analysis::{SilenceDetector, SilenceRange};

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    format!("{secs:.3}s")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut threshold_db = -40.0_f32;
    let mut min_duration_ms = 500u64;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--threshold-db" => {
                let raw = args.next().unwrap_or_default();
                threshold_db = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid threshold-db: {raw}");
                    process::exit(1);
                });
            }
            "--min-duration-ms" => {
                let raw = args.next().unwrap_or_default();
                min_duration_ms = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid min-duration-ms: {raw}");
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
        eprintln!(
            "Usage: silence_detection --input <audio> \
             [--threshold-db <dB>] [--min-duration-ms <ms>]"
        );
        process::exit(1);
    });

    println!("Detecting silence in: {input}");
    println!("Threshold: {threshold_db:.1} dBFS");
    println!("Minimum duration: {min_duration_ms} ms");
    println!();

    let ranges: Vec<SilenceRange> = SilenceDetector::new(&input)
        .threshold_db(threshold_db)
        .min_duration(Duration::from_millis(min_duration_ms))
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    if ranges.is_empty() {
        println!("No silence detected.");
    } else {
        println!("Detected {} silent interval(s):", ranges.len());
        println!("{:<14}  {:<14}  {:<12}", "Start", "End", "Duration");
        println!("{}", "-".repeat(44));
        for r in &ranges {
            let duration = r.end.saturating_sub(r.start);
            println!(
                "{:<14}  {:<14}  {:<12}",
                fmt_duration(r.start),
                fmt_duration(r.end),
                fmt_duration(duration)
            );
        }
    }
}
