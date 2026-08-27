//! Compute peak and RMS amplitude per time interval for an audio file.
//!
//! Uses [`WaveformAnalyzer`] to produce waveform data suitable for rendering
//! audio waveform displays.  Each sample covers one configurable time interval
//! and reports peak and RMS amplitudes in dBFS.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example waveform --features decode -- --input audio.mp3
//! cargo run --example waveform --features decode -- --input audio.mp3 --interval-ms 50
//! ```

use std::process;
use std::time::Duration;

use ff_analysis::{WaveformAnalyzer, WaveformSample};

fn fmt_db(db: f32) -> String {
    if db == f32::NEG_INFINITY {
        "-inf".to_string()
    } else {
        format!("{db:+.1}")
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut interval_ms = 100u64;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--interval-ms" => {
                let raw = args.next().unwrap_or_default();
                interval_ms = raw.parse().unwrap_or_else(|_| {
                    eprintln!("Invalid interval-ms: {raw}");
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
        eprintln!("Usage: waveform --input <audio> [--interval-ms <ms>]");
        process::exit(1);
    });

    println!("Analyzing waveform: {input}");
    println!("Interval: {interval_ms} ms");
    println!();

    let samples: Vec<WaveformSample> = WaveformAnalyzer::new(&input)
        .interval(Duration::from_millis(interval_ms))
        .run()
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    println!("{} interval(s) analyzed.", samples.len());
    println!();

    // Print first 20 samples to avoid flooding the terminal.
    let display_count = samples.len().min(20);
    println!(
        "{:<12}  {:>10}  {:>10}",
        "Time (s)", "Peak dBFS", "RMS dBFS"
    );
    println!("{}", "-".repeat(38));
    for s in samples.iter().take(display_count) {
        let secs = s.timestamp.as_secs_f64();
        println!(
            "{secs:<12.3}  {:>10}  {:>10}",
            fmt_db(s.peak_db),
            fmt_db(s.rms_db)
        );
    }
    if samples.len() > display_count {
        println!("  … ({} more intervals)", samples.len() - display_count);
    }
}
