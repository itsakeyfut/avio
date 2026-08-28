//! Detect scene changes in a video file and print their timestamps.
//!
//! Uses [`SceneDetector`] to identify hard cuts and transitions. The detection
//! threshold controls sensitivity: lower values report more cuts (including
//! subtle ones); higher values report only hard cuts.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example scene_detection -- --input video.mp4
//! cargo run --example scene_detection -- --input video.mp4 --threshold 0.3
//! ```

use std::process;

use ff_analysis::SceneDetector;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut threshold = 0.4_f64;

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
        eprintln!("Usage: scene_detection --input <video> [--threshold <0.0–1.0>]");
        process::exit(1);
    });

    println!("Detecting scene changes in: {input}");
    println!("Threshold: {threshold:.2}");
    println!();

    let cuts = SceneDetector::new(&input)
        .threshold(threshold)
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    if cuts.is_empty() {
        println!("No scene changes detected.");
    } else {
        println!("Detected {} scene change(s):", cuts.len());
        for (i, ts) in cuts.iter().enumerate() {
            let secs = ts.as_secs_f64();
            let h = ts.as_secs() / 3600;
            let m = (ts.as_secs() % 3600) / 60;
            let s = ts.as_secs() % 60;
            let ms = ts.subsec_millis();
            println!("  [{i:3}] {h:02}:{m:02}:{s:02}.{ms:03}  ({secs:.3}s)");
        }
    }
}
