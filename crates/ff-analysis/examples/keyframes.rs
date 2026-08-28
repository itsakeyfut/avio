//! Enumerate keyframe timestamps in a video file.
//!
//! Uses [`KeyframeEnumerator`] to list the presentation timestamps of all
//! keyframes (I-frames) in a video stream.  Only packet headers are read —
//! no decoding is performed — so this is fast even for large files.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example keyframes -- --input video.mp4
//! cargo run --example keyframes -- --input video.mp4 --stream 0
//! ```

use std::process;

use ff_analysis::KeyframeEnumerator;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut stream_index = None::<usize>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--stream" | "-s" => {
                let raw = args.next().unwrap_or_default();
                stream_index = Some(raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid stream index: {raw}");
                    process::exit(1);
                }));
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: keyframes --input <video> [--stream <index>]");
        process::exit(1);
    });

    println!("Enumerating keyframes in: {input}");
    if let Some(idx) = stream_index {
        println!("Stream index: {idx}");
    } else {
        println!("Stream: first video stream (default)");
    }
    println!();

    let mut enumerator = KeyframeEnumerator::new(&input);
    if let Some(idx) = stream_index {
        enumerator = enumerator.stream_index(idx);
    }

    let keyframes = enumerator.run().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    println!("Found {} keyframe(s):", keyframes.len());

    // Print first 30 keyframes to avoid flooding the terminal.
    let display_count = keyframes.len().min(30);
    for (i, ts) in keyframes.iter().take(display_count).enumerate() {
        let h = ts.as_secs() / 3600;
        let m = (ts.as_secs() % 3600) / 60;
        let s = ts.as_secs() % 60;
        let ms = ts.subsec_millis();
        println!("  [{i:4}] {h:02}:{m:02}:{s:02}.{ms:03}");
    }
    if keyframes.len() > display_count {
        println!("  … ({} more keyframes)", keyframes.len() - display_count);
    }

    if !keyframes.is_empty() {
        let avg_interval_ms = keyframes
            .windows(2)
            .map(|w| w[1].saturating_sub(w[0]).as_millis())
            .sum::<u128>()
            .checked_div((keyframes.len() - 1) as u128)
            .unwrap_or(0);
        println!();
        println!("Average keyframe interval: {avg_interval_ms} ms");
    }
}
