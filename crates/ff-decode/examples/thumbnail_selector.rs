//! Automatically select the best thumbnail frame from a video file.
//!
//! Uses [`ThumbnailSelector`] to pick the single most representative frame by
//! scoring candidates for brightness and sharpness.  Near-black, near-white,
//! and blurry frames are skipped; the sharpest acceptable frame is returned.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example thumbnail_selector -- --input video.mp4
//! cargo run --example thumbnail_selector -- \
//!     --input video.mp4 --interval-secs 10
//! ```

use std::process;
use std::time::Duration;

use ff_decode::ThumbnailSelector;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut interval_secs = 5u64;

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
        eprintln!("Usage: thumbnail_selector --input <video> [--interval-secs <s>]");
        process::exit(1);
    });

    println!("Selecting best thumbnail from: {input}");
    println!("Candidate interval: {interval_secs}s");
    println!();

    let frame = ThumbnailSelector::new(&input)
        .candidate_interval(Duration::from_secs(interval_secs))
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    let ts = frame.timestamp().as_duration();
    let ts_secs = frame.timestamp().as_secs_f64();
    let h = ts.as_secs() / 3600;
    let m = (ts.as_secs() % 3600) / 60;
    let s = ts.as_secs() % 60;
    let ms = ts.subsec_millis();

    println!("Best thumbnail frame:");
    println!("  Timestamp:    {h:02}:{m:02}:{s:02}.{ms:03}  ({ts_secs:.3}s)");
    println!("  Dimensions:   {}×{}", frame.width(), frame.height());
    println!("  Pixel format: {:?}", frame.format());
    println!();
    println!("To save this frame, pipe its raw bytes to an image encoder or use");
    println!("the `encode` feature with ImageEncoder.");
}
