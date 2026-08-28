//! Inspect per-frame PTS timestamps from an audio file.
//!
//! Demonstrates `Timestamp::is_valid()`, `as_secs_f64()`, and `pts()` on frames
//! returned by `AudioDecoder`. Each frame carries a PTS that lets you know exactly
//! where in the stream it belongs — useful for A/V sync, subtitle alignment, or
//! seeking verification.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example audio_timestamps -- \
//!   --input  audio.mp3   \
//!   [--frames 10]        # number of frames to inspect (default: 10)
//! ```

use std::process;

use ff_decode::AudioDecoder;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut max_frames: usize = 10;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--frames" | "-n" => {
                let v = args.next().unwrap_or_default();
                max_frames = v.parse().unwrap_or(10);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: audio_timestamps --input <file> [--frames N]");
        process::exit(1);
    });

    let mut decoder = match AudioDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening '{input}': {e}");
            process::exit(1);
        }
    };

    println!("Input:      {input}");
    println!("Channels:   {}", decoder.channels());
    println!("SampleRate: {} Hz", decoder.sample_rate());
    println!();
    println!(
        "{:<6}  {:<12}  {:<10}  samples",
        "frame", "time (s)", "pts (raw)"
    );
    println!("{}", "-".repeat(44));

    let mut frame_count: usize = 0;
    let mut invalid_count: usize = 0;
    let mut last_secs: Option<f64> = None;

    for result in decoder.by_ref().take(max_frames) {
        let frame = match result {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        frame_count += 1;
        let ts = frame.timestamp();

        if ts.is_valid() {
            let secs = ts.as_secs_f64();
            let gap = last_secs.map(|prev| format!("  Δ={:.4}s", secs - prev));
            println!(
                "{:<6}  {:<12.6}  {:<10}  {}{}",
                frame_count,
                secs,
                ts.pts(),
                frame.sample_count(),
                gap.unwrap_or_default(),
            );
            last_secs = Some(secs);
        } else {
            invalid_count += 1;
            println!(
                "{:<6}  {:<12}  {:<10}  {}",
                frame_count,
                "(no pts)",
                "—",
                frame.sample_count(),
            );
        }
    }

    println!("{}", "-".repeat(44));
    println!("{frame_count} frames shown.");
    if invalid_count > 0 {
        println!("Warning: {invalid_count} frame(s) had no PTS (AV_NOPTS_VALUE).");
    }
}
